//! El compare-and-swap de una ventana: compara lo que el tip actual de la
//! rama tiene escrito contra lo que el proveedor tiene en vivo. No mira el
//! contenido que llega en el push.
//!
//! Son **dos alcances**, porque son dos promesas: el `status` sobre todas las
//! claves del tip —la rama promete verificarse entera— y el titulo y el cuerpo
//! solo sobre las que el push escribe, porque *"cualquier escritura tiene que
//! probar que parte del estado actual"*. Un push que no toca un item no puede
//! pisarlo. Ver `concepts/sync.md`.

use crate::provider::{key_of_filename, status_of, Provider};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

pub struct RejectedKey {
    pub key: String,
    /// Que campo difiere: `status`, `titulo` o `cuerpo`.
    pub field: &'static str,
    pub tip_status: String,
    pub live_status: String,
}

pub const ALL_ZEROS: &str = "0000000000000000000000000000000000000000";

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
    new: &str,
    refname: &str,
    provider: &dyn Provider,
) -> Result<Vec<RejectedKey>> {
    // `old` en ceros es una rama que nace: no habia creencia previa que
    // comparar. `new` en ceros es un **borrado**, y borrar una rama no escribe
    // nada en el proveedor, asi que no hay nada que probar. Ver la task `5j`.
    if classify(refname) != RefClass::Secure || old == ALL_ZEROS || new == ALL_ZEROS {
        return Ok(vec![]);
    }

    let beliefs = tip_beliefs(repo, old)?;
    if beliefs.is_empty() {
        return Ok(vec![]);
    }
    let keys: Vec<String> = beliefs.keys().cloned().collect();
    let live = provider.snapshot(&keys)?;

    // El `status`, sobre todas: la rama promete verificarse entera.
    let mut out: Vec<RejectedKey> = beliefs
        .iter()
        .filter_map(|(key, tip_status)| {
            let live_status = live.get(key)?.status.as_ref()?;
            (live_status != tip_status).then(|| RejectedKey {
                key: key.clone(),
                field: "status",
                tip_status: tip_status.clone(),
                live_status: live_status.clone(),
            })
        })
        .collect();

    // El titulo y el cuerpo, solo sobre lo que este push escribe.
    for key in crate::assign::changed_keys(repo, old, new)? {
        let Some(snap) = live.get(&key) else { continue };
        let Ok(text) = git_output(repo, &["show", &format!("{old}:{}", file_of(repo, old, &key)?)])
        else {
            continue;
        };
        if let Some(live_title) = &snap.summary {
            if let Some(tip_title) = crate::assign::title_of(&text) {
                if *live_title != tip_title {
                    out.push(RejectedKey {
                        key: key.clone(),
                        field: "titulo",
                        tip_status: tip_title,
                        live_status: live_title.clone(),
                    });
                    continue;
                }
            }
        }
        if let Some(live_adf) = &snap.description {
            // Se compara **markdown contra markdown**: lo guardado es la vuelta
            // del round-trip, asi que el archivo del tip es lo que el proveedor
            // deberia tener. Convertir de un solo lado alcanza.
            let Ok(live_body) = crate::body::adf_to_body(live_adf) else { continue };
            let (_, tip_body) = crate::body::split_frontmatter(&text);
            if live_body.trim() != tip_body.trim() {
                out.push(RejectedKey {
                    key: key.clone(),
                    field: "cuerpo",
                    tip_status: "el del tip".into(),
                    live_status: "otro en el proveedor".into(),
                });
            }
        }
    }

    Ok(out)
}

/// El archivo de `key` en `rev`, con su tipo.
fn file_of(repo: &Path, rev: &str, key: &str) -> Result<String> {
    let listing = git_output(repo, &["ls-tree", "-r", "--name-only", rev])?;
    listing
        .lines()
        .find(|n| key_of_filename(n).as_deref() == Some(key))
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("{key} no esta en {rev}"))
}
