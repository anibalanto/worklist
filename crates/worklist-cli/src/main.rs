use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::BufRead;
use std::path::PathBuf;
use std::process::Command;
use worklist::check_push::{check_one, classify, RefClass};
use worklist::board::{dry_run_plan, JiraBoard, Board};
use worklist::propagate::Verdict;
use worklist::provider::FileProvider;

#[derive(Parser)]
#[command(name = "worklist")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// El compare-and-swap de una ventana. Pensado para `hooks/pre-receive`.
    CheckPush {
        /// El proveedor de prueba: un archivo `clave -> status`. Solo informa
        /// el status, asi que con el no se compara ni el cuerpo ni el titulo.
        #[arg(long, conflicts_with = "project")]
        provider_file: Option<PathBuf>,
        /// El proveedor real, por `acli`. Informa status, titulo y cuerpo.
        #[arg(long, conflicts_with = "provider_file")]
        project: Option<String>,
        /// Lee `<viejo> <nuevo> <ref>` por linea — el protocolo de pre-receive.
        #[arg(long)]
        stdin: bool,
    },
    /// Recorta una ventana: la rama con los items de un sprint y nada mas.
    Window {
        #[command(subcommand)]
        sub: WindowCmd,
    },
    /// Manipula el proveedor de prueba directamente, sin pasar por git.
    Provider {
        #[command(subcommand)]
        sub: ProviderCmd,
    },
    /// Resuelve los pedidos de una ventana: clave, renombre y reescritura.
    /// Pensado para `hooks/post-receive`.
    AssignKeys {
        #[arg(long)]
        project: String,
        /// El id del board donde vive el sprint de la ventana. Es de la
        /// instalacion, no del worklist, y **no tiene default**: sin el la
        /// pasada del sprint no puede correr, y saltearla sola porque falta un
        /// dato de configuracion es la peor forma de enterarse de que falta.
        #[arg(long)]
        board: String,
        /// Base del proveedor, para traducir los links a otros items.
        #[arg(long, default_value = "https://lamansys.atlassian.net")]
        base: String,
        /// Lee `<viejo> <nuevo> <ref>` por linea — el protocolo del hook.
        #[arg(long, conflicts_with_all = ["window", "all_windows"])]
        stdin: bool,
        /// Una ventana por su id: `--window 1`. Se puede repetir.
        ///
        /// **`--stdin` es para el hook; esto es para una persona.** Reconciliar
        /// una ventana ya resuelta obligaba a imitar el protocolo del hook a
        /// mano — un `rev-parse`, un `echo` con el sha repetido, y saber el
        /// nombre de la ref. Ver la task `6q`.
        #[arg(long = "window", conflicts_with = "all_windows")]
        window: Vec<String>,
        /// Todas las ventanas del repo, en orden numerico.
        #[arg(long)]
        all_windows: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Le da clave del proveedor a lo que no la tiene, sobre el panorama y sin
    /// prometer que la rama se verifique. Es el camino del backlog.
    Bootstrap {
        #[arg(long)]
        project: String,
        /// Sobre que rama. Por defecto el panorama, que es donde vive todo.
        #[arg(long = "ref", default_value = "refs/heads/insecure/all")]
        refname: String,
        /// Base del proveedor, para traducir los links a otros items.
        #[arg(long, default_value = "https://lamansys.atlassian.net")]
        base: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Sube al panorama lo que una ventana resolvio. Corre al final del
    /// `post-receive`, y a mano para reintentar lo que no subio.
    Propagate {
        /// Base del proveedor: la normalizacion se **rehace** arriba, y
        /// traducir los links necesita saber contra que host.
        #[arg(long, default_value = "https://lamansys.atlassian.net")]
        base: String,
        /// Lee `<viejo> <nuevo> <ref>` por linea — el protocolo del hook.
        #[arg(long, conflicts_with_all = ["window", "all_windows"])]
        stdin: bool,
        /// Una ventana por su id: `--window 1`. Se puede repetir.
        #[arg(long = "window", conflicts_with = "all_windows")]
        window: Vec<String>,
        /// Todas las ventanas del repo, en orden numerico.
        #[arg(long)]
        all_windows: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Busca un issue por titulo antes de crear, para no duplicar en un reintento.
    CreateOrFind {
        #[arg(long)]
        project: String,
        /// task, user-story o epic — el vocabulario del worklist, no el de Jira.
        #[arg(long)]
        r#type: String,
        #[arg(long)]
        source: String,
        titulo: String,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum WindowCmd {
    Open {
        sprint_id: String,
        #[arg(long, default_value = "insecure/all")]
        from: String,
        #[arg(long)]
        dry_run: bool,
        /// No replanta: el corte nuevo reemplaza a la ventana, descartando
        /// lo que tenia encima. Ver `commands/window-open.md`.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum ProviderCmd {
    SetStatus {
        #[arg(long)]
        provider_file: PathBuf,
        clave: String,
        status: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::CheckPush { provider_file, project, stdin } => {
            cmd_check_push(provider_file, project, stdin)
        }
        Cmd::Provider { sub: ProviderCmd::SetStatus { provider_file, clave, status } } => {
            let provider = FileProvider::new(provider_file);
            let old = provider.set_status(&clave, &status)?;
            match old {
                Some(old) => println!("{clave}: {old} -> {status}"),
                None => println!("{clave}: (nuevo) -> {status}"),
            }
            Ok(())
        }
        Cmd::Window { sub: WindowCmd::Open { sprint_id, from, dry_run, force } } => {
            let repo = std::env::current_dir()?;
            let (files, head) = worklist::window::open(&repo, &sprint_id, &from, dry_run, force)?;
            println!("secure/sprint/{sprint_id}: {} archivo(s)", files.len());
            for f in &files {
                println!("  {f}");
            }
            if !dry_run {
                println!("recortado desde {from} -> {}", short(&head));
            }
            Ok(())
        }
        Cmd::AssignKeys { project, board, base, stdin, window, all_windows, dry_run } => {
            cmd_assign_keys(project, board, base, stdin, &window, all_windows, dry_run)
        }
        Cmd::Bootstrap { project, refname, base, dry_run } => {
            cmd_bootstrap(project, refname, base, dry_run)
        }
        Cmd::Propagate { stdin, window, all_windows, base, dry_run } => {
            cmd_propagate(stdin, &window, all_windows, &base, dry_run)
        }
        Cmd::CreateOrFind { project, r#type, source, titulo, dry_run } => {
            cmd_create_or_find(project, r#type, source, titulo, dry_run)
        }
    }
}

/// Busca por titulo antes de crear, para que un reintento no duplique.
///
/// Extraida de `main` como sus hermanas, y no por prolijidad: mientras vivio
/// adentro del `match`, el bilink que gobierna su § "Comportamiento" apuntaba a
/// `fn main` entero, asi que **cualquier subcomando nuevo lo ensuciaba**. Un
/// bilink que se enciende por trabajo ajeno no señala nada. Ver
/// `commands/create-or-find.md`.
fn cmd_create_or_find(
    project: String,
    item_type: String,
    source: String,
    titulo: String,
    dry_run: bool,
) -> Result<()> {
    let description = format!("Fuente: {source}");
    if dry_run {
        println!("{}", dry_run_plan(&project, &item_type, &titulo, &description)?);
        return Ok(());
    }
    worklist::port::preflight()?;
    let board = JiraBoard::new(project);
    // Sin `--parent`: este comando resuelve un item suelto, y la jerarquia la
    // calcula `assign-keys` sobre la ventana entera.
    let outcome = board.create_or_find(&titulo, &item_type, &description, None)?;
    println!("{}", outcome.key());
    Ok(())
}

fn cmd_assign_keys(
    project: String,
    board_id: String,
    base: String,
    stdin: bool,
    windows: &[String],
    all_windows: bool,
    dry_run: bool,
) -> Result<()> {
    // Antes de mirar el arbol: sin credencial, la mitad de abajo de la tabla
    // del puerto no existe, y enterarse con una ventana a medio resolver es la
    // peor forma. `--dry-run` no habla con nadie, asi que no la pide.
    if !dry_run {
        worklist::port::preflight()?;
    }
    let repo = std::env::current_dir()?;
    let board = JiraBoard::new(project);

    let lines = if all_windows || !windows.is_empty() {
        window_lines(&repo, windows, all_windows)?
    } else {
        read_hook_lines(&repo, stdin)?
    };
    // Una vez, antes del lote: que falte el panorama es del repo, no de cada
    // ventana. Ver la task `77`.
    let hay_panorama = worklist::propagate::has_panorama(&repo);
    if !hay_panorama {
        println!(
            "aviso: este repo no tiene {} — lo que se resuelva no sube a ningun lado",
            worklist::propagate::PANORAMA
        );
    }
    for (old, new, refname) in lines {
        if classify(&refname) != RefClass::Secure {
            continue;
        }
        let r = worklist::assign::assign_window(
            &repo, &refname, &old, &new, &base, &board, &board_id, dry_run,
        )?;
        let Some(r) = r else {
            println!("{refname}: nada que resolver ni que actualizar");
            continue;
        };
        if !r.assigned.is_empty() {
            println!("{}: {} pedido(s)", r.refname, r.assigned.len());
            println!("  orden: {}", r.order.join(", "));
        }
        for a in &r.assigned {
            let refs = match a.rewritten {
                0 => String::new(),
                n => format!("  ({n} refs reescritas)"),
            };
            let padre = match &a.parent {
                Some(p) => format!("  [parent {p}]"),
                None => String::new(),
            };
            println!("  {} -> {}{}{}", a.slug, a.key, refs, padre);
            // Ya no es un aviso: el issue existia, su epica estaba mal y se
            // corrigio. Se informa lo que se hizo, no lo que no se pudo.
            if a.parent_fixed {
                println!(
                    "  · {}: ya existia y quedo bajo {}",
                    a.key,
                    a.parent.as_deref().unwrap_or("?")
                );
            }
        }
        for (blocker, blocked) in &r.linked {
            println!("  vinculo: {blocker} Blocks {blocked}");
        }
        for (story, task) in &r.related {
            println!("  vinculo: {story} Relates {task}");
        }
        for key in &r.updated {
            println!("  actualizado en el proveedor: {key}");
        }
        for (dep, key) in &r.untranslated {
            println!(
                "  ! {key} depende de '{dep}', que no esta en esta ventana — el vinculo no se creo"
            );
        }
        if let Some(s) = &r.sprint {
            let creado = if s.created { " (creado)" } else { "" };
            match s.already {
                Some(ya) => println!(
                    "  sprint {} -> {}{}: {} issue(s) agregados, {ya} ya estaban",
                    s.id,
                    s.key,
                    creado,
                    s.added.len()
                ),
                // Sin haber preguntado, lo unico cierto es la membresia que se
                // calculo de `items`. Cuantos ya estan del otro lado no se sabe.
                None => println!(
                    "  sprint {} -> {}: {} miembro(s) calculados; no se pregunto cuantos ya estan",
                    s.id,
                    s.key,
                    s.added.len()
                ),
            }
        }
        if r.new_head != r.old_head {
            println!("{}: {} -> {}", r.refname, short(&r.old_head), short(&r.new_head));
        }
        // Y recien ahora sube: lo que mas falta arriba son las claves, y las
        // acaba de escribir esta corrida. Ver `concepts/propagation.md`.
        //
        // Que la propagacion falle no deshace lo resuelto: la ventana queda
        // adelantada del panorama, que es un estado del que se sale
        // reintentando con `worklist propagate`.
        if hay_panorama {
            match worklist::propagate::propagate(&repo, &r.refname, &r.new_head, &base, dry_run) {
                Ok(Some(p)) => report_propagated(&p, dry_run),
                Ok(None) => {}
                Err(e) => println!("  ! el panorama no avanzo: {e}"),
            }
        }
    }
    Ok(())
}

/// El bootstrap del panorama: clave para lo que no la tiene.
///
/// **No propaga ni verifica.** Escribe en el panorama porque el que escribe es
/// el servidor, que es lo mismo que ya hace la propagacion — `insecure/**`
/// rechaza al cliente, no al servidor.
fn cmd_bootstrap(project: String, refname: String, base: String, dry_run: bool) -> Result<()> {
    if !dry_run {
        worklist::port::preflight()?;
    }
    let repo = std::env::current_dir()?;
    let board = JiraBoard::new(project);
    let Some(r) = worklist::assign::bootstrap(&repo, &refname, &base, &board, dry_run)? else {
        println!("{refname}: no hay ningun item sin clave");
        return Ok(());
    };
    let verbo = if dry_run { "pediria clave para" } else { "resolvio" };
    println!("{}: {verbo} {} item(s)", r.refname, r.assigned.len());
    for a in &r.assigned {
        let refs = match a.rewritten {
            0 => String::new(),
            n => format!("  ({n} refs reescritas)"),
        };
        let padre = match &a.parent {
            Some(p) => format!("  [parent {p}]"),
            None => String::new(),
        };
        // Encontrado no es creado, y la diferencia se ve: sobre lo encontrado
        // el cuerpo no viaja, porque no hay con que probar que no se pisa.
        //
        // **En dry-run no se sabe cual es cual**, porque no se le pregunto a
        // nadie. Decir "ya existia" sobre los 258 seria afirmar sobre el board
        // sin haberlo mirado, que es el defecto que `67` corrigio.
        let como = match (dry_run, a.created) {
            (true, _) => "",
            (false, true) => "",
            (false, false) => "  (ya existia: el cuerpo no se toco)",
        };
        println!("  {} -> {}{}{}{}", a.slug, a.key, refs, padre, como);
    }
    if !dry_run && r.new_head != r.old_head {
        println!("{}: {} -> {}", r.refname, short(&r.old_head), short(&r.new_head));
    }
    Ok(())
}

/// Sube lo que las ventanas nombradas resolvieron.
///
/// **No pide credencial**: la propagacion es entre ramas de git y no habla con
/// ningun proveedor. Es lo que la deja reintentable cuando el token falta.
fn cmd_propagate(stdin: bool, windows: &[String], all_windows: bool, base: &str, dry_run: bool) -> Result<()> {
    let repo = std::env::current_dir()?;
    let lines = if all_windows || !windows.is_empty() {
        window_lines(&repo, windows, all_windows)?
    } else {
        read_hook_lines(&repo, stdin)?
    };
    if !worklist::propagate::has_panorama(&repo) {
        anyhow::bail!(
            "este repo no tiene {} — no hay a donde propagar.\n\
             Corre esto parado en el repo donde vive el panorama.",
            worklist::propagate::PANORAMA
        );
    }
    for (_, new, refname) in lines {
        if classify(&refname) != RefClass::Secure {
            continue;
        }
        match worklist::propagate::propagate(&repo, &refname, &new, base, dry_run)? {
            Some(p) => report_propagated(&p, dry_run),
            None => println!("{refname}: el panorama ya tiene todo lo suyo"),
        }
    }
    Ok(())
}

fn report_propagated(p: &worklist::propagate::Propagated, dry_run: bool) {
    use worklist::propagate::Step;
    let verbo = if dry_run { "subiria" } else { "sube" };
    println!("{}: {verbo} {} commit(s) al panorama", p.refname, p.steps.len());
    for step in &p.steps {
        match step {
            Step::Picked { sha, subject } => println!("  {} {subject}", short(sha)),
            Step::Empty { sha, subject } => {
                println!("  {} {subject}  (el panorama ya lo tenia)", short(sha))
            }
            // El renombre no se copia: se rehace, y su reescritura es la del
            // panorama entero, no la de los 19 archivos de la ventana.
            Step::Renamed { slug, key, rewritten } => {
                let refs = match rewritten {
                    0 => String::new(),
                    n => format!("  ({n} refs reescritas en el panorama)"),
                };
                println!("  rename {slug} -> {key}  (rehecho){refs}");
            }
            Step::AlreadyRenamed { slug, key } => {
                println!("  rename {slug} -> {key}  (el panorama ya lo tenia)")
            }
            // Y la normalizacion tampoco se copia: su contenido es la
            // traduccion de los links leida en el arbol de la ventana.
            Step::Normalized { key } => println!("  normalize: {key}  (rehecho)"),
            Step::AlreadyNormalized { key } => {
                println!("  normalize: {key}  (el panorama ya lo tenia)")
            }
            // Y la clave del sprint es un campo, no un parche: el `.sprint.md`
            // del panorama es el que se planifica.
            Step::SprintKeyed { id, key } => println!("  sprint: {id} -> {key}  (rehecho)"),
            Step::AlreadySprintKeyed { id, key } => {
                println!("  sprint: {id} -> {key}  (el panorama ya lo tenia)")
            }
        }
    }
    if !dry_run && p.panorama_new != p.panorama_old {
        println!(
            "  panorama: {} -> {}",
            short(&p.panorama_old),
            short(&p.panorama_new)
        );
    }
}

fn short(sha: &str) -> &str {
    &sha[..7.min(sha.len())]
}

/// Las ventanas nombradas, como lineas del protocolo del hook.
///
/// **El mismo sha de los dos lados, y no es un truco**: es lo que significa
/// "mira esta ventana, no traigo nada nuevo". Lo que las pasadas tienen que
/// hacer no depende de que algo se haya movido en git — depende de que git y el
/// proveedor puedan diferir. Ver `commands/assign-keys.md`.
fn window_lines(
    repo: &std::path::Path,
    windows: &[String],
    all: bool,
) -> Result<Vec<(String, String, String)>> {
    let refs: Vec<String> = if all {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["for-each-ref", "--format=%(refname)", "refs/heads/secure/"])
            .output()
            .context("git for-each-ref")?;
        if !out.status.success() {
            anyhow::bail!("git for-each-ref fallo: {}", String::from_utf8_lossy(&out.stderr));
        }
        let mut r: Vec<String> =
            String::from_utf8(out.stdout)?.lines().map(|s| s.to_string()).collect();
        if r.is_empty() {
            // **El error nombra donde miro.** Sin eso manda a revisar el repo
            // equivocado, que es la unica forma de equivocarse aca: el comando
            // corre sobre el directorio actual, y el servidor de sincronizacion
            // es un bare aparte del clon donde se trabaja.
            anyhow::bail!(
                "no hay ninguna ventana en {}: refs/heads/secure/** esta vacio\n\
                 \n\
                 este comando corre sobre el repo del directorio actual. Si ese no es\n\
                 el del worklist, parate en el que si lo es.",
                repo.display()
            );
        }
        // Un listado de refs viene ordenado como texto —1, 10, 11, 2— y el que
        // lo lee espera el otro. El orden es de la salida, no del resultado,
        // pero una salida que no se puede seguir es una que nadie mira.
        r.sort_by_key(|s| numero_final(s));
        r
    } else {
        windows.iter().map(|w| format!("refs/heads/secure/sprint/{w}")).collect()
    };

    let mut out = Vec::new();
    for refname in refs {
        let sha = rev_parse(repo, &refname)?;
        if sha.is_empty() {
            anyhow::bail!("la ventana {refname} no existe en {}", repo.display());
        }
        out.push((sha.clone(), sha, refname));
    }
    Ok(out)
}

/// El ultimo tramo numerico de una ref, para ordenar `sprint/2` antes que
/// `sprint/10`. Lo que no termine en numero va al final, junto y estable.
fn numero_final(refname: &str) -> (u64, String) {
    refname
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|n| (n, String::new()))
        .unwrap_or((u64::MAX, refname.to_string()))
}

fn read_hook_lines(repo: &std::path::Path, stdin: bool) -> Result<Vec<(String, String, String)>> {
    if !stdin {
        let head = rev_parse(repo, "HEAD")?;
        let branch = current_branch(repo)?;
        return Ok(vec![(head.clone(), head, format!("refs/heads/{branch}"))]);
    }
    let mut out = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 3 {
            anyhow::bail!("linea invalida, se esperaba '<viejo> <nuevo> <ref>': {line}");
        }
        out.push((parts[0].to_string(), parts[1].to_string(), parts[2].to_string()));
    }
    Ok(out)
}

fn cmd_check_push(
    provider_file: Option<PathBuf>,
    project: Option<String>,
    stdin: bool,
) -> Result<()> {
    // Uno de los dos, y el de prueba solo informa el status: con el, el cuerpo
    // y el titulo no se comparan. Ver `concepts/sync.md`.
    let provider: Box<dyn worklist::provider::Provider> = match (provider_file, project) {
        (Some(f), None) => Box::new(FileProvider::new(f)),
        (None, Some(p)) => {
            worklist::port::preflight()?;
            Box::new(worklist::provider::JiraProvider::new(p))
        }
        _ => anyhow::bail!("hace falta --provider-file o --project, y no los dos"),
    };
    let provider = provider.as_ref();
    let repo = std::env::current_dir()?;

    let lines: Vec<(String, String, String)> = if stdin {
        let mut out = Vec::new();
        for line in std::io::stdin().lock().lines() {
            let line = line?;
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() != 3 {
                anyhow::bail!("linea invalida, se esperaba '<viejo> <nuevo> <ref>': {line}");
            }
            out.push((parts[0].to_string(), parts[1].to_string(), parts[2].to_string()));
        }
        out
    } else {
        let head = rev_parse(&repo, "HEAD")?;
        let branch = current_branch(&repo)?;
        vec![(head.clone(), head, format!("refs/heads/{branch}"))]
    };

    let mut any_rejected = false;
    for (old, new, refname) in lines {
        if classify(&refname) == RefClass::Insecure {
            any_rejected = true;
            println!(
                "reject: {refname} es una rama insegura — no se puede verificar, asi que no acepta escrituras."
            );
            println!("        Recorta una ventana segura (secure/…) y empuja ahi.");
            continue;
        }
        let rejected = check_one(&repo, &old, &new, &refname, provider)?;
        for r in rejected {
            any_rejected = true;
            println!(
                "reject: {} {} era \"{}\" en el tip, el proveedor dice \"{}\"",
                r.key, r.field, r.tip_status, r.live_status
            );
        }
        // Y lo mismo contra el panorama: de `all` se corta todo, asi que un
        // conflicto escrito ahi entra en el proximo recorte de cada ventana.
        // Probarlo aca es lo que evita tener que anotarlo alla.
        match worklist::propagate::would_conflict(&repo, &refname, &new)? {
            Verdict::Applies => {}
            // No poder probar no es que entre. Aceptar en silencio seria decir
            // que se verifico algo que nadie miro. Ver la task `77`.
            Verdict::NoPanorama => println!(
                "aviso: {refname} no se pudo probar contra el panorama — este repo no tiene {}",
                worklist::propagate::PANORAMA
            ),
            Verdict::Conflict { sha, subject, files } => {
                any_rejected = true;
                println!(
                    "reject: {} {subject} no entra al panorama — choca en {}",
                    short(&sha),
                    files.join(", ")
                );
                println!(
                    "        alguien mas escribio eso desde otra ventana. Regenera la tuya y volve a aplicarlo."
                );
            }
        }
    }
    if any_rejected {
        std::process::exit(1);
    }
    Ok(())
}

fn current_branch(repo: &std::path::Path) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("git rev-parse")?;
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

fn rev_parse(repo: &std::path::Path, refname: &str) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(repo).args(["rev-parse", refname]).output()?;
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}



#[cfg(test)]
mod tests {
    use super::numero_final;

    /// Un listado de refs viene ordenado como texto —`sprint/1`, `sprint/10`,
    /// `sprint/11`, `sprint/2`— y el que lo lee espera 1, 2, … 16.
    #[test]
    fn las_ventanas_salen_en_orden_numerico() {
        let mut refs: Vec<&str> = vec![
            "refs/heads/secure/sprint/1",
            "refs/heads/secure/sprint/10",
            "refs/heads/secure/sprint/2",
            "refs/heads/secure/sprint/16",
        ];
        refs.sort_by_key(|s| numero_final(s));
        assert_eq!(
            refs,
            vec![
                "refs/heads/secure/sprint/1",
                "refs/heads/secure/sprint/2",
                "refs/heads/secure/sprint/10",
                "refs/heads/secure/sprint/16",
            ]
        );
    }

    /// Una ventana que no termina en numero —`secure/to-work`, la de `5q`— no
    /// tiene con que ordenarse: va al final y no rompe el orden de las otras.
    #[test]
    fn una_ventana_sin_numero_va_al_final_sin_romper_nada() {
        let mut refs: Vec<&str> = vec![
            "refs/heads/secure/to-work",
            "refs/heads/secure/sprint/2",
            "refs/heads/secure/sprint/1",
        ];
        refs.sort_by_key(|s| numero_final(s));
        assert_eq!(refs[0], "refs/heads/secure/sprint/1");
        assert_eq!(refs[1], "refs/heads/secure/sprint/2");
        assert_eq!(refs[2], "refs/heads/secure/to-work");
    }
}
