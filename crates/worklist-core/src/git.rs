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
    Conflict { files: Vec<String>, output: String },
}

/// Cherry-pick de un commit, distinguiendo *no aporta nada* de *choca*.
///
/// La diferencia se lee del indice y no del codigo de salida: git falla en los
/// dos casos. Si no quedo nada en conflicto, el commit ya estaba aplicado.
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
    let files = unmerged.lines().map(|s| s.to_string()).collect();
    let _ = try_git(tmp, &["cherry-pick", "--abort"]);
    Ok(Picked::Conflict { files, output })
}
