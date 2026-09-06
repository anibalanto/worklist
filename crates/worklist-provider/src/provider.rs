//! El puerto de proveedor: `Provider::state` contesta el estado en vivo de un
//! conjunto de claves. `FileProvider` es la implementación de prueba —un
//! archivo `clave -> status`, mutable desde afuera para poder ejercer el
//! rechazo del compare-and-swap.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;

pub trait Provider {
    /// Lo que el proveedor tiene **en vivo** para esas claves.
    ///
    /// El `status` lo mira el compare-and-swap sobre toda la rama; el titulo y
    /// el cuerpo, solo sobre lo que un push escribe. Ver `concepts/sync.md`
    /// seccion "Dos alcances, porque son dos promesas".
    fn snapshot(&self, keys: &[String]) -> Result<HashMap<String, Snapshot>>;
}

/// Lo que el proveedor sabe de un item. `None` en un campo es "este proveedor
/// no lo informa", que no es lo mismo que vacio: el de prueba solo lleva
/// status, y comparar contra su ausencia daria siempre distinto.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub status: Option<String>,
    pub summary: Option<String>,
    /// El cuerpo tal como el proveedor lo devuelve — ADF, en el caso de Jira.
    pub description: Option<String>,
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
    /// El de prueba solo lleva `clave -> status`: los otros campos van en
    /// `None`, que es "no lo informa" y no "esta vacio".
    fn snapshot(&self, keys: &[String]) -> Result<HashMap<String, Snapshot>> {
        let all = self.read_all()?;
        Ok(keys
            .iter()
            .filter_map(|k| {
                all.get(k).map(|v| {
                    (k.clone(), Snapshot { status: Some(v.clone()), ..Default::default() })
                })
            })
            .collect())
    }
}

/// Si `path` es un nombre de item real (`<CLAVE>.<tipo>.md`), su clave y status.
pub fn key_of_filename(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".task.md")
        .or_else(|| name.strip_suffix(".user-story.md"))
        .or_else(|| name.strip_suffix(".epic.md"))
        .or_else(|| name.strip_suffix(".sprint.md"))?;
    (!worklist_core::is_unassigned(stem)).then(|| stem.to_string())
}

pub fn status_of(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?m)^status:\s*(\S+)").unwrap();
    re.captures(text).map(|c| c[1].to_string())
}


/// El proveedor real: Jira, por `acli`.
///
/// **Una sola llamada para N claves.** `search --jql "key in (…)"` con
/// `--fields` trae status, titulo y descripcion juntos; los campos por defecto
/// no incluyen la descripcion, asi que hay que pedirla.
pub struct JiraProvider {
    project: String,
}

impl JiraProvider {
    pub fn new(project: impl Into<String>) -> Self {
        JiraProvider { project: project.into() }
    }
}

impl Provider for JiraProvider {
    fn snapshot(&self, keys: &[String]) -> Result<HashMap<String, Snapshot>> {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        // Las claves las genera el propio sistema y matchean `^[A-Z]+-\d+$`, asi
        // que no hay texto libre entrando al JQL — el problema de `search_text`
        // no se repite aca.
        let jql = format!("project = {} AND key in ({})", self.project, keys.join(", "));
        let parsed = crate::board::acli_json(
            crate::port::Op::Snapshot,
            None,
            "search --fields",
            &[
                "jira", "workitem", "search",
                "--jql", &jql,
                "--fields", "key,status,summary,description",
                "--json", "--paginate",
            ],
        )?;
        let mut out = HashMap::new();
        for item in parsed.as_array().into_iter().flatten() {
            let Some(key) = item.get("key").and_then(|k| k.as_str()) else { continue };
            let f = item.get("fields");
            let get = |name: &str| -> Option<String> {
                f?.get(name)?.get("name")?.as_str().map(|s| s.to_string())
            };
            out.insert(
                key.to_string(),
                Snapshot {
                    status: get("status"),
                    summary: f
                        .and_then(|f| f.get("summary"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    // La descripcion viene como ADF, que es lo que el puerto
                    // promete: el cuerpo tal como el proveedor lo devuelve.
                    description: f
                        .and_then(|f| f.get("description"))
                        .filter(|v| !v.is_null())
                        .map(|v| v.to_string()),
                },
            );
        }
        Ok(out)
    }
}
