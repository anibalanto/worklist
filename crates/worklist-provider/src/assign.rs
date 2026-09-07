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
    /// El issue lo creo esta corrida. `false` es *"ya existia y lo encontre"*,
    /// y la diferencia importa donde no hay compare-and-swap detras: sobre
    /// algo que ya estaba, escribir sin haber mirado es pisar.
    pub created: bool,
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
    /// Las que el proveedor **rechazo**: el worklist las tiene y el board no.
    ///
    /// No es un fracaso de la pasada — es deriva, y este es el unico lugar
    /// donde se ve. Medido el 2026-09-07: `ACC-268` estaba borrada del board y
    /// el lote es todo o nada, asi que arrastraba a otras seis en cada push.
    pub rechazadas: Vec<String>,
    /// Cuantas ya estaban adentro. **Volver a correrlo es esto y nada mas.**
    ///
    /// `None` es *"no se pregunto"* — el `--dry-run` no habla con el proveedor,
    /// y decir `0` ahi seria afirmar sobre el board sin haberlo mirado. Es el
    /// mismo defecto que el aviso de "no quedo bajo X" que `67` corrigio.
    pub already: Option<usize>,
}

fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
    let out = worklist_core::git_command(repo)
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

/// Todos los sprints del arbol: `(numero, ruta)`.
///
/// Un sprint no es un issue —se pide con `create_or_find_sprint`— pero uno sin
/// clave es lo mismo que un item sin clave: algo que el proveedor todavia no
/// nombra. Dejarlos afuera producia el agujero que el board mostro, con el
/// sprint en curso entre los que faltaban. Ver `ACC-299`.
///
/// **Y se devuelven todos, no solo los que no tienen clave.** Filtrar por eso
/// trataba al sprint como a un item: una vez que tiene identidad, listo. Pero
/// un sprint es tambien una **membresia**, y la membresia cambia mientras el
/// sprint vive — que es lo normal en el que esta en curso. La idempotencia no
/// la da este filtro sino `resolve_sprint`, que lee que hay adentro y manda
/// solo lo que falta. Ver `ACC-301`.
pub fn sprints_del_arbol(repo: &Path, rev: &str) -> Result<Vec<(String, String)>> {
    let listing = git_output(repo, &["ls-tree", "-r", "--name-only", rev])?;
    let mut out = Vec::new();
    for name in listing.lines() {
        let Some(base) = name.strip_prefix("_sprints/") else { continue };
        let Some(id) = base.strip_suffix(".sprint.md") else { continue };
        out.push((id.to_string(), name.to_string()));
    }
    out.sort_by_key(|(id, _)| id.parse::<u32>().unwrap_or(u32::MAX));
    Ok(out)
}

/// Todos los items del arbol, con su tipo — tengan clave o no.
///
/// Distinto de `pending_requests`, que lista **lo que hay que pedir**. Quien
/// tiene clave decide que se pide, no que se puede **nombrar**: una epica ya
/// resuelta sigue siendo la epica ancestro de sus tasks, y buscarla solo entre
/// los pedidos la vuelve invisible en cuanto cruzo. Ver la task `7p`.
pub fn all_items(repo: &Path, rev: &str) -> Result<Vec<(String, String)>> {
    let listing = git_output(repo, &["ls-tree", "-r", "--name-only", rev])?;
    let mut out = Vec::new();
    for name in listing.lines() {
        if !name.ends_with(".md") {
            continue;
        }
        if let Some((id, item_type)) = split_item_name(name) {
            out.push((id, item_type));
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
/// **Los borrados no entran**: un item que el push saca del arbol no se
/// actualiza, se transiciona a `dropped` — ver `dropped_keys`. Meterlo aca
/// habria hecho que el servidor le re-subiera al proveedor el contenido viejo
/// de algo que se acaba de sacar.
///
/// Con `old` en ceros —una rama nueva— no hay nada que actualizar: todo lo que
/// trae es un pedido o ya viene resuelto de otra ventana.
pub fn changed_keys(repo: &Path, old: &str, new_rev: &str) -> Result<Vec<String>> {
    keys_del_diff(repo, old, new_rev, "ACMRT")
}

/// Las claves de los items que **este push borra**.
///
/// Sacar un item del arbol es proponer su transicion a `dropped`, y eso se lee
/// del diff sin ningun campo ni lapida: el nombre del archivo borrado da la
/// clave, y que ya no este en el arbol nuevo da la intencion. Ver
/// `commands/remove.md`.
pub fn dropped_keys(repo: &Path, old: &str, new_rev: &str) -> Result<Vec<String>> {
    keys_del_diff(repo, old, new_rev, "D")
}

fn keys_del_diff(repo: &Path, old: &str, new_rev: &str, filtro: &str) -> Result<Vec<String>> {
    if old == crate::check_push::ALL_ZEROS || new_rev == crate::check_push::ALL_ZEROS {
        return Ok(Vec::new());
    }
    let filtro = format!("--diff-filter={filtro}");
    let listing = git_output(repo, &["diff", "--name-only", &filtro, old, new_rev])?;
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

/// `<id>.<tipo>.md` -> `(id, tipo)`, y `None` si el nombre no es un item.
///
/// El id se valida contra el alfabeto, que es lo que deja afuera a
/// `_sprints/20.sprint.md` —los items viven en la raiz, asi que el `/` no es de
/// un id— y a cualquier nombre que el renombre despues no podria reescribir
/// entero. Ver `concepts/item.md` § "El alfabeto de un id".
fn split_item_name(name: &str) -> Option<(String, String)> {
    for t in worklist_core::TYPES {
        if let Some(stem) = name.strip_suffix(&format!(".{t}.md")) {
            if !worklist_core::is_valid_id(stem) {
                return None;
            }
            return Some((stem.to_string(), t.to_string()));
        }
    }
    None
}

/// Lo que paso con cada estado que el push proponia mover.
pub struct EstadoMovido {
    pub key: String,
    pub destino: String,
    pub resultado: crate::board::Transicion,
}

/// Mueve los estados que este push propone, **despues** de que el push entro.
///
/// Es una pasada aparte de las cinco de `assign_window` por dos razones que se
/// suman: no necesita el worktree —lee dos arboles de git y habla con el
/// board— y **falla distinto**. Las otras pasadas escriben; esta pide, y el
/// workflow puede negarse.
///
/// La negativa normal ya se cazo en el `pre-receive`, que rechaza el push antes
/// de aceptarlo. La que llega aca es la que el workflow no admitia **al momento
/// de aplicar**: el board pudo cambiar entre un paso y el otro, y ahi el estado
/// local y el del proveedor quedan divergiendo — que es exactamente lo que el
/// compare-and-swap del proximo push caza. No se pierde, se pospone.
pub fn apply_transitions(
    repo: &Path,
    old: &str,
    new_rev: &str,
    board: &dyn Board,
    estados: &crate::states::Estados,
) -> Result<Vec<EstadoMovido>> {
    let mut out = Vec::new();
    for (key, _) in crate::check_push::transiciones_propuestas(repo, old, new_rev, estados)? {
        // El destino completo, no solo su status: la resolucion es lo que
        // distingue `dropped` de `done` del otro lado.
        let Some(destino) = destino_de(repo, old, new_rev, &key, estados) else { continue };
        let resultado = board.transition(&key, &destino)?;
        out.push(EstadoMovido {
            key,
            destino: destino.status().to_string(),
            resultado,
        });
    }
    Ok(out)
}

/// El destino de una clave: el de su `status` nuevo, o el de `dropped` si el
/// push la borro.
fn destino_de(
    repo: &Path,
    old: &str,
    new_rev: &str,
    key: &str,
    estados: &crate::states::Estados,
) -> Option<crate::states::Destino> {
    if dropped_keys(repo, old, new_rev).ok()?.iter().any(|k| k == key) {
        return estados.destino(crate::states::DESCARTADO).cloned();
    }
    let file = git_output(repo, &["ls-tree", "-r", "--name-only", new_rev])
        .ok()?
        .lines()
        .find(|n| crate::provider::key_of_filename(n).as_deref() == Some(key))?
        .to_string();
    let text = git_output(repo, &["show", &format!("{new_rev}:{file}")]).ok()?;
    estados.destino(&crate::provider::status_of(&text)?).cloned()
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
        let order = worklist_core::topo_order(&tmp, &slugs)?;

        // La jerarquia se lee una vez, antes de crear nada: el `--parent` de un
        // item es su epica ancestro, y para saberla hay que tener la cadena
        // entera. Ver `concepts/sync.md`.
        let mut parents: HashMap<String, String> = HashMap::new();
        for slug in &slugs {
            let (path, _) = worklist_core::find_file(&tmp, slug)?;
            if let Some(p) = parent_of(&std::fs::read_to_string(&path)?) {
                parents.insert(slug.clone(), p);
            }
        }

        // ── Pasada 1: claves y renombres. Nada viaja al proveedor todavia:
        // un cuerpo enviado aca llevaria los nombres previos al renombre de
        // los demas del lote, y quedaria congelado asi.
        // Sin ancla: la ref de una ventana la mueve el hook al final, y ese
        // orden es su contrato con el `pre-receive`. Lo de `7k` es del
        // panorama, donde no hay nadie esperando la respuesta.
        let mut sin_ancla = Ancla::new(&tmp, refname, "", false);
        let assigned =
            assign_and_rename(&tmp, &order, &types, &parents, board, dry_run, &mut sin_ancla)?;
        let mut assigned = assigned;

        // ── Pasada 2: los cuerpos, ya con todos los nombres finales puestos.
        if !dry_run {
            for a in assigned.iter_mut() {
                let (path, _) = worklist_core::find_file(&tmp, &a.key)?;
                let text = std::fs::read_to_string(&path)?;
                let (adf, canonical) = worklist_core::body::round_trip(&text, base, &tmp)?;
                if canonical != text {
                    std::fs::write(&path, &canonical)?;
                    worklist_core::commit_all(&tmp, &format!("normalize: {}", a.key))?;
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
                let (path, _) = worklist_core::find_file(&tmp, &a.key)?;
                let text = std::fs::read_to_string(&path)?;
                for dep in depends_of(&text) {
                    // Los `depends` de adentro llegan traducidos: el renombre
                    // los reescribio. Un slug que sobrevive apunta afuera de la
                    // ventana, y no hay clave que mandarle al proveedor. Se
                    // informa y se sigue: exigir que toda dependencia caiga
                    // adentro seria pedirle al backlog que se ordene por el
                    // recorte. Ver `concepts/sync.md`.
                    if worklist_core::is_unassigned(&dep) {
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
                let Ok((path, _)) = worklist_core::find_file(&tmp, key) else {
                    // Ya no deberia pasar: `changed_keys` deja los borrados
                    // afuera, y esos van por `apply_transitions` a `dropped`.
                    // Queda como red: leer el archivo del arbol viejo para
                    // re-subirle al proveedor el cuerpo de algo que se acaba de
                    // sacar seria peor que no hacer nada.
                    continue;
                };
                let text = std::fs::read_to_string(&path)?;
                let (adf, canonical) = worklist_core::body::round_trip(&text, base, &tmp)?;
                if canonical != text {
                    std::fs::write(&path, &canonical)?;
                    worklist_core::commit_all(&tmp, &format!("normalize: {key}"))?;
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
            Some((id, file)) => Some(resolve_sprint(&tmp, id, file, board, board_id, dry_run)?),
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
/// **No es un pedido y nunca lo fue**: el `/` no es un caracter de id, asi que
/// `split_item_name` descarta `_sprints/17.sprint.md` y no entra a la pasada 1.
/// Un sprint del proveedor no es un issue. Ver `concepts/sync.md` seccion "El
/// sprint viaja como sprint, no como issue".
/// La pasada 5 sobre **un** sprint: lo crea del otro lado si no esta, le anota
/// su `key`, y le mete adentro los issues que le falten.
///
/// Extraida porque la usan dos: la ventana, donde el sprint es el suyo, y
/// `bootstrap`, que resuelve los que ninguna ventana cubrio. Un sprint sin
/// ventana no tenia camino al proveedor —ver la task `ACC-299`— y copiar esto
/// habria sido la tercera copia de la misma pasada.
pub(crate) fn resolve_sprint(
    tmp: &Path,
    id: &str,
    file: &str,
    board: &dyn Board,
    board_id: &str,
    dry_run: bool,
) -> Result<SprintResult> {
    let path = tmp.join(file);
    let text = std::fs::read_to_string(&path).with_context(|| format!("leyendo {file}"))?;
    let members = sprint_members(tmp, &sprint_declared(&text))?;
    if dry_run {
        return Ok(SprintResult {
            id: id.to_string(),
            key: sprint_key(&text).unwrap_or_else(|| "(dry-run)".into()),
            created: false,
            added: members,
            // El `--dry-run` no habla con el proveedor, asi que no puede saber
            // cuales rechazaria. Vacio aca es "no se pregunto".
            rechazadas: Vec::new(),
            already: None,
        });
    }
    // El nombre del otro lado lo escribe el worklist, con la regla de siempre:
    // nunca el id solo. Y no es la llave —esa es el `key`— asi que cambiarlo
    // no rompe nada.
    let nombre = sprint_name(id, title_of(&text).as_deref());
    let (key, created) = match sprint_key(&text) {
        Some(k) => (k, false),
        None => {
            let (k, created) = board.create_or_find_sprint(board_id, &nombre)?;
            std::fs::write(&path, with_sprint_key(&text, &k)?)?;
            worklist_core::commit_all(tmp, &format!("sprint: {id} -> {k}"))?;
            (k, created)
        }
    };
    // Se lee antes para mandar solo lo que falta, no para verificar lo que se
    // mando: el codigo de salida de `jira-cli` es fiel. El efecto es que volver
    // a correrlo cuesta una lectura y cero escrituras.
    let adentro = board.sprint_items(board_id, &key)?;
    let faltan: Vec<&str> =
        members.iter().filter(|m| !adentro.contains(m)).map(|m| m.as_str()).collect();
    // Las que el board rechazo no frenan la pasada: son claves que el worklist
    // tiene y el proveedor no, y **esto es el unico lugar donde esa deriva se
    // ve**. Frenar aca hacia que una clave muerta bloqueara el sprint entero,
    // en cada push.
    let mut rechazadas = Vec::new();
    if !faltan.is_empty() {
        let (_, muertas) = board.add_to_sprint(&key, &faltan)?;
        rechazadas = muertas;
    }
    Ok(SprintResult {
        id: id.to_string(),
        key,
        created,
        added: faltan
            .iter()
            .filter(|k| !rechazadas.iter().any(|r| r == *k))
            .map(|s| s.to_string())
            .collect(),
        rechazadas,
        already: Some(members.len() - faltan.len()),
    })
}

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
pub(crate) fn sprint_key(text: &str) -> Option<String> {
    let end = text.find("\n---\n")?;
    let re = regex::Regex::new(r"(?m)^key:\s*(\S+)$").unwrap();
    re.captures(&text[..end]).map(|c| c[1].to_string())
}

/// Anota el `key` en el frontmatter, despues de `items` si esta y al final si
/// no. Es el unico campo del sprint que escribe el servidor.
pub(crate) fn with_sprint_key(text: &str, key: &str) -> Result<String> {
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
    let id = regex::Regex::new(r"@?[A-Za-z0-9_-]+").unwrap();
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
        let Some(id) = worklist_core::TYPES
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
    let id = regex::Regex::new(r"@?[A-Za-z0-9_-]+").unwrap();
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

/// **Pasada 1**: por cada pedido, su clave del proveedor y su renombre.
///
/// La comparten [`assign_window`] y [`bootstrap`], que son la misma operacion
/// sobre dos conjuntos distintos: los pedidos de una ventana, y los del
/// panorama entero. Ver `concepts/sync.md`.
fn assign_and_rename(
    tmp: &Path,
    order: &[String],
    types: &HashMap<String, String>,
    parents: &HashMap<String, String>,
    board: &dyn Board,
    dry_run: bool,
    ancla: &mut Ancla,
) -> Result<Vec<Assigned>> {
    let mut assigned: Vec<Assigned> = Vec::new();
    for slug in order {
        let item_type = &types[slug];
        let (path, _) = worklist_core::find_file(tmp, slug)?;
        let text = std::fs::read_to_string(&path)?;
        let title = title_of(&text).unwrap_or_else(|| slug.clone());

        // La epica ya tiene clave: el orden topologico la pone antes.
        let epic_slug = epic_ancestor(slug, parents, types);
        let parent_key = epic_slug.as_ref().and_then(|e| {
            // La epica ya cruzo: su nombre **es** la clave, y no hay nada que
            // buscar en lo asignado de esta corrida. Ver `7p`.
            if !worklist_core::is_unassigned(e) {
                return Some(e.clone());
            }
            assigned.iter().find(|a: &&Assigned| &a.slug == e).map(|a| a.key.clone())
        });

        if dry_run {
            assigned.push(Assigned {
                slug: slug.clone(),
                key: "(dry-run)".into(),
                rewritten: 0,
                normalized: false,
                created: false,
                parent: parent_key,
                parent_fixed: false,
            });
            continue;
        }
        let outcome = board.create_or_find(&title, item_type, &title, parent_key.as_deref())?;
        let key = outcome.key().to_string();
        let created = matches!(outcome, crate::board::Assignment::Created(_));

        // El `--parent` solo viaja en la creacion. Sobre un issue que ya
        // existia hay que ponerlo aparte, y **ponerlo** y no avisar: el aviso
        // decia "no quedo bajo X" sin haber mirado el board.
        let parent_fixed = match parent_key.as_deref() {
            Some(epic) if outcome.needs_parent_apart(parent_key.as_deref()) => {
                board.set_parent(&key, epic)?
            }
            _ => false,
        };

        let touched = worklist_core::rename_one(tmp, slug, &key)?;
        // La clave ya existe del otro lado: que el panorama lo sepa ahora, y
        // no dentro de ochenta items. Ver la task `7k`.
        ancla.avanzar(tmp);
        assigned.push(Assigned {
            slug: slug.clone(),
            key,
            rewritten: touched.len(),
            normalized: false,
            created,
            parent_fixed,
            parent: parent_key,
        });
    }
    Ok(assigned)
}

/// Lo que el bootstrap hizo sobre el panorama.
#[derive(Debug)]
pub struct BootstrapResult {
    pub refname: String,
    pub order: Vec<String>,
    pub assigned: Vec<Assigned>,
    /// Los sprints que no tenian `key`, resueltos al final. Ver `ACC-299`.
    pub sprints: Vec<SprintResult>,
    pub old_head: String,
    pub new_head: String,
}

/// **El bootstrap**: darle clave del proveedor a lo que no la tiene, sin
/// prometer que la rama se verifique.
///
/// Es la separacion que pide la task `5n`: *tener clave* es del item y pasa una
/// vez; *verificarse entera* es de la rama y se paga en cada push. El panorama
/// puede tener lo primero sin prometer lo segundo — que es exactamente lo que
/// "insegura" significa.
///
/// **Corre donde el panorama vive**, que hoy es un worktree del clon y manana
/// puede ser una rama del bare. Las dos formas estan cubiertas:
///
/// | | |
/// |---|---|
/// | la rama esta checkouteada **aca** | se trabaja en el arbol y se commitea, como cualquiera |
/// | no lo esta | worktree temporal en `--detach`, y `update-ref` al final |
///
/// **Lo que no se hace es mover una rama que otro worktree tiene abierta.** Es
/// el defecto de `5o`: `update-ref` la mueve igual y deja ese worktree con el
/// indice del arbol anterior, y aca serian ciento y pico de renombres. Sobre
/// eso no hay `--force` que valga.
///
/// Y no es un push: `insecure/**` rechaza escrituras del **cliente**, que es
/// otra cosa que escribir en el arbol donde uno esta parado.
///
/// De las cinco pasadas hace **solo la primera**, y una parte de la segunda:
/// el cuerpo viaja unicamente donde el issue se **creo**. Sobre uno que ya
/// existia no hay compare-and-swap detras que pruebe que partimos del estado
/// actual, y escribir sin eso es pisar lo que alguien haya editado en el board.
pub fn bootstrap(
    repo: &Path,
    refname: &str,
    base: &str,
    board: &dyn Board,
    board_id: &str,
    limit: Option<usize>,
    dry_run: bool,
) -> Result<Option<BootstrapResult>> {
    let head = git_output(repo, &["rev-parse", refname])?.trim().to_string();
    let raw = pending_requests(repo, &head)?;
    let sprints_pendientes = sprints_del_arbol(repo, &head)?;
    if raw.is_empty() && sprints_pendientes.is_empty() {
        return Ok(None);
    }

    let (aca, tmp) = worktree_para(repo, refname, &head, raw.len(), dry_run, "bootstrap")?;

    let slugs: Vec<String> =
        raw.iter().map(|s| s.split('\t').next().unwrap().to_string()).collect();
    // El tipo se conoce de **todos**, no solo de los pedidos: la epica ancestro
    // puede tener clave ya. Ver `7p`.
    let types: HashMap<String, String> = all_items(repo, &head)?.into_iter().collect();

    // La ref se mueve a medida: cada clave conseguida es un hecho consumado
    // del otro lado, y descartarla al caer solo borra la mitad local. Ver `7k`.
    let mut ancla = Ancla::new(repo, refname, &head, !aca);

    let result = (|| -> Result<(Vec<String>, Vec<Assigned>, Vec<SprintResult>, String)> {
        let mut order = worklist_core::topo_order(&tmp, &slugs)?;
        // El corte va **despues** del orden topologico, no antes: un lote puede
        // dejar una epica creada y sus tasks sin crear —estado valido, porque
        // el `--parent` se pone al crear cada task y la epica ya tiene clave—
        // y al reves no puede pasar. Ver `commands/bootstrap.md`.
        if let Some(n) = limit {
            order.truncate(n);
        }
        // La cadena de `parent` se arma sobre el arbol entero, por lo mismo.
        let mut parents: HashMap<String, String> = HashMap::new();
        for (id, _) in types.iter() {
            let Ok((path, _)) = worklist_core::find_file(&tmp, id) else { continue };
            if let Some(p) = parent_of(&std::fs::read_to_string(&path)?) {
                parents.insert(id.clone(), p);
            }
        }
        let mut assigned =
            assign_and_rename(&tmp, &order, &types, &parents, board, dry_run, &mut ancla)?;
        if !dry_run {
            for a in assigned.iter_mut().filter(|a| a.created) {
                let (path, _) = worklist_core::find_file(&tmp, &a.key)?;
                let text = std::fs::read_to_string(&path)?;
                let (adf, canonical) = worklist_core::body::round_trip(&text, base, &tmp)?;
                if canonical != text {
                    std::fs::write(&path, &canonical)?;
                    worklist_core::commit_all(&tmp, &format!("normalize: {}", a.key))?;
                    a.normalized = true;
                    ancla.avanzar(&tmp);
                }
                board.set_description(&a.key, &adf)?;
            }
        }

        // Y los sprints al final: meterles los issues adentro necesita que sus
        // items ya tengan clave. Es la misma restriccion topologica que ordena
        // la epica antes que sus tasks, un escalon mas arriba. Ver `ACC-299`.
        let mut sprints = Vec::new();
        for (id, file) in &sprints_pendientes {
            sprints.push(resolve_sprint(&tmp, id, file, board, board_id, dry_run)?);
            ancla.avanzar(&tmp);
        }

        let new_head = git_output(&tmp, &["rev-parse", "HEAD"])?.trim().to_string();
        Ok((order, assigned, sprints, new_head))
    })();

    if !aca {
        let _ = git_output(repo, &["worktree", "remove", "--force", tmp.to_str().unwrap()]);
    }
    let (order, assigned, sprints, new_head) = result?;

    // Parado en la rama, los commits ya la movieron: `update-ref` seria mover
    // dos veces la misma cosa. Y el compare-and-swap parte de donde el ancla
    // la dejo, que en una corrida entera es ya el ultimo item.
    if !aca && !dry_run {
        ancla.fijar(&new_head);
    }
    Ok(Some(BootstrapResult {
        refname: refname.to_string(),
        order,
        assigned,
        sprints,
        old_head: head,
        new_head,
    }))
}

/// Un item que se adopto: existia del otro lado y el panorama no lo sabia.
#[derive(Debug)]
pub struct Adopted {
    pub slug: String,
    pub key: String,
    pub rewritten: usize,
}

/// Un item sin contraparte. **Se nombra, no se cuenta**: quien corre esto esta
/// reparando, y necesita saber cuales quedaron afuera. Ver
/// `commands/reconcile.md`.
#[derive(Debug)]
pub struct Missing {
    pub slug: String,
    pub title: String,
}

/// Lo que la reconciliacion hizo sobre el panorama.
#[derive(Debug)]
pub struct ReconcileResult {
    pub refname: String,
    pub adopted: Vec<Adopted>,
    pub missing: Vec<Missing>,
    pub old_head: String,
    pub new_head: String,
}

/// **Reconciliar**: adoptar los issues que existen del otro lado y el panorama
/// no registro. Es la pasada 1 de `bootstrap` **sin la creacion**.
///
/// Existe separada por una propiedad y no por comodidad: `bootstrap` recupera
/// lo perdido de paso —`create_or_find` encuentra antes de crear— pero para
/// recuperar veintitres claves hay que correr el comando que ademas crea
/// ochenta y seis issues. **Reparar no deberia poder crear**, y la garantia se
/// sostiene por no haber por donde, no por un flag apagado.
///
/// Tampoco escribe nada del otro lado: ni cuerpo, ni `--parent`. Adoptar es
/// escribir la clave de este lado. Ver `commands/reconcile.md`.
pub fn reconcile(
    repo: &Path,
    refname: &str,
    board: &dyn Board,
    dry_run: bool,
) -> Result<Option<ReconcileResult>> {
    let head = git_output(repo, &["rev-parse", refname])?.trim().to_string();
    let raw = pending_requests(repo, &head)?;
    if raw.is_empty() {
        return Ok(None);
    }
    let (aca, tmp) = worktree_para(repo, refname, &head, raw.len(), dry_run, "reconcile")?;

    let slugs: Vec<String> =
        raw.iter().map(|s| s.split('\t').next().unwrap().to_string()).collect();

    let result = (|| -> Result<(Vec<Adopted>, Vec<Missing>, String)> {
        // Topologico por lo mismo que en el bootstrap: el renombre de una
        // epica reescribe lo que cuelga de ella.
        let order = worklist_core::topo_order(&tmp, &slugs)?;
        let mut adopted = Vec::new();
        let mut missing = Vec::new();
        for slug in &order {
            let (path, _) = worklist_core::find_file(&tmp, slug)?;
            let text = std::fs::read_to_string(&path)?;
            let title = title_of(&text).unwrap_or_else(|| slug.clone());
            match board.find(&title)? {
                None => missing.push(Missing { slug: slug.clone(), title }),
                Some(key) => {
                    let rewritten = if dry_run {
                        0
                    } else {
                        worklist_core::rename_one(&tmp, slug, &key)?.len()
                    };
                    adopted.push(Adopted { slug: slug.clone(), key, rewritten });
                }
            }
        }
        let new_head = git_output(&tmp, &["rev-parse", "HEAD"])?.trim().to_string();
        Ok((adopted, missing, new_head))
    })();

    if !aca {
        let _ = git_output(repo, &["worktree", "remove", "--force", tmp.to_str().unwrap()]);
    }
    let (adopted, missing, new_head) = result?;

    if !aca && !dry_run && new_head != head {
        git_output(repo, &["update-ref", refname, &new_head, &head])?;
    }
    Ok(Some(ReconcileResult {
        refname: refname.to_string(),
        adopted,
        missing,
        old_head: head,
        new_head,
    }))
}

/// La ref del panorama, movida **a medida** y no al final.
///
/// Parado en la rama no hace falta: cada commit ya la mueve. En un worktree
/// temporal la ref no se entera hasta el `update-ref`, y ahi esta el defecto
/// que la task `7k` diagnostica: una corrida que crea issues y se cae descarta
/// el arbol entero, y las claves que ya consiguio quedan **solo del otro
/// lado**.
///
/// Descartar en bloque es correcto cuando lo descartado no salio del repo —es
/// lo que hace la propagacion, y ahi esta bien—. Crear un issue es un efecto
/// afuera, irreversible y pago: la unidad de atomicidad no es la corrida, es
/// el item.
pub(crate) struct Ancla<'a> {
    repo: &'a Path,
    refname: &'a str,
    /// Donde quedo la ref la ultima vez que se movio, para el compare-and-swap
    /// del `update-ref`.
    ultimo: String,
    /// Parado en la rama los commits ya la mueven: no hay nada que anclar.
    activo: bool,
}

impl<'a> Ancla<'a> {
    pub(crate) fn new(repo: &'a Path, refname: &'a str, head: &str, activo: bool) -> Self {
        Ancla { repo, refname, ultimo: head.to_string(), activo }
    }

    /// Deja en la ref lo que el arbol temporal tiene ahora.
    ///
    /// **Que esto falle no puede tirar abajo lo que ya se hizo**, asi que no
    /// propaga: la corrida sigue y el `update-ref` final lo reintenta. Un
    /// ancla que se pierde deja el estado de antes, que es lo que habia sin
    /// ella.
    pub(crate) fn avanzar(&mut self, tmp: &Path) {
        let Ok(ahora) = git_output(tmp, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string()) else {
            return;
        };
        self.fijar(&ahora);
    }

    /// Lo mismo, con el sha ya leido — el cierre, cuando el worktree temporal
    /// ya no esta.
    pub(crate) fn fijar(&mut self, sha: &str) {
        if !self.activo || sha == self.ultimo {
            return;
        }
        if git_output(self.repo, &["update-ref", self.refname, sha, &self.ultimo]).is_ok() {
            self.ultimo = sha.to_string();
        }
    }
}

/// Donde se escribe: el arbol de uno si la rama esta checkouteada aca, y un
/// worktree temporal si no.
///
/// **Lo que no se hace es mover una rama que otro worktree tiene abierta**:
/// `update-ref` la mueve igual y le deja el indice del arbol anterior, y aca
/// son ciento y pico de renombres. Sobre eso no hay `--force` que valga.
fn worktree_para(
    repo: &Path,
    refname: &str,
    head: &str,
    renombres: usize,
    dry_run: bool,
    prefijo: &str,
) -> Result<(bool, std::path::PathBuf)> {
    // Un bare puede tener el `HEAD` apuntando a la rama y no tener donde
    // escribir: sin arbol de trabajo, "aca" no existe y va el worktree
    // temporal. Preguntarlo por el `HEAD` solo mandaba a `git status` a fallar
    // con "esta operacion debe ser realizada en un arbol de trabajo".
    let con_arbol = git_output(repo, &["rev-parse", "--is-bare-repository"])
        .map(|s| s.trim() != "true")
        .unwrap_or(true);
    let aca = con_arbol
        && git_output(repo, &["rev-parse", "--symbolic-full-name", "HEAD"])
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
            == refname;
    if !aca {
        if let Some(wt) = worklist_core::checked_out_at(repo, refname) {
            bail!(
                "{refname} esta checkouteada en otro worktree y no se puede mover:\n\
                 \x20 {wt}\n\
                 \n\
                 moverla dejaria ese worktree con el indice del arbol anterior, y aca son\n\
                 {renombres} renombres. Corre esto parado ahi, o saca el worktree primero."
            );
        }
    }
    // Trabajar en el arbol de uno exige que este limpio: `rename_one` commitea
    // con `add -A`, asi que lo que hubiera sin commitear se colaria adentro.
    if aca && !dry_run {
        let sucio = git_output(repo, &["status", "--porcelain"])?;
        if !sucio.trim().is_empty() {
            bail!(
                "el arbol tiene cambios sin commitear, y el renombre commitea con `add -A`:\n\
                 \x20 {}\n\
                 \n\
                 comitealos o descartalos antes.",
                sucio.lines().take(3).collect::<Vec<_>>().join("\n  ")
            );
        }
    }
    if aca {
        return Ok((true, repo.to_path_buf()));
    }
    let tmp = tempdir_path(repo, &format!("{prefijo}-{refname}"));
    let _ = std::fs::remove_dir_all(&tmp);
    git_output(repo, &["worktree", "add", "--detach", "-q", tmp.to_str().unwrap(), head])?;
    Ok((false, tmp))
}

/// Jira no acepta un nombre de sprint de 30 caracteres o mas.
pub const SPRINT_NAME_MAX: usize = 29;

/// El nombre del sprint del otro lado: `<numero> <titulo>`, recortado.
///
/// **Nunca el id solo**, porque el que lee es el que menos contexto tiene. Asi
/// que lo que se recorta es el **titulo**, y se marca: un titulo cortado sin
/// aviso se lee como un titulo raro.
///
/// Diez de los veintidos sprints de este repo se pasaban del limite, asi que
/// no es un caso de borde — el mas largo mide 65.
///
/// **Y es deterministico**, que es lo que lo hace seguro: mientras el
/// `.sprint.md` no tenga `key`, este nombre es con lo que se busca antes de
/// crear. Dos corridas que produjeran nombres distintos duplicarian el sprint.
/// Ver `concepts/sync.md` seccion "Se busca por nombre exactamente cuando no
/// hay `key`".
pub fn sprint_name(id: &str, title: Option<&str>) -> String {
    let Some(title) = title else { return id.to_string() };
    let entero = format!("{id} {title}");
    if entero.chars().count() <= SPRINT_NAME_MAX {
        return entero;
    }
    let recortado: String = entero.chars().take(SPRINT_NAME_MAX - 1).collect();
    format!("{recortado}…")
}
