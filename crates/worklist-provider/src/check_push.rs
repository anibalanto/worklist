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
    /// Los dos lados de la diferencia. No se llaman `status` porque el campo no
    /// siempre lo es: con `cuerpo` llevan **la linea** que difiere, no el
    /// cuerpo entero.
    pub tip: String,
    pub live: String,
    /// En que linea del cuerpo aparece la primera diferencia, 1-based. Solo en
    /// un `cuerpo`, y es lo que vuelve accionable el rechazo: decir que difiere
    /// sin decir donde deja al que empuja comparando los dos cuerpos a ojo.
    pub line: Option<usize>,
    /// Las transiciones que el workflow si admite. Solo en un rechazo por
    /// regla, y **`None` es "no se pudieron listar"**: un rechazo que no dice
    /// cuales si se puede no informo nada, pero inventar una lista vacia seria
    /// afirmar sobre el board sin mirarlo.
    pub disponibles: Option<Vec<String>>,
}

/// Lo que una corrida sin push encontro sobre una rama entera.
///
/// Lleva **cuantas claves se compararon y cuales no se pudieron**, porque un
/// listado de diferencias sin esos dos numeros no dice sobre que universo
/// habla: cero diferencias sobre cero claves se lee igual que cero sobre
/// doscientas. Ver `commands/check-push.md`.
pub struct RefReport {
    /// Las claves del tip que el proveedor si informo.
    pub compared: usize,
    /// Las que no. No son coincidentes: son las que no se vieron. Ver
    /// `commands/push-states.md`.
    pub uninformed: Vec<String>,
    pub differences: Vec<RejectedKey>,
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
pub fn tip_beliefs(repo: &Path, rev: &str) -> Result<HashMap<String, String>> {
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
    let mut out = compare_status(&beliefs, &live, estados);

    // El titulo y el cuerpo, solo sobre lo que este push escribe.
    for key in crate::assign::changed_keys(repo, old, new)? {
        let Some(snap) = live.get(&key) else { continue };
        out.extend(compare_content(repo, old, &key, snap));
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
            tip: propuesto,
            live: "el workflow no la admite".into(),
            line: None,
            disponibles: Some(disponibles),
        });
    }

    Ok(out)
}

/// Las diferencias de **una rama entera**, sin push de por medio: el `status`,
/// el titulo y el cuerpo sobre todas las claves del tip.
///
/// Es lo mismo que compara `check_one` —las dos funciones de abajo, no una
/// copia—, con los dos recortes del push sacados: aca no hay ventana que se
/// pueda rechazar ni escritura que este pisando algo, asi que la rama puede ser
/// insegura y el titulo y el cuerpo se miran sobre todas. Ver
/// `commands/check-push.md` seccion "Comparar sin rechazar".
pub fn check_ref(
    repo: &Path,
    refname: &str,
    provider: &dyn Provider,
    estados: &Estados,
) -> Result<RefReport> {
    let beliefs = tip_beliefs(repo, refname)?;
    let mut keys: Vec<String> = beliefs.keys().cloned().collect();
    keys.sort();
    let live = provider.snapshot(&keys)?;

    let uninformed: Vec<String> = keys.iter().filter(|k| !live.contains_key(*k)).cloned().collect();
    let mut differences = compare_status(&beliefs, &live, estados);
    for key in &keys {
        let Some(snap) = live.get(key) else { continue };
        differences.extend(compare_content(repo, refname, key, snap));
    }
    differences.sort_by(|a, b| (&a.key, a.field).cmp(&(&b.key, b.field)));

    Ok(RefReport { compared: keys.len() - uninformed.len(), uninformed, differences })
}

/// El `status` de cada clave del tip contra el vivo, **traducido**.
///
/// El del tip esta en el vocabulario del proyecto y el del proveedor en el
/// suyo, asi que compararlos crudos rechaza TODAS las ventanas — que es lo que
/// tenia a la instalacion apuntando al proveedor de prueba. El de prueba recibe
/// el mapeo identidad, asi que aca no hay rama sin traducir. Ver
/// `concepts/states.md`.
fn compare_status(
    beliefs: &HashMap<String, String>,
    live: &HashMap<String, crate::provider::Snapshot>,
    estados: &Estados,
) -> Vec<RejectedKey> {
    beliefs
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
                    tip: format!("{tip_status} (no esta en el vocabulario)"),
                    live: live_status.clone(),
                    line: None,
                    disponibles: None,
                });
            };
            let esperado = destino.status();
            (live_status != esperado).then(|| RejectedKey {
                key: key.clone(),
                field: "status",
                // La traduccion se muestra **solo cuando dice algo**: con el
                // mapeo identidad, `open (open)` es ruido que tapa el dato.
                tip: if esperado == tip_status {
                    tip_status.clone()
                } else {
                    format!("{tip_status} ({esperado})")
                },
                live: live_status.clone(),
                line: None,
                disponibles: None,
            })
        })
        .collect()
}

/// El titulo y el cuerpo de una clave, contra lo que `rev` tiene escrito.
///
/// Devuelve **a lo sumo una** diferencia: si el titulo ya difiere, el cuerpo no
/// se mira. Son el mismo item y lo que hay que hacer es abrirlo igual.
fn compare_content(
    repo: &Path,
    rev: &str,
    key: &str,
    snap: &crate::provider::Snapshot,
) -> Option<RejectedKey> {
    let file = file_of(repo, rev, key).ok()?;
    let text = git_output(repo, &["show", &format!("{rev}:{file}")]).ok()?;

    if let (Some(live_title), Some(tip_title)) = (&snap.summary, crate::assign::title_of(&text)) {
        if *live_title != tip_title {
            return Some(RejectedKey {
                key: key.to_string(),
                field: "titulo",
                tip: tip_title,
                live: live_title.clone(),
                line: None,
                disponibles: None,
            });
        }
    }

    // Se compara **markdown contra markdown**: lo guardado es la vuelta del
    // round-trip, asi que el archivo del tip es lo que el proveedor deberia
    // tener. Convertir de un solo lado alcanza.
    let live_adf = snap.description.as_ref()?;
    let live_body = worklist_core::body::adf_to_body(live_adf).ok()?;
    let (_, tip_body) = worklist_core::body::split_frontmatter(&text);
    if live_body.trim() == tip_body.trim() {
        return None;
    }
    // Un cuerpo son cientos de lineas: ponerlas al lado convierte un rechazo en
    // un volcado del archivo. Se informa **donde** empieza a diferir.
    let (line, tip, live) = first_difference(tip_body.trim(), live_body.trim());
    Some(RejectedKey {
        key: key.to_string(),
        field: "cuerpo",
        tip,
        live,
        line: Some(line),
        disponibles: None,
    })
}

/// La primera linea en la que dos cuerpos difieren: su numero 1-based y las dos
/// versiones, recortadas.
///
/// Se llama solo sobre dos cuerpos que ya se sabe que difieren, asi que siempre
/// hay una linea que devolver: si todas las comunes coinciden, la diferencia es
/// que uno termina antes, y esa es la linea.
fn first_difference(tip: &str, live: &str) -> (usize, String, String) {
    let tip_lines: Vec<&str> = tip.lines().collect();
    let live_lines: Vec<&str> = live.lines().collect();
    for i in 0..tip_lines.len().max(live_lines.len()) {
        let a = tip_lines.get(i);
        let b = live_lines.get(i);
        if a == b {
            continue;
        }
        return (i + 1, side(a), side(b));
    }
    // Inalcanzable mientras quien llama compare antes, pero devolver algo cierto
    // es mejor que un panic en un hook.
    (1, side(tip_lines.first()), side(live_lines.first()))
}

/// Un lado de la diferencia: la linea recortada, o que ahi ya no hay linea.
fn side(line: Option<&&str>) -> String {
    match line {
        Some(l) => truncate_to(l, 72),
        None => "(el cuerpo termina antes)".into(),
    }
}

/// `s` hasta `max` caracteres — no bytes: un recorte a la mitad de una `ó`
/// rompe el UTF-8 de la salida del hook.
fn truncate_to(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
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
