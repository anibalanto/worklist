//! El puerto de proveedor: `Provider::state` contesta el estado en vivo de un
//! conjunto de claves. `FileProvider` es la implementación de prueba —un
//! archivo `clave -> status`, mutable desde afuera para poder ejercer el
//! rechazo del compare-and-swap.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;

/// Que sabe el proveedor de una clave que no informo.
///
/// **Son dos casos que su mensaje no distingue** —*"no existe o no tienes
/// permiso para verla"*— y que su codigo HTTP si.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existencia {
    Si,
    /// 404. La clave murio del otro lado.
    Borrada,
    /// 403. **No se toca**: una credencial sin permiso no es un item borrado.
    SinPermiso,
}

pub trait Provider {
    /// Lo que el proveedor tiene **en vivo** para esas claves.
    ///
    /// El `status` lo mira el compare-and-swap sobre toda la rama; el titulo y
    /// el cuerpo, solo sobre lo que un push escribe. Ver `concepts/sync.md`
    /// seccion "Dos alcances, porque son dos promesas".
    fn snapshot(&self, keys: &[String]) -> Result<HashMap<String, Snapshot>>;

    /// Si el proveedor **todavia tiene** esta clave.
    ///
    /// `None` es *"este proveedor no lo puede contestar"* — el de prueba no
    /// tiene con que— y **no es** "no existe": de eso depende que un item se
    /// saque del arbol, asi que confundirlos destruye.
    ///
    /// Ver `concepts/composition.md` seccion "Se decide por el codigo, nunca
    /// por el mensaje".
    fn existe(&self, _key: &str) -> Result<Option<Existencia>> {
        Ok(None)
    }

    /// Los estados a los que el workflow deja mover este issue **hoy**.
    ///
    /// Es una **lectura**, y por eso vive en este puerto y no en `Board`:
    /// `Board` escribe. Con esto el `pre-receive` puede rechazar una transicion
    /// ilegal **antes** de aceptar el push, que es lo unico que hace que un
    /// rechazo por regla se pueda distinguir de uno por deriva sin haber
    /// escrito nada de por medio.
    ///
    /// `None` es *"este proveedor no lo informa"* —el de prueba no tiene
    /// workflow— y ahi no hay nada que verificar. No es "ninguna disponible".
    fn available_transitions(&self, _key: &str) -> Result<Option<Vec<String>>> {
        Ok(None)
    }
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

/// Si `path` es un nombre de item real (`<CLAVE>.<tipo>.md`), su clave.
///
/// Dos condiciones y no una: el stem tiene que ser un id —lo que deja afuera a
/// `_sprints/20.sprint.md`, porque el `/` no es de un id— y no llevar la marca
/// `@`, que es lo que distingue un pedido de algo que el proveedor ya nombra.
pub fn key_of_filename(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".task.md")
        .or_else(|| name.strip_suffix(".user-story.md"))
        .or_else(|| name.strip_suffix(".epic.md"))
        .or_else(|| name.strip_suffix(".sprint.md"))?;
    (worklist_core::is_valid_id(stem) && !worklist_core::is_unassigned(stem))
        .then(|| stem.to_string())
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
    api: crate::api::Api,
}

impl JiraProvider {
    pub fn new(project: impl Into<String>, api: crate::api::Api) -> Self {
        JiraProvider { project: project.into(), api }
    }
}

impl Provider for JiraProvider {
    /// Los status a los que este issue puede ir hoy, **por REST**: ni `acli` ni
    /// `jira-cli` saben listar las transiciones de un issue.
    ///
    /// Devuelve **status de destino y no nombres de transicion**, porque es
    /// contra el mapeo de la instalacion que se compara y el mapeo habla de
    /// status. Los dos nombres pueden no coincidir — medido: la transicion
    /// `Listo` deja el item en `Finalizada`.
    ///
    /// Su fracaso no se traga: se propaga, y quien llama decide — no poder
    /// listar no es "no hay ninguna".
    fn existe(&self, key: &str) -> Result<Option<Existencia>> {
        self.api.existe(key).map(Some)
    }

    fn available_transitions(&self, key: &str) -> Result<Option<Vec<String>>> {
        self.api.reachable_statuses(key).map(Some)
    }

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
