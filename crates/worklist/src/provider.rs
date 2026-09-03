//! El puerto de proveedor: `Provider::state` contesta el estado en vivo de un
//! conjunto de claves. `FileProvider` es la implementación de prueba —un
//! archivo `clave -> status`, mutable desde afuera para poder ejercer el
//! rechazo del compare-and-swap.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;

pub trait Provider {
    fn state(&self, keys: &[String]) -> Result<HashMap<String, String>>;
}

pub struct FileProvider {
    path: PathBuf,
}

impl FileProvider {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        FileProvider { path: path.into() }
    }

    fn read_all(&self) -> Result<HashMap<String, String>> {
        if !self.path.exists() {
            return Ok(HashMap::new());
        }
        let text = std::fs::read_to_string(&self.path)
            .with_context(|| format!("leyendo {}", self.path.display()))?;
        Ok(serde_json::from_str(&text)?)
    }

    fn write_all(&self, data: &HashMap<String, String>) -> Result<()> {
        let text = serde_json::to_string_pretty(data)?;
        std::fs::write(&self.path, text)
            .with_context(|| format!("escribiendo {}", self.path.display()))
    }

    /// Pisa el status de `key`. Devuelve el valor anterior, si había.
    pub fn set_status(&self, key: &str, status: &str) -> Result<Option<String>> {
        let mut all = self.read_all()?;
        let old = all.insert(key.to_string(), status.to_string());
        self.write_all(&all)?;
        Ok(old)
    }
}

impl Provider for FileProvider {
    fn state(&self, keys: &[String]) -> Result<HashMap<String, String>> {
        let all = self.read_all()?;
        Ok(keys
            .iter()
            .filter_map(|k| all.get(k).map(|v| (k.clone(), v.clone())))
            .collect())
    }
}

/// Si `path` es un nombre de item real (`<CLAVE>.<tipo>.md`), su clave y status.
pub fn key_of_filename(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".task.md")
        .or_else(|| name.strip_suffix(".user-story.md"))
        .or_else(|| name.strip_suffix(".epic.md"))
        .or_else(|| name.strip_suffix(".sprint.md"))?;
    (!crate::is_unassigned(stem)).then(|| stem.to_string())
}

pub fn status_of(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?m)^status:\s*(\S+)").unwrap();
    re.captures(text).map(|c| c[1].to_string())
}

