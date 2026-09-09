//! Las primitivas de git que el cliente y el servidor comparten.
//!
//! Vivian en `propagate` por quien las llamo primero, y eso las dejaba del lado
//! del proveedor: recortar una ventana **ya usa el cherry-pick** —replantar el
//! trabajo que la vista tenia encima del corte es lo que la vuelve segura de
//! re-cortar— asi que el cliente las necesita.
//!
//! Lo que **no** esta aca es la propagacion: que commits suben al panorama y
//! como se reescriben es de `worklist-provider`. Ver
//! `concepts/distribution.md` § "El corte de la lib no es el mismo que el de
//! los comandos".

use crate::git_command;
use anyhow::{bail, Context, Result};
use std::path::Path;

/// El panorama: la rama de la que se corta todo y a la que nadie empuja.
pub const PANORAMA: &str = "refs/heads/insecure/all";

/// El sha que git manda cuando una ref se crea o se borra.
pub const ALL_ZEROS: &str = "0000000000000000000000000000000000000000";

pub fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
    let out = git_command(repo)
        .args(args)
        .output()
        .with_context(|| format!("corriendo git {args:?}"))?;
    if !out.status.success() {
        bail!("git {args:?} fallo: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8(out.stdout)?)
}

pub fn try_git(repo: &Path, args: &[&str]) -> Result<(bool, String)> {
    let out = git_command(repo)
        .args(args)
        .output()
        .with_context(|| format!("corriendo git {args:?}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

pub fn rev_parse(repo: &Path, refname: &str) -> Option<String> {
    let out = git_command(repo).args(["rev-parse", "--verify", "-q", refname]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// El commit del corte: el primero que la ventana tiene sobre el panorama.
///
/// Todo lo que esta **encima** es trabajo de la ventana; el corte no, porque es
/// un commit que borra los items que el recorte dejo afuera. Lo usan las dos
/// direcciones: subir arranca despues del corte, y regenerar lo reemplaza por
/// uno nuevo y replanta lo que estaba encima.
pub fn cut_commit(repo: &Path, tip: &str, base_ref: &str) -> Result<String> {
    let base = git_output(repo, &["merge-base", base_ref, tip])?.trim().to_string();
    let log = git_output(repo, &["log", "--reverse", "--format=%H%x09%s", &format!("{base}..{tip}")])?;
    let first = log.lines().next().unwrap_or_default();
    let (sha, subject) = first.split_once('\t').unwrap_or((first, ""));
    if !subject.starts_with("window: ") {
        bail!(
            "no encuentro el corte de esta ventana: el primer commit sobre el panorama es\n\
             \x20 {} {subject}\n\
             \n\
             sin el corte no se sabe donde empieza el trabajo de la ventana, y propagarlo\n\
             entero le borraria al panorama todo lo que el recorte dejo afuera.",
            &sha[..7.min(sha.len())]
        );
    }
    Ok(sha.to_string())
}

/// Que paso al re-aplicar un commit sobre el HEAD de un worktree.
pub enum Picked {
    Applied,
    /// El arbol ya lo tenia: la re-aplicacion no aporta nada.
    Empty,
    /// El arbol ya lo dice, **escrito de otra manera**: el commit choca en
    /// bytes y coincide en forma canonica. Se deja caer igual que `Empty`.
    Superseded,
    Conflict { files: Vec<String>, output: String },
}

/// Cherry-pick de un commit, distinguiendo *no aporta nada* de *choca*.
///
/// La diferencia se lee del indice y no del codigo de salida: git falla en los
/// dos casos. Si no quedo nada en conflicto, el commit ya estaba aplicado.
///
/// Y antes de llamarlo conflicto se descuenta la normalizacion: el panorama
/// guarda la vuelta del round-trip y el commit del cliente guarda lo que se
/// tipeo, asi que un commit ya superado choca en bytes sin decir nada. Ver
/// `concepts/propagation.md` seccion "Salvo que primero hay que descontar la
/// normalizacion".
pub fn cherry_pick_one(tmp: &Path, sha: &str) -> Result<Picked> {
    let (ok, output) = try_git(tmp, &["cherry-pick", "--allow-empty", sha])?;
    if ok {
        return Ok(Picked::Applied);
    }
    let unmerged = git_output(tmp, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    if unmerged.trim().is_empty() {
        let _ = try_git(tmp, &["cherry-pick", "--skip"]);
        return Ok(Picked::Empty);
    }
    let files: Vec<String> = unmerged.lines().map(|s| s.to_string()).collect();
    let _ = try_git(tmp, &["cherry-pick", "--abort"]);
    if superseded(tmp, sha, &files) {
        return Ok(Picked::Superseded);
    }
    Ok(Picked::Conflict { files, output })
}

/// Donde el servidor anota hasta donde subio una ventana.
///
/// Va en `refs/worklist/**` y no en una rama: es contabilidad del servidor, no
/// contenido. Deducirlo comparando arboles seria preguntar *"este cambio ya
/// esta?"* sobre el panorama entero en cada push, y contestarlo mal en cuanto
/// dos ventanas tocaran lo mismo.
pub fn propagated_ref(refname: &str) -> String {
    let short = refname.strip_prefix("refs/heads/").unwrap_or(refname);
    format!("refs/worklist/propagated/{short}")
}

/// `rename <slug> -> <clave>` o `rename <slug> -> <clave> (N refs)`.
pub fn rename_subject(subject: &str) -> Option<(String, String)> {
    let rest = subject.strip_prefix("rename ")?;
    let (slug, rest) = rest.split_once(" -> ")?;
    let key = rest.split(' ').next()?;
    if slug.is_empty() || key.is_empty() {
        return None;
    }
    Some((slug.to_string(), key.to_string()))
}

/// La clave de un `normalize: <clave>`, que tampoco se copia.
///
/// La pasada 2 escribe la forma canonica **traduciendo los links a otros
/// items**, y esa traduccion lee los nombres del arbol donde corre. Asi que el
/// texto que deja es el renombre parcial de la ventana, congelado como
/// contenido: cherry-pickearlo vuelve a meter lo que el renombre rehecho
/// acababa de arreglar. Ver `concepts/propagation.md`.
pub fn normalize_subject(subject: &str) -> Option<String> {
    let key = subject.strip_prefix("normalize: ")?.trim();
    if key.is_empty() || key.contains(' ') {
        return None;
    }
    Some(key.to_string())
}

/// El id y la clave de un `sprint: <id> -> <clave>`, que tampoco se copia.
///
/// La ventana no tiene `product.yaml` para escribirle el `key` — es del
/// panorama, de un solo lado — asi que la pasada 5 deja este commit como
/// marcador, sin ningun archivo adentro. Rehacerlo es donde de verdad se
/// anota: `product::anotar_key` sobre el arbol del panorama. Ver
/// `concepts/propagation.md`.
pub fn sprint_subject(subject: &str) -> Option<(String, String)> {
    let rest = subject.strip_prefix("sprint: ")?;
    let (id, key) = rest.split_once(" -> ")?;
    if id.is_empty() || key.is_empty() || id.contains(' ') || key.contains(' ') {
        return None;
    }
    Some((id.to_string(), key.to_string()))
}

/// Si el servidor **rehace** este commit arriba en vez de copiarlo.
///
/// Son los tres que dependen del arbol que los vio: el renombre, la
/// normalizacion y la clave del sprint. Subiendo se recalculan sobre el
/// panorama; **bajando eso quiere decir que el corte ya los trae hechos**, asi
/// que replantarlos no puede aportar nada.
///
/// Se pregunta recien cuando el cherry-pick choco, y no antes: si la ventana
/// quedo adelantada —la propagacion se cayo por la mitad— el panorama todavia
/// no los tiene, el replante aplica limpio y no se pierde nada.
pub fn redone_above(subject: &str) -> bool {
    rename_subject(subject).is_some()
        || normalize_subject(subject).is_some()
        || sprint_subject(subject).is_some()
}

/// Si lo que el commit dice en los archivos que chocan ya esta en el arbol,
/// **modulo normalizacion**.
///
/// Se pregunta sobre los archivos en conflicto y no sobre los que el commit
/// toca: los demas ya los resolvio el merge. Y se pregunta con `git show`, no
/// leyendo el worktree, porque el cherry-pick se acaba de abortar.
///
/// Se contesta en una pasada porque la conversion converge: normalizar las dos
/// puntas y comparar da si o no, sin iterar.
fn superseded(tmp: &Path, sha: &str, files: &[String]) -> bool {
    files.iter().all(|file| {
        let mine = git_output(tmp, &["show", &format!("{sha}:{file}")]);
        let theirs = git_output(tmp, &["show", &format!("HEAD:{file}")]);
        let (Ok(mio), Ok(suyo)) = (mine, theirs) else {
            // Uno de los dos no existe: es un alta o una baja, y eso no es
            // normalizacion. Que lo decida el conflicto.
            return false;
        };
        match (crate::body::canonical(&mio), crate::body::canonical(&suyo)) {
            (Ok(a), Ok(b)) => a == b,
            // Un archivo que el conversor no puede leer no es uno que este
            // superado: no se puede afirmar, asi que no se afirma.
            _ => false,
        }
    })
}
