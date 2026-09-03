//! El recorte de una ventana: qué archivos lleva, y producir la rama.
//!
//! Una rama segura se hace responsable de poder verificarse entera contra el
//! proveedor, y **por eso** no puede tener todos los items: la responsabilidad
//! es lo que la acota. Ver `concepts/sync.md`.

use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

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

/// Un item del arbol: su id, su archivo, y de quien cuelga.
struct Item {
    file: String,
    parent: Option<String>,
}

fn read_items(repo: &Path, rev: &str) -> Result<HashMap<String, Item>> {
    let listing = git_output(repo, &["ls-tree", "-r", "--name-only", rev])?;
    let mut items = HashMap::new();
    for name in listing.lines() {
        if name.starts_with("_sprints/") || !name.ends_with(".md") {
            continue;
        }
        let Some(id) = crate::TYPES
            .iter()
            .find_map(|t| name.strip_suffix(&format!(".{t}.md")))
        else {
            continue;
        };
        if id.contains('/') {
            continue;
        }
        let text = git_output(repo, &["show", &format!("{rev}:{name}")])?;
        let parent = regex::Regex::new(r"(?m)^parent:\s*(\S+)")
            .unwrap()
            .captures(&text)
            .map(|c| c[1].to_string());
        items.insert(id.to_string(), Item { file: name.to_string(), parent });
    }
    Ok(items)
}

/// Los ids que el `items` del frontmatter del sprint declara.
fn sprint_items(repo: &Path, rev: &str, sprint_file: &str) -> Result<Vec<String>> {
    let text = git_output(repo, &["show", &format!("{rev}:{sprint_file}")])
        .with_context(|| format!("el sprint {sprint_file} no existe en {rev}"))?;
    let re = regex::Regex::new(r"(?m)^items:\s*\[([^\]]*)\]").unwrap();
    let Some(c) = re.captures(&text) else {
        bail!("{sprint_file} no declara `items` en su frontmatter");
    };
    Ok(regex::Regex::new(r"[A-Za-z0-9_-]+")
        .unwrap()
        .find_iter(&c[1])
        .map(|m| m.as_str().to_string())
        .collect())
}

/// Los archivos que lleva la ventana del sprint `sprint_id` en `rev`.
///
/// El `.sprint.md`, los items que declara **con todo su subarbol**, y los
/// ancestros de cada uno — en la practica la epica, que viaja de solo lectura
/// para que la cadena `parent` cierre adentro.
pub fn window_files(repo: &Path, rev: &str, sprint_id: &str) -> Result<Vec<String>> {
    let sprint_file = format!("_sprints/{sprint_id}.sprint.md");
    let declared = sprint_items(repo, rev, &sprint_file)?;
    let items = read_items(repo, rev)?;

    // hijos: se calculan, no se mantienen
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, it) in &items {
        if let Some(p) = &it.parent {
            children.entry(p.as_str()).or_default().push(id.as_str());
        }
    }

    let mut keep: HashSet<String> = HashSet::new();
    let mut pending: Vec<String> = Vec::new();

    for id in &declared {
        if !items.contains_key(id) {
            bail!("{sprint_file} nombra a `{id}`, que no esta en {rev}");
        }
        pending.push(id.clone());
    }

    // subarbol: los hijos van con el padre
    while let Some(id) = pending.pop() {
        if !keep.insert(id.clone()) {
            continue;
        }
        for c in children.get(id.as_str()).into_iter().flatten() {
            pending.push(c.to_string());
        }
    }

    // ancestros, de solo lectura: la cadena `parent` tiene que cerrar
    let members: Vec<String> = keep.iter().cloned().collect();
    for id in members {
        let mut cur = items[&id].parent.clone();
        while let Some(p) = cur {
            if !items.contains_key(&p) || !keep.insert(p.clone()) {
                break;
            }
            cur = items[&p].parent.clone();
        }
    }

    let mut files: Vec<String> = keep.iter().map(|id| items[id].file.clone()).collect();
    files.push(sprint_file);
    files.sort();
    Ok(files)
}

/// Produce la rama de la ventana con esos archivos y nada mas.
pub fn open(repo: &Path, sprint_id: &str, from: &str, dry_run: bool) -> Result<(Vec<String>, String)> {
    let files = window_files(repo, from, sprint_id)?;
    if dry_run {
        return Ok((files, String::new()));
    }

    let branch = format!("refs/heads/secure/sprint/{sprint_id}");
    let tmp = repo.join(format!("../.worklist-window-{sprint_id}"));
    let _ = std::fs::remove_dir_all(&tmp);
    git_output(repo, &["worktree", "add", "--detach", "-q", tmp.to_str().unwrap(), from])?;

    let result = (|| -> Result<String> {
        let keep: HashSet<&str> = files.iter().map(|s| s.as_str()).collect();
        let listing = git_output(&tmp, &["ls-files"])?;
        for name in listing.lines() {
            if !keep.contains(name) {
                git_output(&tmp, &["rm", "-q", "--cached", name])?;
                let _ = std::fs::remove_file(tmp.join(name));
            }
        }
        crate::commit_all(&tmp, &format!("window: sprint/{sprint_id} recortado desde {from}"))?;
        Ok(git_output(&tmp, &["rev-parse", "HEAD"])?.trim().to_string())
    })();

    let _ = git_output(repo, &["worktree", "remove", "--force", tmp.to_str().unwrap()]);
    let head = result?;
    git_output(repo, &["update-ref", &branch, &head])?;
    Ok((files, head))
}
