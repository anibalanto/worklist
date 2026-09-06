//! El binario del cliente: lo que se instala en la maquina de quien trabaja.
//!
//! **No enlaza `worklist-provider`**, asi que no tiene con que escribir en el
//! proveedor aunque quien lo corra tenga la credencial. Eso no es una regla que
//! alguien respete: es que el codigo no esta. Ver `concepts/distribution.md`.
//!
//! Hoy tiene un solo subcomando, y no es un accidente del recorte — es donde
//! esta el proyecto. Lo que va a llenarlo ya esta decidido y sin implementar:
//! `sync`, `view add`, `new`, `status`, `is-secure`.

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
    }
}

fn short(sha: &str) -> &str {
    &sha[..7.min(sha.len())]
}
