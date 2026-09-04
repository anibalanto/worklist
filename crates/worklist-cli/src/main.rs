use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::BufRead;
use std::path::PathBuf;
use std::process::Command;
use worklist::check_push::{check_one, classify, RefClass};
use worklist::creator::{dry_run_plan, AcliCreator, Creator};
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
        #[arg(long)]
        provider_file: PathBuf,
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
        Cmd::AssignKeys { project, base, stdin, dry_run } => {
            cmd_assign_keys(project, base, stdin, dry_run)
        }
        Cmd::CreateOrFind { project, r#type, source, titulo, dry_run } => {
            let description = format!("Fuente: {source}");
            if dry_run {
                println!("{}", dry_run_plan(&project, &r#type, &titulo, &description)?);
                return Ok(());
            }
            let creator = AcliCreator::new(project);
            // Sin `--parent`: este comando resuelve un item suelto, y la
            // jerarquia la calcula `assign-keys` sobre la ventana entera.
            let outcome = creator.create_or_find(&titulo, &r#type, &description, None)?;
            println!("{}", outcome.key());
            Ok(())
        }
    }
}

fn cmd_assign_keys(project: String, base: String, stdin: bool, dry_run: bool) -> Result<()> {
    let repo = std::env::current_dir()?;
    let creator = AcliCreator::new(project);

    let lines = read_hook_lines(stdin)?;
    for (old, new, refname) in lines {
        if classify(&refname) != RefClass::Secure {
            continue;
        }
        let r =
            worklist::assign::assign_window(&repo, &refname, &old, &new, &base, &creator, dry_run)?;
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
            // El padre solo se puede poner al crear: sobre un issue que ya
            // existia, la jerarquia pedida no se aplico y hay que decirlo.
            if a.parent_missed {
                println!(
                    "  ! {}: ya existia, asi que NO quedo bajo {} — acli no acepta --parent al editar",
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
        if classify(&refname) == RefClass::Insecure {
            any_rejected = true;
            println!(
                "reject: {refname} es una rama insegura — no se puede verificar, asi que no acepta escrituras."
            );
            println!("        Recorta una ventana segura (secure/…) y empuja ahi.");
            continue;
        }
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


