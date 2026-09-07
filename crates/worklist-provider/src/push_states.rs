//! Empujar al proveedor los estados que **ya** divergen.
//!
//! Es la otra pregunta que `check_push` no hace. Aquel sale de un diff —sobre
//! un push, mover solo lo que ese push movio es lo correcto, porque un push que
//! no toca un item no puede pisarlo—, y por eso mismo no alcanza para lo que
//! cambio antes de que el mapeo existiera: un item cerrado hace tres semanas no
//! vuelve a cambiar de estado, asi que su divergencia no se cierra sola.
//!
//! Aca la pregunta es **"en que difiere el proveedor de lo que el arbol dice,
//! hoy"**, y se contesta con una sola lectura de N claves. Ver `ACC-317`.

use crate::board::{Board, Transicion};
use crate::provider::Provider;
use crate::states::Estados;
use anyhow::Result;
use std::path::Path;

/// Una clave cuyo status del proveedor no es el que el arbol pide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub key: String,
    /// El estado del worklist, sin traducir. Es lo que hay que poder leer para
    /// decidir si el plan tiene sentido.
    pub local: String,
    /// Lo que el proveedor tiene hoy. `None` es "no lo informa" — el de prueba
    /// no lleva status de todas las claves.
    pub live: Option<String>,
    /// El status del proveedor al que hay que llevarlo.
    pub target: String,
}

/// El plan: lo que difiere, y lo que no se pudo mirar.
///
/// **Las dos cosas juntas y no solo la primera.** Una clave que el proveedor no
/// informa no es una que coincida: es una que no se vio, y contarla como
/// coincidente seria afirmar sobre el board sin mirarlo. Va aparte porque lo
/// que hay que hacer con ella es distinto — no se mueve, se averigua por que no
/// esta.
#[derive(Debug, Default)]
pub struct Plan {
    pub divergences: Vec<Divergence>,
    /// Claves del arbol que la lectura no trajo.
    pub unseen: Vec<String>,
}

/// Lo que hay que mover, sin mover nada.
///
/// **Una sola llamada al proveedor para las N claves**, que es lo que hace que
/// planificar sobre 288 items no cueste 288 lecturas.
pub fn divergences(
    repo: &Path,
    rev: &str,
    provider: &dyn Provider,
    states: &Estados,
) -> Result<Plan> {
    let beliefs = crate::check_push::tip_beliefs(repo, rev)?;
    if beliefs.is_empty() {
        return Ok(Plan::default());
    }
    let mut keys: Vec<String> = beliefs.keys().cloned().collect();
    keys.sort();
    let live = provider.snapshot(&keys)?;

    let mut plan = Plan::default();
    for key in keys {
        let local = &beliefs[&key];
        let Some(target) = states.destino(local) else { continue };
        let target = target.status();
        let Some(actual) = live.get(&key).and_then(|s| s.status.clone()) else {
            plan.unseen.push(key);
            continue;
        };
        if actual == target {
            continue;
        }
        plan.divergences.push(Divergence {
            key,
            local: local.clone(),
            live: Some(actual),
            target: target.to_string(),
        });
    }
    Ok(plan)
}

/// Lo que paso con una divergencia que se intento cerrar.
#[derive(Debug)]
pub struct Moved {
    pub key: String,
    pub target: String,
    pub outcome: Transicion,
}

/// Mueve las divergencias que se le den, en orden, **y no se detiene en la
/// primera que el workflow rechace**.
///
/// Un rechazo por regla es una respuesta y no una falla —reintentarlo lo vuelve
/// a rechazar para siempre—, asi que cortar el lote por uno dejaria las
/// doscientas siguientes sin mover por algo que no se va a arreglar solo. Lo
/// que si corta es un `Err`: eso es el transporte, y el que sigue va a fallar
/// igual.
pub fn push(board: &dyn Board, states: &Estados, items: &[Divergence]) -> Result<Vec<Moved>> {
    let mut out = Vec::new();
    for d in items {
        let Some(destino) = states.destino(&d.local) else { continue };
        let outcome = board.transition(&d.key, destino)?;
        out.push(Moved { key: d.key.clone(), target: d.target.clone(), outcome });
    }
    Ok(out)
}
