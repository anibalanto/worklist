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
use crate::states::Estados;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

pub struct RejectedKey {
    pub key: String,
    /// Que campo difiere: `status`, `titulo`, `cuerpo` — o `transicion`, que no
    /// es un campo que difiere sino una regla del workflow que no se cumple.
    pub field: &'static str,
    pub tip_status: String,
    pub live_status: String,
    /// Las transiciones que el workflow si admite. Solo en un rechazo por
    /// regla, y **`None` es "no se pudieron listar"**: un rechazo que no dice
    /// cuales si se puede no informo nada, pero inventar una lista vacia seria
    /// afirmar sobre el board sin mirarlo.
    pub disponibles: Option<Vec<String>>,
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
    let out = worklist_core::git_command(repo)
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
    estados: &Estados,
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
    //
    // Y se compara **traducido**: el del tip esta en el vocabulario del
    // proyecto y el del proveedor en el suyo, asi que compararlos crudos
    // rechaza TODAS las ventanas — que es lo que tenia a la instalacion
    // apuntando al proveedor de prueba. El de prueba recibe el mapeo identidad,
    // asi que aca no hay rama sin traducir. Ver `concepts/states.md`.
    let mut out: Vec<RejectedKey> = beliefs
        .iter()
        .filter_map(|(key, tip_status)| {
            // `None` en el status del proveedor es "no lo informa", que no es
            // lo mismo que vacio: no hay nada contra que comparar.
            let live_status = live.get(key)?.status.as_ref()?;
            // Un status que el vocabulario no declara **se rechaza**, no se
            // saltea: no poder traducirlo es no poder decir nada sobre ese
            // item, y callar seria confundirlo con "esta bien".
            let Some(destino) = estados.destino(tip_status) else {
                return Some(RejectedKey {
                    key: key.clone(),
                    field: "status",
                    tip_status: format!("{tip_status} (no esta en el vocabulario)"),
                    live_status: live_status.clone(),
                    disponibles: None,
                });
            };
            let esperado = destino.status();
            (live_status != esperado).then(|| RejectedKey {
                key: key.clone(),
                field: "status",
                // La traduccion se muestra **solo cuando dice algo**: con el
                // mapeo identidad, `open (open)` es ruido que tapa el dato.
                tip_status: if esperado == tip_status {
                    tip_status.clone()
                } else {
                    format!("{tip_status} ({esperado})")
                },
                live_status: live_status.clone(),
                disponibles: None,
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
                        disponibles: None,
                    });
                    continue;
                }
            }
        }
        if let Some(live_adf) = &snap.description {
            // Se compara **markdown contra markdown**: lo guardado es la vuelta
            // del round-trip, asi que el archivo del tip es lo que el proveedor
            // deberia tener. Convertir de un solo lado alcanza.
            let Ok(live_body) = worklist_core::body::adf_to_body(live_adf) else { continue };
            let (_, tip_body) = worklist_core::body::split_frontmatter(&text);
            if live_body.trim() != tip_body.trim() {
                out.push(RejectedKey {
                    key: key.clone(),
                    field: "cuerpo",
                    tip_status: "el del tip".into(),
                    live_status: "otro en el proveedor".into(),
                    disponibles: None,
                });
            }
        }
    }

    // ── La transicion propuesta.
    //
    // **Es lo unico de este archivo que mira lo que trae el push**, y no
    // contradice al compare-and-swap: es otra pregunta. Aquel verifica que la
    // rama siga siendo consistente; esta, que lo que se pide sea legal en el
    // workflow. Un rechazo por regla no se arregla reintentando, asi que tiene
    // que llegar **antes** de aceptar el push y no despues.
    //
    // El costo es una llamada por item **cuyo status cambia**, no por item de
    // la ventana: en un push normal son cero o uno.
    for (key, propuesto) in transiciones_propuestas(repo, old, new, estados)? {
        let Some(disponibles) = provider.available_transitions(&key)? else { continue };
        if disponibles.iter().any(|d| d == &propuesto) {
            continue;
        }
        out.push(RejectedKey {
            key,
            field: "transicion",
            tip_status: propuesto,
            live_status: "el workflow no la admite".into(),
            disponibles: Some(disponibles),
        });
    }

    Ok(out)
}

/// Los `(clave, status del proveedor)` que este push propone mover.
///
/// Dos fuentes, y la segunda no lleva ningun campo: un item cuyo `status`
/// cambio entre los dos arboles, y uno que el push **borra** — sacar un item
/// del arbol es proponer su transicion a `dropped`. Ver `commands/remove.md`.
pub fn transiciones_propuestas(
    repo: &Path,
    old: &str,
    new: &str,
    estados: &Estados,
) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for key in crate::assign::changed_keys(repo, old, new)? {
        let (Some(antes), Some(ahora)) = (status_en(repo, old, &key), status_en(repo, new, &key))
        else {
            continue;
        };
        if antes == ahora {
            continue;
        }
        if let Some(d) = estados.destino(&ahora) {
            out.push((key, d.status().to_string()));
        }
    }
    for key in crate::assign::dropped_keys(repo, old, new)? {
        if let Some(d) = estados.destino(crate::states::DESCARTADO) {
            out.push((key, d.status().to_string()));
        }
    }
    Ok(out)
}

fn status_en(repo: &Path, rev: &str, key: &str) -> Option<String> {
    let file = file_of(repo, rev, key).ok()?;
    let text = git_output(repo, &["show", &format!("{rev}:{file}")]).ok()?;
    status_of(&text)
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
