use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::BufRead;
use std::path::PathBuf;
use std::process::Command;
use worklist_provider::check_push::{check_one, classify, RefClass};
use worklist_provider::board::{dry_run_plan, JiraBoard, Board};
use worklist_provider::propagate::Verdict;
use worklist_provider::provider::FileProvider;

#[derive(Parser)]
#[command(name = "worklist-server", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Recorta una ventana: la rama con los items de un sprint y nada mas.
    ///
    /// **Es del servidor por los dos criterios a la vez**: lee el panorama,
    /// que vive de un solo lado, y escribe la rama de la ventana, que es un
    /// artefacto. El cliente la trae con `git fetch` y un worktree — no tiene
    /// de donde cortar, porque no tiene el panorama. Ver
    /// `concepts/distribution.md`.
    Window {
        #[command(subcommand)]
        sub: WindowCmd,
    },
    /// El compare-and-swap de una ventana. Pensado para `hooks/pre-receive`.
    CheckPush {
        /// El proveedor de prueba: un archivo `clave -> status`. Solo informa
        /// el status, asi que con el no se compara ni el cuerpo ni el titulo.
        #[arg(long, conflicts_with = "project")]
        provider_file: Option<PathBuf>,
        /// El proveedor real, por `acli`. Informa status, titulo y cuerpo.
        #[arg(long, conflicts_with = "provider_file")]
        project: Option<String>,
        /// El mapeo de estados de esta instalacion, en JSON. **Obligatorio con
        /// `--project`**: el status del worklist y el de Jira no son el mismo
        /// campo, y compararlos sin traducir rechaza todas las ventanas.
        #[arg(long)]
        states_map: Option<PathBuf>,
        /// Base del proveedor. Con `--project` se le piden por REST las
        /// transiciones que el workflow admite, que es lo que decide un rechazo
        /// por regla.
        #[arg(long, default_value = "https://lamansys.atlassian.net")]
        base: String,
        /// El email de la cuenta con la que se habla por REST. Ver `--account`
        /// de `install-hooks`.
        #[arg(long)]
        account: Option<String>,
        /// Lee `<viejo> <nuevo> <ref>` por linea — el protocolo de pre-receive.
        #[arg(long)]
        stdin: bool,
        /// Compara y **no rechaza**: reporta lo que difiere y sale con cero.
        ///
        /// Es lo que hace falta para cruzar la instalacion al proveedor real:
        /// el titulo y el cuerpo nunca se compararon contra nada, asi que
        /// encenderlos de golpe es enterarse de cuantos difieren cuando ya
        /// rechazan. Ver `commands/check-push.md`.
        #[arg(long)]
        dry_run: bool,
        /// Sobre que rama comparar. **Solo con `--dry-run`**, y por defecto el
        /// panorama, que es el unico con el inventario completo. Sin
        /// `--dry-run` el rango sale de `--stdin` o de la rama actual, asi que
        /// aca no significaria nada.
        #[arg(long = "ref")]
        refname: Option<String>,
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
        /// El email de la cuenta con la que se habla por REST. **No es un
        /// secreto y por eso no viaja con el token**: es un dato de la
        /// instalacion, del mismo lado que la base y el id del board. Ver
        /// ADR-0001 § 5.
        #[arg(long)]
        account: Option<String>,
        /// El mapeo de estados de esta instalacion, en JSON. Sin el, los
        /// estados no viajan: no hay con que traducirlos, y **no viajar es
        /// mejor que viajar mal**. Se avisa, no se falla — el resto de las
        /// pasadas si puede correr.
        #[arg(long)]
        states_map: Option<PathBuf>,
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
        /// El board donde viven los sprints. **Obligatorio y sin default**: una
        /// pasada que se saltea sola porque falta configuracion es la peor
        /// forma de enterarse de que falta.
        #[arg(long = "board")]
        board_id: String,
        /// Sobre que rama. Por defecto el panorama, que es donde vive todo.
        #[arg(long = "ref", default_value = "refs/heads/insecure/all")]
        refname: String,
        /// Base del proveedor, para traducir los links a otros items.
        #[arg(long, default_value = "https://lamansys.atlassian.net")]
        base: String,
        /// El email de la cuenta con la que se habla por REST. **No es un
        /// secreto y por eso no viaja con el token**: es un dato de la
        /// instalacion, del mismo lado que la base y el id del board. Ver
        /// ADR-0001 § 5.
        #[arg(long)]
        account: Option<String>,
        /// Crea solo los primeros N y para. Un lote no necesita ser una
        /// transaccion —de eso ya se ocupa el ancla— sino un corte: noventa y
        /// un issues en un board real conviene verlos a la decima.
        #[arg(long)]
        limit: Option<usize>,
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
    /// Adopta los issues que ya existen del otro lado y el panorama no
    /// registro. Es la pasada 1 de `bootstrap` **sin la creacion**.
    ///
    /// **No puede crear, y no hay flag que lo habilite.** Reparar se corre
    /// cuando algo salio mal, que es cuando menos se quiere estar decidiendo
    /// si ademas se va a escribir. Ver `commands/reconcile.md`.
    Reconcile {
        #[arg(long)]
        project: String,
        /// Sobre que rama. Por defecto el panorama, que es donde vive todo.
        #[arg(long = "ref", default_value = "refs/heads/insecure/all")]
        refname: String,
        #[arg(long, default_value = "https://lamansys.atlassian.net")]
        base: String,
        /// El email de la cuenta con la que se habla por REST. **No es un
        /// secreto y por eso no viaja con el token**: es un dato de la
        /// instalacion, del mismo lado que la base y el id del board. Ver
        /// ADR-0001 § 5.
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Escribe `hooks/pre-receive` y `hooks/post-receive` del bare, apuntando
    /// al binario que los esta escribiendo. Ver `commands/install-hooks.md`.
    InstallHooks {
        /// El bare del servidor. Los hooks van en `<repo>/hooks/`.
        #[arg(long)]
        repo: PathBuf,
        #[arg(long)]
        project: String,
        /// El id del board. **Obligatorio y sin default**, por lo mismo que en
        /// `assign-keys`: una pasada que se saltea sola porque falta
        /// configuracion es la peor forma de enterarse de que falta.
        #[arg(long = "board")]
        board_id: String,
        /// El proveedor **de prueba** para el `pre-receive`. Su archivo lleva
        /// valores con la forma del worklist, asi que su mapeo de estados es la
        /// identidad y no necesita `--states-map`.
        #[arg(long)]
        provider_file: Option<PathBuf>,
        /// El mapeo de estados de esta instalacion. **Es lo que permite
        /// instalar contra el proveedor real**: sin el, el `status` del
        /// worklist y el de Jira se comparan crudos y rechazan todas las
        /// ventanas. Va a los dos hooks, porque los dos lo necesitan — uno para
        /// comparar y el otro para mover. Ver `concepts/states.md`.
        #[arg(long)]
        states_map: Option<PathBuf>,
        #[arg(long, default_value = "https://lamansys.atlassian.net")]
        base: String,
        /// El email de la cuenta con la que se habla por REST. **No es un
        /// secreto y por eso no viaja con el token**: es un dato de la
        /// instalacion, del mismo lado que la base y el id del board. Ver
        /// ADR-0001 § 5.
        #[arg(long)]
        account: Option<String>,
        /// Sobrescribe un hook que ya existe.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Trae a la rama de la ventana lo que el proveedor dice y el tip no.
    ///
    /// **Es la unica direccion que faltaba**: todo lo demas va hacia afuera. Un
    /// cambio hecho en el board se detectaba —el push quedaba rechazado— y no
    /// tenia por donde entrar. Es del servidor por los dos criterios, y por eso
    /// `pull` puede traerlo sin que el cliente hable con el proveedor. Ver
    /// `commands/absorb.md`.
    Absorb {
        /// La ventana. Se leen las claves de su tip.
        #[arg(long = "ref")]
        refname: String,
        #[arg(long)]
        provider_file: Option<PathBuf>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        states_map: Option<PathBuf>,
        #[arg(long, default_value = "https://lamansys.atlassian.net")]
        base: String,
        #[arg(long)]
        account: Option<String>,
        /// Dice que absorberia, sin escribir.
        #[arg(long)]
        dry_run: bool,
    },
    /// Cierra las divergencias de estado que un push ya no puede alcanzar.
    ///
    /// **No es lo mismo que la pasada de estados de `assign-keys`**, y por eso
    /// es otro comando: aquella sale de un diff, asi que solo mueve lo que el
    /// push movio. Lo que cambio antes de que hubiera mapeo no vuelve a
    /// cambiar, y su divergencia se queda para siempre. Ver `ACC-317`.
    PushStates {
        #[arg(long)]
        project: String,
        /// El mapeo de estados de esta instalacion. **Sin el no hay nada que
        /// hacer**: sin traducir, el status del worklist y el del proveedor no
        /// se pueden comparar. Por eso aca es obligatorio y no un aviso.
        #[arg(long)]
        states_map: PathBuf,
        /// Sobre que rama. Por defecto el panorama, que es el unico que tiene
        /// el inventario completo. **Se lee, no se escribe** — que sea insegura
        /// no entra en juego: este comando no toca git.
        #[arg(long = "ref", default_value = "refs/heads/insecure/all")]
        refname: String,
        #[arg(long, default_value = "https://lamansys.atlassian.net")]
        base: String,
        /// El email de la cuenta con la que se habla por REST. **No es un
        /// secreto y por eso no viaja con el token**: es un dato de la
        /// instalacion, del mismo lado que la base y el id del board. Ver
        /// ADR-0001 § 5.
        #[arg(long)]
        account: Option<String>,
        /// Mueve solo las primeras N y para. Un lote no necesita ser una
        /// transaccion sino un corte: doscientas escrituras en un board real
        /// conviene verlas a la decima.
        #[arg(long)]
        limit: Option<usize>,
        /// Imprime el plan y no escribe nada. Sobre una corrida que toca
        /// cientos de issues reales, mirar el plan antes no es una comodidad.
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
        #[arg(long, default_value = "https://lamansys.atlassian.net")]
        base: String,
        /// El email de la cuenta con la que se habla por REST. **No es un
        /// secreto y por eso no viaja con el token**: es un dato de la
        /// instalacion, del mismo lado que la base y el id del board. Ver
        /// ADR-0001 § 5.
        #[arg(long)]
        account: Option<String>,
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
        Cmd::CheckPush { provider_file, project, states_map, base, account, stdin, dry_run, refname } => {
            cmd_check_push(
                provider_file,
                project,
                states_map,
                &base,
                account.as_deref(),
                stdin,
                dry_run,
                refname,
            )
        }
        Cmd::Window { sub: WindowCmd::Open { sprint_id, from, dry_run, force } } => {
            let repo = std::env::current_dir()?;
            let (files, head) = worklist_core::window::open(&repo, &sprint_id, &from, dry_run, force)?;
            println!("secure/sprint/{sprint_id}: {} archivo(s)", files.len());
            for f in &files {
                println!("  {f}");
            }
            if !dry_run {
                println!("recortado desde {from} -> {}", short(&head));
            }
            Ok(())
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
        Cmd::AssignKeys {
            project,
            board,
            base,
            account,
            states_map,
            stdin,
            window,
            all_windows,
            dry_run,
        } => cmd_assign_keys(
            project,
            board,
            base,
            account.as_deref(),
            states_map,
            stdin,
            &window,
            all_windows,
            dry_run,
        ),
        Cmd::Bootstrap { project, board_id, refname, base, account, limit, dry_run } => {
            cmd_bootstrap(project, board_id, refname, base, account.as_deref(), limit, dry_run)
        }
        Cmd::Reconcile { project, refname, base, account, dry_run } => {
            cmd_reconcile(project, refname, &base, account.as_deref(), dry_run)
        }
        Cmd::Propagate { stdin, window, all_windows, base, dry_run } => {
            cmd_propagate(stdin, &window, all_windows, &base, dry_run)
        }
        Cmd::InstallHooks {
            repo,
            project,
            board_id,
            provider_file,
            states_map,
            base,
            account,
            force,
            dry_run,
        } => {
            cmd_install_hooks(
                &repo,
                &project,
                &board_id,
                provider_file.as_deref(),
                states_map.as_deref(),
                &base,
                account.as_deref(),
                force,
                dry_run,
            )
        }
        Cmd::PushStates { project, states_map, refname, base, account, limit, dry_run } => {
            cmd_push_states(
                project,
                &states_map,
                &refname,
                &base,
                account.as_deref(),
                limit,
                dry_run,
            )
        }
        Cmd::Absorb { refname, provider_file, project, states_map, base, account, dry_run } => {
            cmd_absorb(
                &refname,
                provider_file,
                project,
                states_map,
                &base,
                account.as_deref(),
                dry_run,
            )
        }
        Cmd::CreateOrFind { project, r#type, source, base, account, titulo, dry_run } => {
            cmd_create_or_find(project, r#type, source, &base, account.as_deref(), titulo, dry_run)
        }
    }
}

/// La conexion al proveedor real: la credencial de los tres transportes
/// verificada, y la direccion de REST.
///
/// **En un solo lugar y no en cada comando.** El chequeo de credencial ya
/// estaba centralizado; lo que se agrega es que su resultado ahora se usa —
/// antes `preflight` se llamaba por su efecto y se tiraba lo que devolvia,
/// porque los dos CLIs leen su credencial por su cuenta. REST no: la necesita
/// en la mano.
fn conectar(base: &str, account: Option<&str>) -> Result<worklist_provider::api::Api> {
    let creds = worklist_provider::port::preflight(account.unwrap_or_default())?;
    Ok(worklist_provider::api::Api::new(base, creds))
}

/// La conexion de una corrida que **puede** no hablar.
///
/// `--dry-run` no le pide nada al proveedor, asi que tampoco le pide
/// credencial a quien la corre: el plan sale del arbol. Ver `Api::mudo`.
fn conectar_salvo_en_seco(
    base: &str,
    account: Option<&str>,
    dry_run: bool,
) -> Result<worklist_provider::api::Api> {
    if dry_run {
        return Ok(worklist_provider::api::Api::mudo(base));
    }
    conectar(base, account)
}

/// Cierra las divergencias de estado que un push ya no puede alcanzar.
///
/// **Lee una vez y escribe lo justo.** El plan sale de una sola llamada al
/// proveedor para las N claves, asi que planificar sobre el panorama entero no
/// cuesta una lectura por item — lo que cuesta por item es moverlo, y por eso
/// esta `--limit`.
fn cmd_push_states(
    project: String,
    states_map: &std::path::Path,
    refname: &str,
    base: &str,
    account: Option<&str>,
    limit: Option<usize>,
    dry_run: bool,
) -> Result<()> {
    let repo = std::env::current_dir()?;
    let vocabulario = worklist_provider::states::vocabulario(states_del_panorama(&repo).as_deref());
    let estados = worklist_provider::states::Estados::new(
        &vocabulario,
        worklist_provider::states::mapeo_de_archivo(states_map)?,
    )?;

    // La lectura si necesita credencial aunque sea `--dry-run`: el plan sale de
    // comparar contra el proveedor, asi que sin hablar no hay plan. Es lo que
    // lo distingue de las corridas en seco que planifican sobre el arbol solo.
    let api = conectar(base, account)?;
    let provider = worklist_provider::provider::JiraProvider::new(project.clone(), api);
    let plan = worklist_provider::push_states::divergences(&repo, refname, &provider, &estados)?;

    // Antes del plan, porque cambia como se lee: un total que no cuadra con el
    // arbol se explica aca y no en la cabeza del que mira.
    if !plan.unseen.is_empty() {
        println!(
            "aviso: {} clave(s) del arbol que el proveedor no informo — no se mueven: {}",
            plan.unseen.len(),
            plan.unseen.join(", ")
        );
    }
    let todas = plan.divergences;

    if todas.is_empty() {
        println!("{refname}: no hay ningun estado que difiera");
        return Ok(());
    }
    let corte = limit.unwrap_or(todas.len()).min(todas.len());
    let lote = &todas[..corte];
    println!("{refname}: {} divergen, {} en este lote", todas.len(), lote.len());
    for d in lote {
        let live = d.live.as_deref().unwrap_or("?");
        println!("  {} {} -> {} (local: {})", d.key, live, d.target, d.local);
    }
    if dry_run {
        return Ok(());
    }

    let board = JiraBoard::new(project, conectar(base, account)?);
    let hechos = worklist_provider::push_states::push(&board, &estados, lote)?;
    let mut movidos = 0usize;
    for m in &hechos {
        match &m.outcome {
            worklist_provider::board::Transicion::Hecha => {
                movidos += 1;
                println!("  movido: {} -> {}", m.key, m.target);
            }
            worklist_provider::board::Transicion::YaEstaba => {}
            worklist_provider::board::Transicion::Rechazada { motivo, disponibles } => {
                println!("  NO se movio: {} -> {} — {motivo}", m.key, m.target);
                if disponibles.is_empty() {
                    println!("          el workflow no ofrece ninguna");
                } else {
                    println!("          disponibles: {}", disponibles.join(", "));
                }
            }
        }
    }
    println!("movidos {movidos} de {}, quedan {}", lote.len(), todas.len() - movidos);
    Ok(())
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
    base: &str,
    account: Option<&str>,
    titulo: String,
    dry_run: bool,
) -> Result<()> {
    let description = format!("Fuente: {source}");
    if dry_run {
        println!("{}", dry_run_plan(&project, &item_type, &titulo, &description)?);
        return Ok(());
    }
    let board = JiraBoard::new(project, conectar(base, account)?);
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
    account: Option<&str>,
    states_map: Option<PathBuf>,
    stdin: bool,
    windows: &[String],
    all_windows: bool,
    dry_run: bool,
) -> Result<()> {
    // Antes de mirar el arbol: sin credencial, la mitad de abajo de la tabla
    // del puerto no existe, y enterarse con una ventana a medio resolver es la
    // peor forma. `--dry-run` no habla con nadie, asi que no la pide.
    let repo = std::env::current_dir()?;
    let board = JiraBoard::new(project, conectar_salvo_en_seco(&base, account, dry_run)?);

    let lines = if all_windows || !windows.is_empty() {
        window_lines(&repo, windows, all_windows)?
    } else {
        read_hook_lines(&repo, stdin)?
    };
    // Una vez, antes del lote: que falte el panorama es del repo, no de cada
    // ventana. Ver la task `77`.
    // El mapeo de estados, si esta. Sin el, la pasada de estados no corre —
    // ver el flag. El vocabulario sale del panorama, como en `check-push`.
    let estados = match &states_map {
        Some(f) => {
            let vocabulario =
                worklist_provider::states::vocabulario(states_del_panorama(&repo).as_deref());
            Some(worklist_provider::states::Estados::new(
                &vocabulario,
                worklist_provider::states::mapeo_de_archivo(f)?,
            )?)
        }
        None => {
            println!("aviso: sin --states-map los estados no viajan al proveedor");
            None
        }
    };
    let hay_panorama = worklist_provider::propagate::has_panorama(&repo);
    if !hay_panorama {
        println!(
            "aviso: este repo no tiene {} — lo que se resuelva no sube a ningun lado",
            worklist_core::git::PANORAMA
        );
    }
    for (old, new, refname) in lines {
        if classify(&refname) != RefClass::Secure {
            continue;
        }
        let r = worklist_provider::assign::assign_window(
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
        // Los estados que este push proponia mover. Va **despues** de las
        // cinco pasadas: mover un estado no depende de ninguna, y fallar aca no
        // tiene que dejar a medias lo que si se pudo hacer.
        if let Some(estados) = &estados {
            if !dry_run {
                for m in worklist_provider::assign::apply_transitions(
                    &repo, &old, &new, &board, estados,
                )? {
                    match m.resultado {
                        worklist_provider::board::Transicion::Hecha => {
                            println!("  estado: {} -> {}", m.key, m.destino)
                        }
                        worklist_provider::board::Transicion::YaEstaba => {}
                        worklist_provider::board::Transicion::Rechazada { motivo, disponibles } => {
                            println!("  estado: {} NO se movio a {} — {motivo}", m.key, m.destino);
                            // Siempre hay lista, y puede estar vacia: listar es
                            // lo que precede al intento, asi que un rechazo que
                            // exista ya miro. "No se pudieron listar" ya no es
                            // un caso — si no se pudo, no hubo intento.
                            if disponibles.is_empty() {
                                println!("          el workflow no ofrece ninguna");
                            } else {
                                println!("          disponibles: {}", disponibles.join(", "));
                            }
                        }
                    }
                }
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
            match worklist_provider::propagate::propagate(&repo, &r.refname, &r.new_head, &base, dry_run) {
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
fn cmd_bootstrap(
    project: String,
    board_id: String,
    refname: String,
    base: String,
    account: Option<&str>,
    limit: Option<usize>,
    dry_run: bool,
) -> Result<()> {
    let repo = std::env::current_dir()?;
    let board = JiraBoard::new(project, conectar_salvo_en_seco(&base, account, dry_run)?);
    let Some(r) = worklist_provider::assign::bootstrap(&repo, &refname, &base, &board, &board_id, limit, dry_run)? else {
        println!("{refname}: no hay ningun item sin clave");
        return Ok(());
    };
    // Desde que los sprints se reconcilian siempre, "no habia nada" no se
    // puede saber antes de preguntar: se dice por lo que paso, no por lo que
    // el filtro dejo afuera. Ver `ACC-301`.
    let sprints_quietos =
        r.sprints.iter().all(|s| !s.created && s.added.is_empty());
    if !dry_run && r.assigned.is_empty() && sprints_quietos {
        println!("{}: nada que hacer — todo tiene clave y los sprints estan al dia", r.refname);
        return Ok(());
    }
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
    for sp in &r.sprints {
        // Un sprint que no movio nada no se lista: la salida es lo que cambio.
        if !dry_run && !sp.created && sp.added.is_empty() {
            continue;
        }
        let como = match (dry_run, sp.created) {
            (true, _) => String::new(),
            (false, true) => "  (creado)".into(),
            (false, false) => "  (ya existia)".into(),
        };
        // `already` es `None` en dry-run: no se le pregunto a nadie, y decir
        // cero seria afirmar sobre el board sin haberlo mirado.
        let adentro = match sp.already {
            Some(n) => format!("  ({} adentro, {n} ya estaban)", sp.added.len()),
            None => format!("  ({} items)", sp.added.len()),
        };
        println!("  sprint {} -> {}{}{}", sp.id, sp.key, como, adentro);
    }
    if !dry_run && r.new_head != r.old_head {
        println!("{}: {} -> {}", r.refname, short(&r.old_head), short(&r.new_head));
    }
    Ok(())
}

/// Adopta lo que ya existe del otro lado, sin crear nada.
fn cmd_reconcile(
    project: String,
    refname: String,
    base: &str,
    account: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let repo = std::env::current_dir()?;
    let board = JiraBoard::new(project, conectar_salvo_en_seco(base, account, dry_run)?);
    let Some(r) = worklist_provider::assign::reconcile(&repo, &refname, &board, dry_run)? else {
        println!("{refname}: no hay ningun item sin clave");
        return Ok(());
    };
    let verbo = if dry_run { "adoptaria" } else { "adopto" };
    let total = r.adopted.len() + r.missing.len();
    println!("{}: {verbo} {} de {total} item(s)", r.refname, r.adopted.len());
    for a in &r.adopted {
        let refs = match a.rewritten {
            0 => String::new(),
            n => format!("  ({n} refs reescritas)"),
        };
        println!("  {} -> {}{}", a.slug, a.key, refs);
    }
    // Se nombran uno por uno y no como un total: quien corre esto esta
    // reparando, y necesita saber cuales quedaron afuera para decidir si es lo
    // esperado o es otro problema.
    if !r.missing.is_empty() {
        println!("  y {} sin issue del otro lado:", r.missing.len());
        for m in &r.missing {
            println!("    {}  {}", m.slug, m.title);
        }
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
    if !worklist_provider::propagate::has_panorama(&repo) {
        anyhow::bail!(
            "este repo no tiene {} — no hay a donde propagar.\n\
             Corre esto parado en el repo donde vive el panorama.",
            worklist_core::git::PANORAMA
        );
    }
    for (_, new, refname) in lines {
        if classify(&refname) != RefClass::Secure {
            continue;
        }
        match worklist_provider::propagate::propagate(&repo, &refname, &new, base, dry_run)? {
            Some(p) => report_propagated(&p, dry_run),
            None => println!("{refname}: el panorama ya tiene todo lo suyo"),
        }
    }
    Ok(())
}

fn report_propagated(p: &worklist_provider::propagate::Propagated, dry_run: bool) {
    use worklist_provider::propagate::Step;
    let verbo = if dry_run { "subiria" } else { "sube" };
    println!("{}: {verbo} {} commit(s) al panorama", p.refname, p.steps.len());
    for step in &p.steps {
        match step {
            Step::Picked { sha, subject } => println!("  {} {subject}", short(sha)),
            Step::Empty { sha, subject } => {
                println!("  {} {subject}  (el panorama ya lo tenia)", short(sha))
            }
            Step::Superseded { sha, subject } => {
                println!("  {} {subject}  (el panorama ya lo dice, normalizado)", short(sha))
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

/// El `.metadata/states.yaml` del panorama, si el repo lo tiene.
///
/// **Se lee del panorama y no de la ventana que llega**: el vocabulario es del
/// proyecto entero, y un recorte lleva los items de su sprint y nada mas. Sin
/// archivo devuelve `None`, y ahi vale el vocabulario por defecto.
fn states_del_panorama(repo: &std::path::Path) -> Option<String> {
    let out = worklist_core::git_command(repo)
        .args(["show", &format!("{}:.metadata/states.yaml", worklist_core::git::PANORAMA)])
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Una diferencia encontrada, con la etiqueta que dice si costo el push.
///
/// **La misma linea en los dos casos**, porque es el mismo hallazgo: lo unico
/// que cambia es si alguien la esta usando para rechazar. Dos formatos serian
/// dos cosas que mantener de acuerdo, y la medicion existe justamente para
/// anticipar lo que el rechazo va a decir.
fn print_difference(r: &worklist_provider::check_push::RejectedKey, tag: &str, indent: &str) {
    // Un rechazo por regla se lee distinto de uno por deriva, porque lo que hay
    // que hacer es distinto: el primero no se arregla reintentando. Ver
    // `concepts/states.md`.
    if r.field == "transicion" {
        println!("{indent}{tag}: {} no se puede mover a \"{}\" — {}", r.key, r.tip, r.live);
        match &r.disponibles {
            Some(d) if !d.is_empty() => println!("{indent}        disponibles: {}", d.join(", ")),
            Some(_) => println!("{indent}        el workflow no ofrece ninguna transicion"),
            None => println!("{indent}        no se pudieron listar las disponibles"),
        }
        return;
    }
    // El cuerpo no entra en una linea: se dice **donde** empieza a diferir y se
    // muestran los dos lados de esa linea sola. Ver `commands/check-push.md`.
    if let Some(line) = r.line {
        println!("{indent}{tag}: {} cuerpo difiere — linea {line}", r.key);
        println!("{indent}        tip:       {}", r.tip);
        println!("{indent}        proveedor: {}", r.live);
        return;
    }
    println!(
        "{indent}{tag}: {} {} era \"{}\" en el tip, el proveedor dice \"{}\"",
        r.key, r.field, r.tip, r.live
    );
}

fn cmd_check_push(
    provider_file: Option<PathBuf>,
    project: Option<String>,
    states_map: Option<PathBuf>,
    base: &str,
    account: Option<&str>,
    stdin: bool,
    dry_run: bool,
    refname: Option<String>,
) -> Result<()> {
    // Decirlo en vez de ignorarlo: sin `--dry-run` el rango sale de `--stdin` o
    // de la rama actual, asi que `--ref` no tendria a que aplicarse.
    if refname.is_some() && !dry_run {
        anyhow::bail!(
            "--ref es solo de --dry-run: sin el, el rango sale de --stdin o de la rama actual"
        );
    }
    // Uno de los dos, y el de prueba solo informa el status: con el, el cuerpo
    // y el titulo no se comparan. Ver `concepts/sync.md`.
    let real = project.is_some();
    let provider: Box<dyn worklist_provider::provider::Provider> = match (provider_file, project) {
        (Some(f), None) => Box::new(FileProvider::new(f)),
        (None, Some(p)) => Box::new(worklist_provider::provider::JiraProvider::new(
            p,
            conectar(base, account)?,
        )),
        _ => anyhow::bail!("hace falta --provider-file o --project, y no los dos"),
    };
    let provider = provider.as_ref();
    let repo = std::env::current_dir()?;

    // El vocabulario sale del panorama y el mapeo de la instalacion. Ver
    // `concepts/states.md` § "El vocabulario está en git; el mapeo, en la
    // instalación".
    let vocabulario = worklist_provider::states::vocabulario(states_del_panorama(&repo).as_deref());
    let estados = match states_map {
        Some(f) => {
            worklist_provider::states::Estados::new(
                &vocabulario,
                worklist_provider::states::mapeo_de_archivo(&f)?,
            )?
        }
        // El de prueba lleva valores con la forma del worklist, asi que su
        // mapeo **es** la identidad. Con eso la comparacion siempre traduce y
        // no hay una rama sin mapeo que se comporte distinto.
        None if !real => worklist_provider::states::Estados::identidad(&vocabulario),
        None => anyhow::bail!(
            "--project necesita --states-map: el status del worklist y el del proveedor no son \
             el mismo campo, y compararlos sin traducir rechaza todas las ventanas"
        ),
    };

    // El unico momento en que el cliente y el servidor se hablan, asi que es el
    // unico lugar donde una diferencia de version se puede notar sin ir a
    // mirar. Dos binarios son dos formas de quedar viejo. Ver
    // `concepts/distribution.md` § "Dos binarios son dos formas de quedar
    // viejo".
    println!("worklist-server {}", env!("CARGO_PKG_VERSION"));

    // Comparar sin rechazar: una rama entera, ningun push, y salida cero
    // difiera lo que difiera. Lo que ese codigo informa es si la medicion se
    // pudo hacer, no su resultado. Ver `commands/check-push.md`.
    if dry_run {
        let refname = refname.unwrap_or_else(|| worklist_core::git::PANORAMA.to_string());
        let report = worklist_provider::check_push::check_ref(&repo, &refname, provider, &estados, base)?;
        println!("{refname}: {} claves comparadas", report.compared);
        // Una clave que el proveedor no informa no es una que coincida: es una
        // que no se vio. Va aparte porque lo que hay que hacer con ella es
        // averiguar por que no esta. Ver `commands/push-states.md`.
        if !report.uninformed.is_empty() {
            println!(
                "  sin informar ({}): {}",
                report.uninformed.len(),
                report.uninformed.join(", ")
            );
        }
        for r in &report.differences {
            print_difference(r, "difiere", "  ");
        }
        let cuantas = |field: &str| report.differences.iter().filter(|r| r.field == field).count();
        println!(
            "resumen: status {}, titulo {}, cuerpo {}",
            cuantas("status"),
            cuantas("titulo"),
            cuantas("cuerpo")
        );
        return Ok(());
    }

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
        let rejected = check_one(&repo, &old, &new, &refname, provider, &estados, base)?;
        for r in &rejected {
            any_rejected = true;
            print_difference(r, "reject", "");
        }
        // Y lo mismo contra el panorama: de `all` se corta todo, asi que un
        // conflicto escrito ahi entra en el proximo recorte de cada ventana.
        // Probarlo aca es lo que evita tener que anotarlo alla.
        match worklist_provider::propagate::would_conflict(&repo, &refname, &new)? {
            Verdict::Applies => {}
            // No poder probar no es que entre. Aceptar en silencio seria decir
            // que se verifico algo que nadie miro. Ver la task `77`.
            Verdict::NoPanorama => println!(
                "aviso: {refname} no se pudo probar contra el panorama — este repo no tiene {}",
                worklist_core::git::PANORAMA
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

/// Escribe los hooks del bare apuntando **al binario que los esta escribiendo**.
///
/// El path lo resuelve `current_exe` y no lo tipea nadie, que es la unica parte
/// que puede estar mal: un hook apunto una vez a un `target/debug` viejo, y eso
/// es un fix verde en los tests y ausente en produccion — los tests corren la
/// lib, no el hook. Ver `commands/install-hooks.md`.
fn cmd_install_hooks(
    repo: &std::path::Path,
    project: &str,
    board_id: &str,
    provider_file: Option<&std::path::Path>,
    states_map: Option<&std::path::Path>,
    base: &str,
    account: Option<&str>,
    force: bool,
    dry_run: bool,
) -> Result<()> {
    let exe = std::env::current_exe()
        .context("no puedo resolver mi propio path, asi que no hay que escribir en los hooks")?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let exe = exe.display();

    let hooks = repo.join("hooks");
    if !hooks.is_dir() {
        anyhow::bail!("{} no tiene hooks/: no es un bare", repo.display());
    }

    // El `status` del worklist y el del proveedor no son el mismo campo, asi
    // que contra el real hace falta el mapeo — sin el, `check-push --project`
    // se niega a arrancar en vez de rechazar todas las ventanas. Ver
    // `concepts/states.md` y `commands/install-hooks.md`.
    let mapeo = match states_map {
        Some(f) => format!(" --states-map {}", f.display()),
        None => String::new(),
    };
    // El email de la cuenta va a los dos hooks porque los dos hablan por REST:
    // el `pre-receive` para preguntar que transiciones admite el workflow, el
    // `post-receive` para moverlas. Ver ADR-0001 § 5.
    let cuenta = match account {
        Some(a) => format!(" --account {a}"),
        None => String::new(),
    };
    let contra = match provider_file {
        Some(f) => format!("--provider-file {}", f.display()),
        None => format!("--project {project}{mapeo} --base {base}{cuenta}"),
    };
    let nota = match (provider_file, states_map) {
        (Some(_), _) => concat!(
            "# Contra el proveedor de prueba: su archivo lleva valores con la\n",
            "# forma del worklist, asi que su mapeo de estados es la identidad.\n",
        ),
        (None, None) => concat!(
            "# Sin --states-map, y contra el proveedor real: check-push se va a\n",
            "# negar a arrancar. El status del worklist y el de Jira no son el\n",
            "# mismo campo, y compararlos sin traducir rechaza todas las ventanas.\n",
        ),
        (None, Some(_)) => "",
    };
    // Sin `--account` el arranque se niega y dice cual credencial falta, que es
    // lo mismo que ya pasa sin token. Se avisa igual al instalar: enterarse al
    // primer push es enterarse tarde.
    let falta_cuenta = if provider_file.is_none() && account.is_none() {
        concat!(
            "# Sin --account: la API REST autentica con el par email mas token,\n",
            "# asi que el arranque se va a negar diciendo que falta el email.\n",
        )
    } else {
        ""
    };
    let nota = format!("{nota}{falta_cuenta}");
    let pre = format!(
        "#!/bin/sh\n\
         # generado por worklist-server install-hooks — no editar\n\
         {nota}\
         exec {exe} check-push --stdin {contra}\n"
    );
    // **Sin `propagate`**: `assign-keys` ya propaga al final. Llamarlo aparte
    // correria la propagacion dos veces.
    let post = format!(
        "#!/bin/sh\n\
         # generado por worklist-server install-hooks — no editar\n\
         #\n\
         # La propagacion al panorama va adentro de assign-keys, al final.\n\
         #\n\
         # El token llega por el entorno de quien empuja: jira-cli y la API\n\
         # REST lo leen de JIRA_API_TOKEN, y un hook hereda el entorno del\n\
         # proceso que lo dispara — asi que el secreto no vive en disco, y la\n\
         # contra es que un push desde una sesion sin exportarlo falla en el\n\
         # arranque, diciendo cual credencial falta.\n\
         #\n\
         # El email si vive aca: no es secreto, y es de la instalacion.\n\
         exec {exe} assign-keys --stdin --project {project} --board {board_id} --base {base}{cuenta}{mapeo}\n"
    );

    for (name, body) in [("pre-receive", &pre), ("post-receive", &post)] {
        let path = hooks.join(name);
        if dry_run {
            println!("--- hooks/{name} ---");
            print!("{body}");
            continue;
        }
        if path.exists() && !force {
            anyhow::bail!("hooks/{name} ya existe — `--force` para sobrescribirlo");
        }
        std::fs::write(&path, body).with_context(|| format!("escribiendo hooks/{name}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        println!("hooks/{name:<12} -> {exe}");
    }
    Ok(())
}

/// Trae a la ventana lo que el proveedor dice y el tip no.
///
/// El proveedor y el mapeo se arman igual que en `check-push`, y no por
/// comodidad: **lo que absorbe tiene que ser exactamente lo que el otro iba a
/// rechazar.** Dos formas de preguntar lo mismo se separan en el primer cambio.
fn cmd_absorb(
    refname: &str,
    provider_file: Option<PathBuf>,
    project: Option<String>,
    states_map: Option<PathBuf>,
    base: &str,
    account: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let real = project.is_some();
    let provider: Box<dyn worklist_provider::provider::Provider> = match (provider_file, project) {
        (Some(f), None) => Box::new(FileProvider::new(f)),
        (None, Some(p)) => Box::new(worklist_provider::provider::JiraProvider::new(
            p,
            conectar(base, account)?,
        )),
        _ => anyhow::bail!("hace falta --provider-file o --project, y no los dos"),
    };
    let repo = std::env::current_dir()?;
    let vocabulario = worklist_provider::states::vocabulario(states_del_panorama(&repo).as_deref());
    let estados = match states_map {
        Some(f) => worklist_provider::states::Estados::new(
            &vocabulario,
            worklist_provider::states::mapeo_de_archivo(&f)?,
        )?,
        None if !real => worklist_provider::states::Estados::identidad(&vocabulario),
        None => anyhow::bail!(
            "--project necesita --states-map: sin traducir, el status del proveedor no vuelve \
             a ningun estado del vocabulario y no se puede absorber ninguno"
        ),
    };

    let out = worklist_provider::absorb::absorb(
        &repo,
        refname,
        provider.as_ref(),
        &estados,
        dry_run,
    )?;

    println!("{refname}: {} clave(s)", out.claves);
    for paso in &out.pasos {
        match paso {
            worklist_provider::absorb::Paso::Absorbido { key, campo, antes, ahora } => {
                println!("  {key}  {campo}   \"{antes}\" -> \"{ahora}\"")
            }
            worklist_provider::absorb::Paso::Reportado { key, campo, porque } => {
                println!("  {key}  {campo}   {porque}")
            }
        }
    }
    // Las dos cuentas, porque son decisiones distintas: lo absorbido ya esta, y
    // lo reportado espera a alguien.
    println!("resumen: {} absorbido(s), {} reportado(s)", out.absorbidos(), out.reportados());
    if let Some(sha) = out.commit {
        println!("absorb: {refname} <- el proveedor  ({})", &sha[..7.min(sha.len())]);
    }
    Ok(())
}
