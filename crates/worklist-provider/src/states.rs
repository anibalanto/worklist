//! El vocabulario de estados del proyecto y su mapeo al proveedor.
//!
//! Son **dos archivos en dos lugares** porque son dos cosas: el vocabulario es
//! del proyecto y vive en git —`.metadata/states.yaml` del panorama—, y el
//! mapeo depende del workflow del board, asi que vive en la instalacion al
//! lado de la credencial. Ver `concepts/states.md`.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

pub use worklist_core::states::{vocabulario, DESCARTADO, POR_DEFECTO};

/// Lo que el proveedor necesita para dejar un item en un estado del proyecto.
///
/// La forma corta es azucar de la completa: `"Done"` es `{status: "Done"}`. Un
/// mapeo que solo supiera de `status` habria obligado a que `dropped` no
/// existiera —en Jira es un status **mas** una resolucion, que son campos
/// distintos— o a que mintiera diciendo `Done` a secas.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Destino {
    Status(String),
    Completo {
        status: String,
        #[serde(default)]
        resolution: Option<String>,
    },
}

impl Destino {
    pub fn status(&self) -> &str {
        match self {
            Destino::Status(s) => s,
            Destino::Completo { status, .. } => status,
        }
    }

    pub fn resolution(&self) -> Option<&str> {
        match self {
            Destino::Status(_) => None,
            Destino::Completo { resolution, .. } => resolution.as_deref(),
        }
    }
}

/// El vocabulario con su mapeo, ya verificados uno contra el otro.
#[derive(Debug, Clone)]
pub struct Estados {
    mapeo: BTreeMap<String, Destino>,
}

impl Estados {
    /// Falla si el vocabulario declara un estado que el mapeo no cubre.
    ///
    /// **Aceptarlo seria peor de la peor manera: funcionaria.** El worklist
    /// quedaria con items que no sincronizan su estado sin que nada lo diga, y
    /// el sintoma aparece semanas despues como una divergencia sin causa.
    pub fn new(vocabulario: &[String], mapeo: BTreeMap<String, Destino>) -> Result<Self> {
        let faltan: Vec<&str> =
            vocabulario.iter().map(|s| s.as_str()).filter(|s| !mapeo.contains_key(*s)).collect();
        if !faltan.is_empty() {
            bail!(
                "el mapeo de estados no cubre: {}. Todo estado declarado en \
                 .metadata/states.yaml tiene que tener entrada en la instalacion",
                faltan.join(", ")
            );
        }
        Ok(Estados { mapeo })
    }

    /// El mapeo identidad: cada estado se llama igual del otro lado.
    ///
    /// Es lo que le corresponde al proveedor de prueba, cuyo archivo lleva
    /// valores con la forma del worklist. Asi el compare-and-swap **siempre**
    /// traduce, y no hay una rama sin mapeo que se comporte distinto.
    pub fn identidad(vocabulario: &[String]) -> Self {
        Estados {
            mapeo: vocabulario
                .iter()
                .map(|s| (s.clone(), Destino::Status(s.clone())))
                .collect(),
        }
    }

    /// Lo que el proveedor tiene que ver para que el item este en `estado`.
    ///
    /// **Va en esta direccion y no en la otra**: el inverso no siempre es una
    /// funcion, porque dos estados del proyecto pueden mapear al mismo status
    /// del proveedor y distinguirse por la resolucion.
    pub fn destino(&self, estado: &str) -> Option<&Destino> {
        self.mapeo.get(estado)
    }

    pub fn declarados(&self) -> impl Iterator<Item = &str> {
        self.mapeo.keys().map(|s| s.as_str())
    }

    /// La vuelta: que estados del proyecto podrian estar detras de este status
    /// del proveedor.
    ///
    /// **Devuelve todos los candidatos y no uno**, porque el inverso no es una
    /// funcion — es lo que el comentario de `destino` ya decia, dicho ahora en
    /// un tipo. Medido en esta instalacion: `done` y `dropped` mapean los dos a
    /// `Finalizada`, asi que un item que el board dejo ahi tiene dos vueltas.
    ///
    /// Quien absorbe **no elige**: con mas de un candidato reporta, porque
    /// elegir es inventar. Ver `commands/absorb.md`.
    pub fn desde(&self, status_del_proveedor: &str) -> Vec<&str> {
        self.mapeo
            .iter()
            .filter(|(_, d)| d.status() == status_del_proveedor)
            .map(|(estado, _)| estado.as_str())
            .collect()
    }
}

/// El mapeo de la instalacion, en JSON al lado de `provider.json`.
pub fn mapeo_de_archivo(path: &Path) -> Result<BTreeMap<String, Destino>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("leyendo el mapeo de estados {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("parseando el mapeo de estados {}", path.display()))
}
