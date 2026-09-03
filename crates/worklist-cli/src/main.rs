use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::process::Command;

#[derive(Parser)]
#[command(name = "worklist")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Empuja la rama actual (o la que se indique) a la ventana del proveedor.
    Push {
        rama: Option<String>,
        #[arg(long, default_value = "origin")]
        remote: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Push { rama, remote } => push(rama, remote),
    }
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

fn push(rama: Option<String>, remote: String) -> Result<()> {
    let branch = match rama {
        Some(b) => b,
        None => current_branch()?,
    };
    let local_before = rev_parse("HEAD")?;

    let status = Command::new("git")
        .args(["push", &remote, &branch])
        .status()
        .context("git push")?;

    if !status.success() {
        eprintln!("push rechazado por {remote} — el proveedor se movio. Hace `git pull --rebase` y volve a empujar.");
        std::process::exit(1);
    }

    Command::new("git")
        .args(["fetch", "-q", &remote, &branch])
        .status()
        .context("git fetch")?;
    let remote_ref = format!("{remote}/{branch}");
    let remote_after = rev_parse(&remote_ref)?;

    println!("pushed: {branch} -> {remote}/{branch}  ({})", &local_before[..7.min(local_before.len())]);

    if remote_after == local_before {
        println!("server: sin cambios");
        return Ok(());
    }

    // El servidor avanzo: post-receive corrio. Listamos los renombres mirando
    // los commits nuevos del lado del remoto, buscando el patron que
    // `worklist::rename_one` deja en el mensaje: "rename X -> Y".
    let log = Command::new("git")
        .args([
            "log",
            "--format=%s",
            &format!("{local_before}..{remote_after}"),
        ])
        .output()?;
    let msgs = String::from_utf8(log.stdout)?;
    let renames: Vec<&str> = msgs.lines().filter(|l| l.starts_with("rename ")).collect();

    if renames.is_empty() {
        println!("server: avanzo, sin renombres detectados");
    } else {
        println!("server: {} item(s) renombrado(s)", renames.len());
        for r in renames {
            println!("  {r}");
        }
    }
    println!("local branch is behind — run `git pull --rebase` to see the new ids");
    Ok(())
}

