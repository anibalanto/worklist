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

#[derive(Debug)]
pub struct Assigned {
    pub slug: String,
    pub key: String,
    pub rewritten: usize,
    /// El round-trip cambio el archivo: quedo un commit `normalize:` propio.
    pub normalized: bool,
    /// La clave de la epica ancestro que se le pidio de `--parent`, si tenia.
    pub parent: Option<String>,
    /// Se le pidio un padre y el issue ya existia, asi que **no se aplico**:
    /// `acli` acepta `--parent` al crear y no al editar.
    pub parent_missed: bool,
}

#[derive(Debug)]
pub struct WindowResult {
    pub refname: String,
    pub order: Vec<String>,
    pub assigned: Vec<Assigned>,
    /// `(bloqueante, bloqueado)` de los vinculos creados en esta corrida.
    pub linked: Vec<(String, String)>,
    /// `(user story, task)` de los `Relates` creados: el escalon del medio del
    /// worklist, que Jira no tiene como jerarquia.
    pub related: Vec<(String, String)>,
    /// `(slug, clave)` de las dependencias que apuntan **fuera** de la ventana:
    /// no hay clave que mandarle al proveedor, asi que el vinculo no se creo.
    pub untranslated: Vec<(String, String)>,
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
    base: &str,
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

        // La jerarquia se lee una vez, antes de crear nada: el `--parent` de un
        // item es su epica ancestro, y para saberla hay que tener la cadena
        // entera. Ver `concepts/sync.md`.
        let mut parents: HashMap<String, String> = HashMap::new();
        for slug in &slugs {
            let (path, _) = crate::find_file(&tmp, slug)?;
            if let Some(p) = parent_of(&std::fs::read_to_string(&path)?) {
                parents.insert(slug.clone(), p);
            }
        }

        // ── Pasada 1: claves y renombres. Nada viaja al proveedor todavia:
        // un cuerpo enviado aca llevaria los nombres previos al renombre de
        // los demas del lote, y quedaria congelado asi.
        let mut assigned = Vec::new();
        for slug in &order {
            let item_type = &types[slug];
            let (path, _) = crate::find_file(&tmp, slug)?;
            let text = std::fs::read_to_string(&path)?;
            let title = title_of(&text).unwrap_or_else(|| slug.clone());

            // La epica ya tiene clave: el orden topologico la pone antes.
            let epic_slug = epic_ancestor(slug, &parents, &types);
            let parent_key = epic_slug.as_ref().and_then(|e| {
                assigned
                    .iter()
                    .find(|a: &&Assigned| &a.slug == e)
                    .map(|a| a.key.clone())
            });

            if dry_run {
                assigned.push(Assigned {
                    slug: slug.clone(),
                    key: "(dry-run)".into(),
                    rewritten: 0,
                    normalized: false,
                    parent: parent_key,
                    parent_missed: false,
                });
                continue;
            }
            let outcome =
                creator.create_or_find(&title, item_type, &title, parent_key.as_deref())?;
            let key = outcome.key().to_string();
            let touched = crate::rename_one(&tmp, slug, &key)?;
            assigned.push(Assigned {
                slug: slug.clone(),
                key,
                rewritten: touched.len(),
                normalized: false,
                parent_missed: outcome.parent_missed(parent_key.as_deref()),
                parent: parent_key,
            });
        }

        // ── Pasada 2: los cuerpos, ya con todos los nombres finales puestos.
        if !dry_run {
            for a in assigned.iter_mut() {
                let (path, _) = crate::find_file(&tmp, &a.key)?;
                let text = std::fs::read_to_string(&path)?;
                let (adf, canonical) = crate::body::round_trip(&text, base, &tmp)?;
                if canonical != text {
                    std::fs::write(&path, &canonical)?;
                    crate::commit_all(&tmp, &format!("normalize: {}", a.key))?;
                    a.normalized = true;
                }
                creator.set_description(&a.key, &adf)?;
            }
        }

        // ── Pasada 3: los vinculos, con las dos puntas ya existiendo.
        let mut linked = Vec::new();
        let mut related = Vec::new();
        let mut untranslated = Vec::new();
        if !dry_run {
            for a in &assigned {
                let (path, _) = crate::find_file(&tmp, &a.key)?;
                let text = std::fs::read_to_string(&path)?;
                for dep in depends_of(&text) {
                    // Los `depends` de adentro llegan traducidos: el renombre
                    // los reescribio. Un slug que sobrevive apunta afuera de la
                    // ventana, y no hay clave que mandarle al proveedor. Se
                    // informa y se sigue: exigir que toda dependencia caiga
                    // adentro seria pedirle al backlog que se ordene por el
                    // recorte. Ver `concepts/sync.md`.
                    if crate::is_unassigned(&dep) {
                        untranslated.push((dep, a.key.clone()));
                        continue;
                    }
                    if creator.link_blocks(&dep, &a.key)? {
                        linked.push((dep, a.key.clone()));
                    }
                }
                // El escalon que Jira no tiene: si el padre directo no es la
                // epica que ya viajo como `--parent`, va como `Relates`.
                let direct = parents.get(&a.slug);
                let is_epic = direct
                    .map(|d| types.get(d).map(|t| t == "epic").unwrap_or(false))
                    .unwrap_or(false);
                if let (Some(d), false) = (direct, is_epic) {
                    if let Some(dk) = assigned.iter().find(|x| &x.slug == d).map(|x| &x.key) {
                        if creator.link_relates(dk, &a.key)? {
                            related.push((dk.clone(), a.key.clone()));
                        }
                    }
                }
            }
        }

        let new_head = git_output(&tmp, &["rev-parse", "HEAD"])?.trim().to_string();
        Ok(WindowResult {
            refname: refname.to_string(),
            order,
            assigned,
            linked,
            related,
            untranslated,
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

/// Los ids que `relation.depends` declara en el frontmatter.
/// El `parent` del frontmatter, que es un slug del worklist.
fn parent_of(text: &str) -> Option<String> {
    let end = text.find("\n---\n")?;
    let re = regex::Regex::new(r"(?m)^parent:\s*(\S+)$").unwrap();
    re.captures(&text[..end]).map(|c| c[1].to_string())
}

/// La epica de la que cuelga un item, subiendo la cadena `parent` hasta el
/// primer `epic`. Es lo que Jira acepta de `--parent`: `Historia` y `Tarea`
/// estan en el mismo nivel, asi que el padre directo no siempre sirve.
///
/// Se corta si la cadena sale de la ventana o si da una vuelta: un ciclo en
/// los `parent` es un error del worklist, y colgarse no lo arregla.
fn epic_ancestor(
    slug: &str,
    parents: &HashMap<String, String>,
    types: &HashMap<String, String>,
) -> Option<String> {
    let mut seen = vec![slug.to_string()];
    let mut at = parents.get(slug)?.clone();
    loop {
        if types.get(&at).map(|t| t == "epic").unwrap_or(false) {
            return Some(at);
        }
        if seen.contains(&at) {
            return None;
        }
        seen.push(at.clone());
        at = parents.get(&at)?.clone();
    }
}

fn depends_of(text: &str) -> Vec<String> {
    let Some(end) = text.find("\n---\n") else { return Vec::new() };
    let fm = &text[..end];
    let re = regex::Regex::new(r"relation\.depends:\s*(\[[^\]]*\]|\S+)").unwrap();
    let id = regex::Regex::new(r"[A-Za-z0-9_-]+").unwrap();
    re.captures(fm)
        .map(|c| id.find_iter(&c[1]).map(|m| m.as_str().to_string()).collect())
        .unwrap_or_default()
}

fn title_of(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?m)^title:\s*(.+)$").unwrap();
    let raw = re.captures(text)?[1].trim().to_string();
    // El frontmatter puede citar el titulo si lleva `:` u otros caracteres.
    Some(raw.trim_matches(|c| c == '\'' || c == '"').to_string())
}
