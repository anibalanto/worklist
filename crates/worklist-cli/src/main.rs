use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::BufRead;
use std::path::PathBuf;
use std::process::Command;
use worklist::check_push::check_one;
use worklist::provider::FileProvider;

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
    /// El compare-and-swap de una ventana. Pensado para `hooks/pre-receive`.
    CheckPush {
        #[arg(long)]
        provider_file: PathBuf,
        /// Lee `<viejo> <nuevo> <ref>` por linea — el protocolo de pre-receive.
        #[arg(long)]
        stdin: bool,
    },
    /// Manipula el proveedor de prueba directamente, sin pasar por git.
    Provider {
        #[command(subcommand)]
        sub: ProviderCmd,
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
        Cmd::Push { rama, remote } => push(rama, remote),
        Cmd::CheckPush { provider_file, stdin } => cmd_check_push(provider_file, stdin),
        Cmd::Provider { sub: ProviderCmd::SetStatus { provider_file, clave, status } } => {
            let provider = FileProvider::new(provider_file);
            let old = provider.set_status(&clave, &status)?;
            match old {
                Some(old) => println!("{clave}: {old} -> {status}"),
                None => println!("{clave}: (nuevo) -> {status}"),
            }
            Ok(())
        }
    }
}

fn cmd_check_push(provider_file: PathBuf, stdin: bool) -> Result<()> {
    let provider = FileProvider::new(provider_file);
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
    for (old, _new, refname) in lines {
        let rejected = check_one(&repo, &old, &refname, &provider)?;
        for r in rejected {
            any_rejected = true;
            println!(
                "reject: {} status era \"{}\" en el tip, el proveedor dice \"{}\"",
                r.key, r.tip_status, r.live_status
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

