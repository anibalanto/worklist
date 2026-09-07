//! El binario del cliente: lo que se instala en la maquina de quien trabaja.
//!
//! **No enlaza `worklist-provider`**, asi que no tiene con que escribir en el
//! proveedor aunque quien lo corra tenga la credencial. Eso no es una regla que
//! alguien respete: es que el codigo no esta. Ver `concepts/distribution.md`.
//!
//! `state change` y `remove` son el caso que pone a prueba ese corte: los dos
//! terminan en una operacion del proveedor —una transicion— y aun asi ninguno
//! la ejecuta. Escriben la propuesta en la vista, y el servidor la consuma
//! cuando el push llega. **Que el efecto final sea del proveedor no hace que el
//! comando lo sea.**
//!
//! **`window open` no esta aca, y estuvo.** Se fue al servidor cuando el
//! panorama dejo de estar de este lado: sin `insecure/all` local no hay de
//! donde cortar. Lo que le queda al cliente para tener su ventana es `git
//! fetch` y un worktree.
//!
//! **`pull` es el tercero y no escribe una propuesta**: escribe la rama local y
//! el worktree, que es la otra cosa que el cliente posee. Su paso 1 le pide el
//! corte al servidor —que es de quien es— y los pasos 2 y 3 son suyos.
//!
//! **`status` es el cuarto, y no escribe nada.** Es lo unico que hace, y por
//! eso es el unico del que se puede decir.
//!
//! **`push` es el que mas pone a prueba el corte, y lo pasa**: termina en una
//! escritura del proveedor —el hook mueve estados y crea issues— y aun asi no
//! habla con el. Lo que hace es un `git push`; el que habla esta del otro lado.
//!
//! Lo que falta ya esta decidido y sin implementar: `view add`, `new`,
//! `is-secure`.

mod pull;
mod push;
mod status;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "worklist", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// El estado de un item.
    State {
        #[command(subcommand)]
        sub: StateCmd,
    },
    /// Saca un item del arbol. **No es descartarlo**: eso es
    /// `state change <id> dropped`, que conserva el porque.
    Remove {
        id: String,
        /// Lo saca aunque tenga hijos. No borra en cascada: los hijos quedan
        /// colgando de la raiz, que el formato admite.
        #[arg(long)]
        force: bool,
    },
    /// En que estado esta la vista: al dia con el servidor, con trabajo sin
    /// empujar, sucia, y —si se pide— si coincide con el proveedor. **No
    /// escribe nada.** Ver `commands/status.md`.
    Status {
        /// Cual mirar. Por defecto, aquella en la que se esta parado.
        vista: Option<String>,
        /// Todas las vistas del clon.
        #[arg(long)]
        all: bool,
        /// Ademas, le pregunta al proveedor. **Es la cara**, y por eso se pide.
        #[arg(long)]
        verify: bool,
        /// Sale con 1 si algo necesita atencion, para poder encadenarlo. El
        /// default lo lee una persona; quien encadena lo pide.
        #[arg(long)]
        exit_code: bool,
    },
    /// Empuja la vista donde uno esta parado, dice que escribio el servidor
    /// encima, y traduce el rechazo. **No aporta garantias: aporta que el
    /// bucle sea corrible.** Ver `commands/push.md`.
    Push {
        /// Cual empujar. Por defecto, aquella en la que se esta parado.
        vista: Option<String>,
        /// Dice que empujaria, sin empujar.
        #[arg(long)]
        dry_run: bool,
    },
    /// Pone una vista al dia: trae el corte de hoy y replanta encima lo que no
    /// se empujo. Ver `commands/pull.md`.
    Pull {
        /// Que vista. Por defecto, aquella en la que se esta parado.
        vista: Option<String>,
        /// Todas las vistas del clon. **Solo bajada**: son operaciones
        /// independientes y no hay atomicidad que administrar entre ellas.
        #[arg(long)]
        all: bool,
        /// Dice que traeria y que replantaria, sin mover ninguna rama.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum StateCmd {
    /// Propone mover el item a un estado. **Escribe, no consuma**: quien decide
    /// si la transicion es legal es el proveedor, cuando el push llega.
    Change { id: String, estado: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::State { sub: StateCmd::Change { id, estado } } => {
            let repo = std::env::current_dir()?;
            let vocabulario = vocabulario(&repo);
            // Se valida el estado destino y **no la transicion**: que `done`
            // exista es del vocabulario y se puede contestar sin salir; que
            // `open -> done` sea legal es del workflow del proveedor.
            if !vocabulario.iter().any(|v| v == &estado) {
                anyhow::bail!(
                    "`{estado}` no esta en el vocabulario del proyecto: {}",
                    vocabulario.join(", ")
                );
            }
            let (path, _) = worklist_core::find_file(&repo, &id)?;
            let text = std::fs::read_to_string(&path)?;
            let (nuevo, anterior) = worklist_core::states::proponer(&text, &estado, &ahora())?;
            if anterior == estado {
                println!("{id}  ya estaba en {estado}");
                return Ok(());
            }
            std::fs::write(&path, nuevo)?;
            println!("{id}  {anterior} -> {estado}   (propuesto)");
            Ok(())
        }
        Cmd::Remove { id, force } => {
            let repo = std::env::current_dir()?;
            let (path, _) = worklist_core::find_file(&repo, &id)?;
            let hijos = worklist_core::hijos_de(&repo, &id);
            if !hijos.is_empty() && !force {
                anyhow::bail!(
                    "{id} tiene {} hijo(s) —{}— y no se borran en cascada. \
                     Con --force queda(n) colgando de la raiz.",
                    hijos.len(),
                    hijos.join(", ")
                );
            }
            std::fs::remove_file(&path)?;
            println!("removed:  {id}");
            println!("provider: {id}  -> {}   (propuesto)", worklist_core::states::DESCARTADO);
            if !hijos.is_empty() {
                println!("colgando: {}", hijos.join(", "));
            }
            println!("hint: se deshace con git revert");
            Ok(())
        }
        Cmd::Pull { vista, all, dry_run } => pull::run(vista, all, dry_run),
        Cmd::Push { vista, dry_run } => push::run(vista, dry_run),
        Cmd::Status { vista, all, verify, exit_code } => {
            status::run(vista, all, verify, exit_code)
        }
    }
}

/// El vocabulario que el proyecto declara, o el que worklist trae.
///
/// Se lee de la vista donde uno esta parado: el panorama no esta de este lado,
/// asi que el vocabulario viaja adentro del recorte. Ver `concepts/states.md`.
fn vocabulario(repo: &std::path::Path) -> Vec<String> {
    worklist_core::states::vocabulario(worklist_core::states::de_la_vista(repo).as_deref())
}

fn ahora() -> String {
    // Sin dependencia de fechas: el formato es ISO-8601 UTC y `date` lo da.
    std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
