//! Resuelve los pedidos que un push dejó en una ventana: pide clave al
//! proveedor, renombra, reescribe referencias, y mueve la ref.
//!
//! Corre sobre un repo bare —un hook de recepción no tiene working tree—, así
//! que el trabajo se hace en un worktree temporal en `--detach`.

use crate::creator::Creator;
use crate::provider::key_of_filename;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

pub struct Assigned {
    pub slug: String,
    pub key: String,
    pub rewritten: usize,
    /// El round-trip cambio el archivo: quedo un commit `normalize:` propio.
    pub normalized: bool,
}

pub struct WindowResult {
    pub refname: String,
    pub order: Vec<String>,
    pub assigned: Vec<Assigned>,
    pub old_head: String,
    pub new_head: String,
}

fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
    let out = crate::git_command(repo)
        .args(args)
        .output()
        .with_context(|| format!("corriendo git {args:?}"))?;
    if !out.status.success() {
        bail!("git {args:?} fallo: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8(out.stdout)?)
}

/// Los `*.md` del arbol de `rev` cuyo nombre no es una clave de proveedor.
pub fn pending_requests(repo: &Path, rev: &str) -> Result<Vec<String>> {
    let listing = git_output(repo, &["ls-tree", "-r", "--name-only", rev])?;
    let mut out = Vec::new();
    for name in listing.lines() {
        if !name.ends_with(".md") || key_of_filename(name).is_some() {
            continue;
        }
        // `<slug>.<tipo>.md` -> (slug, tipo). Lo que no tenga tipo conocido no
        // es un item y no se toca.
        if let Some((slug, item_type)) = split_item_name(name) {
            out.push(format!("{slug}\t{item_type}"));
        }
    }
    Ok(out)
}

fn split_item_name(name: &str) -> Option<(String, String)> {
    for t in crate::TYPES {
        if let Some(stem) = name.strip_suffix(&format!(".{t}.md")) {
            // sin directorios: los items viven en la raiz
            if stem.contains('/') {
                return None;
            }
            return Some((stem.to_string(), t.to_string()));
        }
    }
    None
}

/// Resuelve una ventana entera. Devuelve `None` si no habia pedidos.
pub fn assign_window(
    repo: &Path,
    refname: &str,
    new_rev: &str,
    creator: &dyn Creator,
    dry_run: bool,
) -> Result<Option<WindowResult>> {
    let raw = pending_requests(repo, new_rev)?;
    if raw.is_empty() {
        return Ok(None);
    }

    let slugs: Vec<String> = raw.iter().map(|s| s.split('\t').next().unwrap().to_string()).collect();
    let types: HashMap<String, String> = raw
        .iter()
        .map(|s| {
            let mut p = s.split('\t');
            (p.next().unwrap().to_string(), p.next().unwrap().to_string())
        })
        .collect();

    // Un worktree temporal en --detach: la rama con worktree asignado no
    // aceptaria el proximo push, y el hook no puede dejar eso atras.
    let tmp = tempdir_path(repo, refname);
    let _ = std::fs::remove_dir_all(&tmp);
    git_output(repo, &["worktree", "add", "--detach", "-q", tmp.to_str().unwrap(), new_rev])?;
    let result = (|| -> Result<WindowResult> {
        let order = crate::topo_order(&tmp, &slugs)?;

        let mut assigned = Vec::new();
        for slug in &order {
            let item_type = &types[slug];
            let (path, _) = crate::find_file(&tmp, slug)?;
            let text = std::fs::read_to_string(&path)?;
            let title = title_of(&text).unwrap_or_else(|| slug.clone());

            if dry_run {
                assigned.push(Assigned {
                    slug: slug.clone(),
                    key: "(dry-run)".into(),
                    rewritten: 0,
                    normalized: false,
                });
                continue;
            }
            // El cuerpo viaja convertido a ADF, y lo que se guarda es la
            // vuelta — no el markdown que llego. Ver `concepts/sync.md`.
            let (adf, canonical) = crate::body::round_trip(&text)?;
            let normalized = canonical != text;
            if normalized {
                std::fs::write(&path, &canonical)?;
                crate::commit_all(&tmp, &format!("normalize: {slug}"))?;
            }

            let key = creator.create_or_find(&title, item_type, &adf)?;
            let touched = crate::rename_one(&tmp, slug, &key)?;
            assigned.push(Assigned {
                slug: slug.clone(),
                key,
                rewritten: touched.len(),
                normalized,
            });
        }

        let new_head = git_output(&tmp, &["rev-parse", "HEAD"])?.trim().to_string();
        Ok(WindowResult {
            refname: refname.to_string(),
            order,
            assigned,
            old_head: new_rev.to_string(),
            new_head,
        })
    })();

    // El worktree se descarta pase lo que pase.
    let _ = git_output(repo, &["worktree", "remove", "--force", tmp.to_str().unwrap()]);
    let result = result?;

    if !dry_run && result.new_head != result.old_head {
        git_output(repo, &["update-ref", refname, &result.new_head])?;
    }
    Ok(Some(result))
}

fn tempdir_path(repo: &Path, refname: &str) -> std::path::PathBuf {
    let safe = refname.replace('/', "-");
    repo.join(format!("../.worklist-assign-{safe}"))
}

fn title_of(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?m)^title:\s*(.+)$").unwrap();
    let raw = re.captures(text)?[1].trim().to_string();
    // El frontmatter puede citar el titulo si lleva `:` u otros caracteres.
    Some(raw.trim_matches(|c| c == '\'' || c == '"').to_string())
}
