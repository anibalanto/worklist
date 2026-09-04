//! Resuelve los pedidos que un push dejó en una ventana: pide clave al
//! proveedor, renombra, reescribe referencias, y mueve la ref.
//!
//! Corre sobre un repo bare —un hook de recepción no tiene working tree—, así
//! que el trabajo se hace en un worktree temporal en `--detach`.

use crate::board::Board;
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
    /// El issue ya existia y **su epica estaba mal, asi que se corrigio** en un
    /// segundo paso: `acli` acepta `--parent` al crear y no al editar, y ahi
    /// entra el otro transporte.
    ///
    /// `false` cubre los dos casos buenos —no habia padre que poner, o ya
    /// estaba puesto— y ninguno de los dos es noticia.
    pub parent_fixed: bool,
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
    /// Las claves que este push cambio y que ya estaban resueltas: su cuerpo y
    /// su titulo se actualizaron en el proveedor.
    pub updated: Vec<String>,
    /// Que paso con el sprint de la ventana. `None` si la rama no lleva
    /// ninguno — una ventana siempre lleva el suyo, pero una rama segura que
    /// no sea una ventana puede no tenerlo.
    pub sprint: Option<SprintResult>,
    pub old_head: String,
    pub new_head: String,
}

/// Lo que la pasada 5 hizo con el sprint de la ventana.
#[derive(Debug)]
pub struct SprintResult {
    /// El numero del worklist — el de `_sprints/17.sprint.md`.
    pub id: String,
    /// Su id en el proveedor.
    pub key: String,
    /// Si no existia del otro lado y esta corrida lo creo.
    pub created: bool,
    /// Las claves que entraron ahora.
    pub added: Vec<String>,
    /// Cuantas ya estaban adentro. **Volver a correrlo es esto y nada mas.**
    pub already: usize,
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

/// Las claves de los items que **este push** toco y que ya estaban resueltos.
///
/// Que cambio lo dice el push, no el proveedor: el diff entre el tip anterior y
/// el que llega nombra los archivos, y de ahi salen las claves. Asi un push que
/// no toca un item no lo re-sube — actualizar uno no puede costar ochenta
/// llamadas. Ver `concepts/sync.md`.
///
/// Con `old` en ceros —una rama nueva— no hay nada que actualizar: todo lo que
/// trae es un pedido o ya viene resuelto de otra ventana.
pub fn changed_keys(repo: &Path, old: &str, new_rev: &str) -> Result<Vec<String>> {
    if old == crate::check_push::ALL_ZEROS || new_rev == crate::check_push::ALL_ZEROS {
        return Ok(Vec::new());
    }
    let listing = git_output(repo, &["diff", "--name-only", old, new_rev])?;
    let mut out = Vec::new();
    for name in listing.lines() {
        if !name.ends_with(".md") || name.contains('/') {
            continue;
        }
        if let Some(key) = crate::provider::key_of_filename(name) {
            if !out.contains(&key) {
                out.push(key);
            }
        }
    }
    out.sort();
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
    old: &str,
    new_rev: &str,
    base: &str,
    board: &dyn Board,
    board_id: &str,
    dry_run: bool,
) -> Result<Option<WindowResult>> {
    // Un borrado de rama llega con `new` en ceros: no hay arbol que resolver, y
    // borrar los issues no es de este comando. Ver la task `5j`.
    if new_rev == crate::check_push::ALL_ZEROS {
        return Ok(None);
    }
    let raw = pending_requests(repo, new_rev)?;
    let changed = changed_keys(repo, old, new_rev)?;
    let sprint_file = sprint_file(repo, new_rev)?;
    // **Que no haya nada que asignar no es que no haya nada que hacer.** Una
    // ventana ya resuelta no trae pedidos ni cambios, y es justo la que tiene
    // sus issues creados y su sprint sin existir del otro lado. La pasada 5
    // reconcilia, asi que corre igual. Ver `concepts/sync.md` seccion "Corre
    // aunque no haya nada que asignar".
    if raw.is_empty() && changed.is_empty() && sprint_file.is_none() {
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
                    parent_fixed: false,
                });
                continue;
            }
            let outcome =
                board.create_or_find(&title, item_type, &title, parent_key.as_deref())?;
            let key = outcome.key().to_string();

            // El `--parent` solo viaja en la creacion. Sobre un issue que ya
            // existia hay que ponerlo aparte, y **ponerlo** y no avisar: el
            // aviso decia "no quedo bajo X" sin haber mirado el board, y una
            // corrida caida a la mitad —que crea con padre y falla despues—
            // basta para que eso sea falso sobre 22 issues a la vez.
            let parent_fixed = match parent_key.as_deref() {
                Some(epic) if outcome.needs_parent_apart(parent_key.as_deref()) => {
                    board.set_parent(&key, epic)?
                }
                _ => false,
            };

            let touched = crate::rename_one(&tmp, slug, &key)?;
            assigned.push(Assigned {
                slug: slug.clone(),
                key,
                rewritten: touched.len(),
                normalized: false,
                parent_fixed,
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
                board.set_description(&a.key, &adf)?;
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
                    if board.link_blocks(&dep, &a.key)? {
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
                        if board.link_relates(dk, &a.key)? {
                            related.push((dk.clone(), a.key.clone()));
                        }
                    }
                }
            }
        }

        // ── Pasada 4: lo que este push cambio y ya tenia clave.
        //
        // Los pedidos se crean; esto se actualiza. Sin esta pasada, editar un
        // item resuelto y empujar dejaba a git y al proveedor divergiendo, con
        // el hook informando "sin pedidos". Ver `concepts/sync.md`.
        let mut updated = Vec::new();
        if !dry_run {
            // Los recien creados ya subieron su cuerpo en la pasada 2.
            let recien: Vec<&str> = assigned.iter().map(|a| a.key.as_str()).collect();
            for key in &changed {
                if recien.contains(&key.as_str()) {
                    continue;
                }
                let Ok((path, _)) = crate::find_file(&tmp, key) else {
                    // Se borro en este mismo push: no hay cuerpo que subir, y
                    // borrar el issue no es de este comando.
                    continue;
                };
                let text = std::fs::read_to_string(&path)?;
                let (adf, canonical) = crate::body::round_trip(&text, base, &tmp)?;
                if canonical != text {
                    std::fs::write(&path, &canonical)?;
                    crate::commit_all(&tmp, &format!("normalize: {key}"))?;
                }
                board.set_description(key, &adf)?;
                if let Some(title) = title_of(&canonical) {
                    board.set_summary(key, &title)?;
                }
                updated.push(key.clone());
            }
        }

        // ── Pasada 5: el sprint, con la ventana entera ya resuelta.
        //
        // Va al final porque la membresia se lee del `items` del `.sprint.md`,
        // y ahi los ids son slugs hasta que la pasada 1 los reescribe.
        let sprint = match &sprint_file {
            None => None,
            Some((id, file)) => {
                let path = tmp.join(file);
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("leyendo {file}"))?;
                let members = sprint_members(&tmp, &sprint_declared(&text))?;
                if dry_run {
                    Some(SprintResult {
                        id: id.clone(),
                        key: sprint_key(&text).unwrap_or_else(|| "(dry-run)".into()),
                        created: false,
                        added: members,
                        already: 0,
                    })
                } else {
                    // El nombre del otro lado lo escribe el worklist, con la
                    // regla de siempre: nunca el id solo. Y no es la llave —
                    // esa es el `key`— asi que cambiarlo no rompe nada.
                    let nombre = match title_of(&text) {
                        Some(titulo) => format!("{id} {titulo}"),
                        None => id.clone(),
                    };
                    let (key, created) = match sprint_key(&text) {
                        Some(k) => (k, false),
                        None => {
                            let (k, created) = board.create_or_find_sprint(board_id, &nombre)?;
                            std::fs::write(&path, with_sprint_key(&text, &k)?)?;
                            crate::commit_all(&tmp, &format!("sprint: {id} -> {k}"))?;
                            (k, created)
                        }
                    };
                    // Se lee antes para mandar solo lo que falta, no para
                    // verificar lo que se mando: el codigo de salida de
                    // `jira-cli` es fiel. El efecto es que volver a correrlo
                    // cuesta una lectura y cero escrituras.
                    let adentro = board.sprint_items(board_id, &key)?;
                    let faltan: Vec<&str> = members
                        .iter()
                        .filter(|m| !adentro.contains(m))
                        .map(|m| m.as_str())
                        .collect();
                    if !faltan.is_empty() {
                        board.add_to_sprint(&key, &faltan)?;
                    }
                    Some(SprintResult {
                        id: id.clone(),
                        key,
                        created,
                        added: faltan.iter().map(|s| s.to_string()).collect(),
                        already: members.len() - faltan.len(),
                    })
                }
            }
        };

        let new_head = git_output(&tmp, &["rev-parse", "HEAD"])?.trim().to_string();
        Ok(WindowResult {
            refname: refname.to_string(),
            order,
            assigned,
            linked,
            related,
            untranslated,
            updated,
            sprint,
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

/// El `.sprint.md` que el arbol de `rev` lleva, y el numero de sprint que lo
/// nombra. Una ventana lleva exactamente uno.
///
/// **No es un pedido y nunca lo fue**: `split_item_name` descarta cualquier
/// stem con `/`, asi que `_sprints/17.sprint.md` no entra a la pasada 1. Un
/// sprint del proveedor no es un issue. Ver `concepts/sync.md` seccion "El
/// sprint viaja como sprint, no como issue".
pub fn sprint_file(repo: &Path, rev: &str) -> Result<Option<(String, String)>> {
    let listing = git_output(repo, &["ls-tree", "-r", "--name-only", rev])?;
    let mut found: Vec<(String, String)> = listing
        .lines()
        .filter_map(|name| {
            let id = name.strip_prefix("_sprints/")?.strip_suffix(".sprint.md")?;
            (!id.contains('/')).then(|| (id.to_string(), name.to_string()))
        })
        .collect();
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found.remove(0))),
        n => bail!(
            "{rev} lleva {n} sprints y una ventana lleva uno: {}",
            found.iter().map(|(_, f)| f.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// El `key` del frontmatter del sprint: su id en el proveedor. **Su ausencia
/// es que el sprint todavia no existe del otro lado**, igual que un archivo de
/// item que todavia lleva slug.
fn sprint_key(text: &str) -> Option<String> {
    let end = text.find("\n---\n")?;
    let re = regex::Regex::new(r"(?m)^key:\s*(\S+)$").unwrap();
    re.captures(&text[..end]).map(|c| c[1].to_string())
}

/// Anota el `key` en el frontmatter, despues de `items` si esta y al final si
/// no. Es el unico campo del sprint que escribe el servidor.
fn with_sprint_key(text: &str, key: &str) -> Result<String> {
    let Some(end) = text.find("\n---\n") else {
        bail!("el sprint no tiene frontmatter donde anotar su clave");
    };
    let (fm, rest) = text.split_at(end);
    let re = regex::Regex::new(r"(?m)^items:.*$").unwrap();
    let fm = match re.find(fm) {
        Some(m) => format!("{}\nkey: {key}{}", &fm[..m.end()], &fm[m.end()..]),
        None => format!("{fm}\nkey: {key}"),
    };
    Ok(format!("{fm}{rest}"))
}

/// Los ids que el `items` del sprint declara.
fn sprint_declared(text: &str) -> Vec<String> {
    let re = regex::Regex::new(r"(?m)^items:\s*\[([^\]]*)\]").unwrap();
    let id = regex::Regex::new(r"[A-Za-z0-9_-]+").unwrap();
    re.captures(text)
        .map(|c| id.find_iter(&c[1]).map(|m| m.as_str().to_string()).collect())
        .unwrap_or_default()
}

/// Los miembros del sprint: la clausura de `items` sobre `parent`.
///
/// **`items` nombra los topes, no los miembros.** La regla del ancestro dice
/// que un item entra con su subarbol entero, asi que una user story esta en la
/// lista y sus tasks no. Y los ancestros que la ventana trae de solo lectura
/// —la epica, para que la cadena `parent` cierre adentro del recorte— **no
/// son miembros**: estan en el arbol porque el recorte los necesita, no porque
/// sean de la iteracion.
///
/// La distincion no es por tipo. Si un dia un sprint nombrara una epica en su
/// `items`, entraria con su subarbol y esto no cambiaria.
fn sprint_members(dir: &Path, declared: &[String]) -> Result<Vec<String>> {
    let mut parents: HashMap<String, String> = HashMap::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let Some(id) = crate::TYPES
            .iter()
            .filter(|t| **t != "sprint")
            .find_map(|t| name.strip_suffix(&format!(".{t}.md")))
        else {
            continue;
        };
        if let Some(p) = parent_of(&std::fs::read_to_string(&path)?) {
            parents.insert(id.to_string(), p);
        }
    }
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, p) in &parents {
        children.entry(p.as_str()).or_default().push(id.as_str());
    }

    let mut members: Vec<String> = Vec::new();
    let mut pending: Vec<String> = declared.to_vec();
    while let Some(id) = pending.pop() {
        if members.contains(&id) {
            continue;
        }
        for c in children.get(id.as_str()).into_iter().flatten() {
            pending.push(c.to_string());
        }
        members.push(id);
    }
    members.sort();
    Ok(members)
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

pub(crate) fn title_of(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?m)^title:\s*(.+)$").unwrap();
    let raw = re.captures(text)?[1].trim().to_string();
    // El frontmatter puede citar el titulo si lleva `:` u otros caracteres.
    Some(raw.trim_matches(|c| c == '\'' || c == '"').to_string())
}
