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
//! Lo que falta ya esta decidido y sin implementar: `sync`, `view add`, `new`,
//! `status`, `is-secure`.

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
    /// Recorta una ventana: la rama con los items de un sprint y nada mas.
    Window {
        #[command(subcommand)]
        sub: WindowCmd,
    },
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
}

#[derive(Subcommand)]
enum StateCmd {
    /// Propone mover el item a un estado. **Escribe, no consuma**: quien decide
    /// si la transicion es legal es el proveedor, cuando el push llega.
    Change { id: String, estado: String },
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
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
    }
}

/// El vocabulario que el proyecto declara, o el que worklist trae.
fn vocabulario(repo: &std::path::Path) -> Vec<String> {
    worklist_core::states::vocabulario(worklist_core::states::del_panorama(repo).as_deref())
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

fn short(sha: &str) -> &str {
    &sha[..7.min(sha.len())]
}
