//! El compare-and-swap de una ventana: compara lo que el tip actual de la
//! rama tiene escrito contra el estado en vivo del proveedor. No mira el
//! contenido que llega en el push.

use crate::provider::{key_of_filename, status_of, Provider};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

pub struct RejectedKey {
    pub key: String,
    pub tip_status: String,
    pub live_status: String,
}

const ALL_ZEROS: &str = "0000000000000000000000000000000000000000";

/// Las tres clases que los hooks distinguen. Ver `concepts/sync.md` § "Dos
/// clases de rama, y el nombre dice qué se puede hacer".
#[derive(Debug, PartialEq, Eq)]
pub enum RefClass {
    /// `refs/heads/secure/**` — una ventana: se verifica y se le puede empujar.
    Secure,
    /// `refs/heads/insecure/**` — el panorama: no se puede verificar, asi que
    /// tampoco aceptar escrituras. Es lo que vuelve cierta la palabra.
    Insecure,
    /// No es del worklist: los hooks no opinan.
    Other,
}

pub fn classify(refname: &str) -> RefClass {
    if refname.starts_with("refs/heads/secure/") {
        RefClass::Secure
    } else if refname.starts_with("refs/heads/insecure/") {
        RefClass::Insecure
    } else {
        RefClass::Other
    }
}

fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
    let out = crate::git_command(repo)
        .args(args)
        .output()
        .with_context(|| format!("corriendo git {:?}", args))?;
    if !out.status.success() {
        bail!("git {:?} fallo: {}", args, String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8(out.stdout)?)
}

/// Clave -> status para todo item con clave real en el arbol de `rev`.
fn tip_beliefs(repo: &Path, rev: &str) -> Result<HashMap<String, String>> {
    let listing = git_output(repo, &["ls-tree", "-r", "--name-only", rev])?;
    let mut beliefs = HashMap::new();
    for name in listing.lines() {
        let Some(key) = key_of_filename(name) else { continue };
        let content = git_output(repo, &["show", &format!("{rev}:{name}")])?;
        if let Some(status) = status_of(&content) {
            beliefs.insert(key, status);
        }
    }
    Ok(beliefs)
}

/// Para un `(old, ref)` de un `pre-receive`: las claves cuyo status en vivo
/// difiere del que el tip `old` tenia escrito. Vacio = nada que rechazar.
/// `new` no entra en la comparacion: el chequeo es sobre lo que ya estaba,
/// nunca sobre lo que trae el push.
pub fn check_one(
    repo: &Path,
    old: &str,
    refname: &str,
    provider: &dyn Provider,
) -> Result<Vec<RejectedKey>> {
    if classify(refname) != RefClass::Secure || old == ALL_ZEROS {
        return Ok(vec![]);
    }

    let beliefs = tip_beliefs(repo, old)?;
    if beliefs.is_empty() {
        return Ok(vec![]);
    }
    let keys: Vec<String> = beliefs.keys().cloned().collect();
    let live = provider.state(&keys)?;

    Ok(beliefs
        .into_iter()
        .filter_map(|(key, tip_status)| {
            let live_status = live.get(&key)?;
            (*live_status != tip_status).then(|| RejectedKey {
                key: key.clone(),
                tip_status,
                live_status: live_status.clone(),
            })
        })
        .collect())
}
