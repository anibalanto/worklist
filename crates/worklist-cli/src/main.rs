use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::BufRead;
use std::path::PathBuf;
use std::process::Command;
use worklist::check_push::{check_one, classify, RefClass};
use worklist::board::{dry_run_plan, JiraBoard, Board};
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
        #[arg(long)]
        stdin: bool,
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
        /// Recorta aunque la rama ya exista con commits propios, descartandolos.
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
        Cmd::AssignKeys { project, board, base, stdin, dry_run } => {
            cmd_assign_keys(project, board, base, stdin, dry_run)
        }
        Cmd::CreateOrFind { project, r#type, source, titulo, dry_run } => {
            let description = format!("Fuente: {source}");
            if dry_run {
                println!("{}", dry_run_plan(&project, &r#type, &titulo, &description)?);
                return Ok(());
            }
            worklist::port::preflight()?;
            let board = JiraBoard::new(project);
            // Sin `--parent`: este comando resuelve un item suelto, y la
            // jerarquia la calcula `assign-keys` sobre la ventana entera.
            let outcome = board.create_or_find(&titulo, &r#type, &description, None)?;
            println!("{}", outcome.key());
            Ok(())
        }
    }
}

fn cmd_assign_keys(
    project: String,
    board_id: String,
    base: String,
    stdin: bool,
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

    let lines = read_hook_lines(stdin)?;
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
            println!(
                "  sprint {} -> {}{}: {} issue(s) agregados, {} ya estaban",
                s.id,
                s.key,
                creado,
                s.added.len(),
                s.already
            );
        }
        if r.new_head != r.old_head {
            println!("{}: {} -> {}", r.refname, short(&r.old_head), short(&r.new_head));
        }
    }
    Ok(())
}

fn short(sha: &str) -> &str {
    &sha[..7.min(sha.len())]
}

fn read_hook_lines(stdin: bool) -> Result<Vec<(String, String, String)>> {
    if !stdin {
        let head = rev_parse("HEAD")?;
        let branch = current_branch()?;
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
        let head = rev_parse("HEAD")?;
        let branch = current_branch()?;
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
    }
    if any_rejected {
        std::process::exit(1);
    }
    Ok(())
}

fn current_branch() -> Result<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("git rev-parse")?;
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

fn rev_parse(refname: &str) -> Result<String> {
    let out = Command::new("git").args(["rev-parse", refname]).output()?;
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}


