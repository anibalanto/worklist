//! Asignar una clave real a un pedido: buscar por titulo antes de crear, para
//! que un reintento despues de una falla nunca duplique.

use anyhow::{bail, Context, Result};
use std::process::Command;

/// Como se obtuvo la clave. La distincion existe porque `acli` acepta
/// `--parent` al crear y no al editar: sobre un issue que ya existia, la
/// jerarquia pedida no se aplico.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assignment {
    Created(String),
    Found(String),
}

impl Assignment {
    pub fn key(&self) -> &str {
        match self {
            Assignment::Created(k) | Assignment::Found(k) => k,
        }
    }
    /// El padre pedido no se aplico: el issue ya existia.
    pub fn parent_missed(&self, parent: Option<&str>) -> bool {
        matches!(self, Assignment::Found(_)) && parent.is_some()
    }
}

pub trait Creator {
    /// La clave del item, creandolo si no existe. `parent` es la clave de su
    /// **epica ancestro**, no la de su padre directo: Jira no admite `parent`
    /// entre tipos del mismo nivel, y el escalon del medio del worklist viaja
    /// como link. Ver `concepts/sync.md` seccion "La jerarquia entra hasta
    /// donde el proveedor la tiene".
    ///
    /// `Found` en vez de `Created` es informacion, no un detalle: el padre solo
    /// se puede poner al crear, asi que un item que se encontro no lleva la
    /// jerarquia que se le pidio y quien llama tiene que poder decirlo.
    fn create_or_find(
        &self,
        title: &str,
        item_type: &str,
        description: &str,
        parent: Option<&str>,
    ) -> Result<Assignment>;
    /// `a` se relaciona con `b`. Idempotente, como `link_blocks`.
    fn link_relates(&self, a: &str, b: &str) -> Result<bool>;
    /// Pisa el titulo de un item que ya existe.
    ///
    /// Es lo que se ve en el board, asi que un titulo que diverge es peor que
    /// un cuerpo que diverge. Ver `concepts/sync.md` seccion "Un item que ya
    /// tiene clave se actualiza".
    fn set_summary(&self, key: &str, title: &str) -> Result<()>;
    /// Pisa la descripcion de un item ya creado. Es la pasada 2: el cuerpo no
    /// puede viajar en la creacion porque ahi los renombres todavia no
    /// terminaron. Ver `concepts/sync.md`.
    fn set_description(&self, key: &str, adf: &str) -> Result<()>;
    /// `blocker` bloquea a `blocked`. Idempotente: si el vinculo ya existe, no
    /// hace nada.
    fn link_blocks(&self, blocker: &str, blocked: &str) -> Result<bool>;
}

/// El texto con el que se **busca**, que no es el titulo.
///
/// `summary ~` no compara texto literal: es full-text, y su parser tiene
/// metacaracteres. `[` y `]` lo rompen aunque vayan entre comillas —JQL no
/// tiene como escaparlos, `\[` es ilegal— y `*` es un comodin que en
/// `refs/bilink/*` devuelve cero resultados sobre un issue que existe.
///
/// **Escapar de a un caracter es perder la carrera**: el que falta se descubre
/// duplicando un issue. Por eso se busca por un subconjunto seguro **por
/// construccion** —solo alfanumericos y espacios— y la decision la toma
/// `search` comparando el `summary` entero. La query busca de mas; la que
/// decide es una comparacion que no pasa por ningun parser.
///
/// Los acentos se conservan: son alfanumericos y el full-text **no** los
/// normaliza — "Indice" no encuentra un issue titulado "Índice".
pub fn search_text(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut space = true; // arranca en true para no abrir con espacio
    for c in title.chars() {
        if c.is_alphanumeric() {
            out.push(c);
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.trim_end().to_string()
}

pub fn jira_type(worklist_type: &str) -> Result<&'static str> {
    match worklist_type {
        "task" => Ok("Tarea"),
        "user-story" => Ok("Historia"),
        "epic" => Ok("Epic"),
        other => bail!("tipo de item desconocido: {other} (se esperaba task, user-story o epic)"),
    }
}

/// Corre `acli --json` y devuelve su salida parseada.
///
/// **`acli` sale con 0 cuando la operacion falla.** Escribe una linea de
/// fracaso en `stdout` y nada mas, asi que mirar el codigo de retorno no
/// alcanza — y buscar ese texto tampoco, porque es de una herramienta ajena y
/// esta en el idioma de quien la corre. El resultado se lee de la salida
/// estructurada. Ver `concepts/sync.md` seccion "El exito se lee de la salida".
fn acli_json(args: &[&str], what: &str) -> Result<serde_json::Value> {
    let out = Command::new("acli")
        .args(args)
        .output()
        .with_context(|| format!("corriendo acli {what}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        bail!("acli {what} fallo (exit {:?}): {stderr}{stdout}", out.status.code());
    }
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .with_context(|| format!("acli {what} no devolvio JSON: {stdout}{stderr}"))?;
    check_batch(&parsed, what)?;
    Ok(parsed)
}

/// Las operaciones de lote de `acli` —`edit` entre ellas— responden con un
/// `results` donde cada entrada lleva su propio `status`. Donde esa forma
/// esta, el estado de cada item es la unica verdad sobre si se hizo. Donde no
/// esta —`create` devuelve la clave sola— no hay nada que chequear aca.
pub fn check_batch(v: &serde_json::Value, what: &str) -> Result<()> {
    let Some(results) = v.get("results").and_then(|r| r.as_array()) else {
        return Ok(());
    };
    let failed: Vec<String> = results
        .iter()
        .filter(|r| r.get("status").and_then(|s| s.as_str()) != Some("SUCCESS"))
        .map(|r| {
            let id = r.get("id").and_then(|i| i.as_str()).unwrap_or("?");
            let msg = r.get("message").and_then(|m| m.as_str()).unwrap_or("sin mensaje");
            format!("{id}: {msg}")
        })
        .collect();
    if !failed.is_empty() {
        bail!("acli {what} rechazado por el proveedor — {}", failed.join("; "));
    }
    Ok(())
}

/// Los links de `key`, como `(tipo, la otra punta)`.
///
/// `link list --json` solo trae la punta de afuera, y viene nula cuando el
/// consultado **es** esa punta: desde ahi no se sabe quien es la otra. Por eso
/// se consulta siempre el lado de adentro, que es el que ve al bloqueador.
fn links_of(key: &str) -> Result<Vec<(String, String)>> {
    let v = acli_json(
        &["jira", "workitem", "link", "list", "--key", key, "--json"],
        "link list",
    )?;
    Ok(v.get("issueLinks")
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| {
                    let t = l.get("typeName")?.as_str()?.to_string();
                    let o = l.get("outwardIssueKey")?.as_str()?.to_string();
                    Some((t, o))
                })
                .collect()
        })
        .unwrap_or_default())
}

pub struct AcliCreator {
    project: String,
}

impl AcliCreator {
    pub fn new(project: impl Into<String>) -> Self {
        AcliCreator { project: project.into() }
    }

    fn jql(&self, title: &str) -> String {
        format!(
            "project = {} AND summary ~ \"{}\"",
            self.project,
            search_text(title)
        )
    }

    /// La clave del issue cuyo `summary` es **exactamente** este titulo.
    ///
    /// La JQL busca de mas a proposito: lleva solo los alfanumericos del
    /// titulo, asi que ningun metacaracter llega al parser. Quien decide es la
    /// comparacion de aca abajo. Ver `search_text`.
    fn search(&self, title: &str) -> Result<Option<String>> {
        let needle = search_text(title);
        if needle.is_empty() {
            // Sin nada alfanumerico no hay query posible, y crear a ciegas
            // seria duplicar por otro camino.
            bail!("el titulo {title:?} no tiene con que buscarse: no deja ningun alfanumerico");
        }
        let jql = self.jql(title);
        let parsed = acli_json(
            &["jira", "workitem", "search", "--jql", &jql, "--json"],
            "search",
        )?;
        let key = parsed
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| {
                item.get("fields")
                    .and_then(|f| f.get("summary"))
                    .and_then(|s| s.as_str())
                    == Some(title)
            })
            .and_then(|item| item.get("key"))
            .and_then(|k| k.as_str())
            .map(|s| s.to_string());
        Ok(key)
    }

    fn create(
        &self,
        title: &str,
        item_type: &str,
        description: &str,
        parent: Option<&str>,
    ) -> Result<String> {
        let mut args = vec![
            "jira", "workitem", "create",
            "--project", &self.project,
            "--type", item_type,
            "--summary", title,
            "--description", description,
            "--json",
        ];
        if let Some(p) = parent {
            args.push("--parent");
            args.push(p);
        }
        let parsed = acli_json(&args, "create")?;
        parsed
            .get("key")
            .and_then(|k| k.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("acli create no devolvio 'key': {parsed}"))
    }
}

impl AcliCreator {
    /// Vincula dos issues y **verifica el efecto**.
    ///
    /// `link create` no acepta `--json`, asi que su resultado no se puede leer
    /// de la salida: se lee del proveedor, listando los links de vuelta. Es una
    /// llamada mas y es la unica forma de no confundir "lo intente" con "esta".
    ///
    /// Buscar antes de vincular es por el mismo motivo que `create_or_find`: si
    /// el hook falla despues, el reintento no puede duplicar. La busqueda es
    /// por `(tipo, punta)` y no por texto — un `contains` sobre la salida
    /// cruda daba por existente cualquier link que mencionara la clave.
    fn link(&self, blocker: &str, blocked: &str, link_type: &str) -> Result<bool> {
        let already = |ls: &[(String, String)]| {
            ls.iter().any(|(t, o)| t == link_type && o == blocker)
        };
        if already(&links_of(blocked)?) {
            return Ok(false);
        }
        let out = Command::new("acli")
            .args([
                "jira", "workitem", "link", "create",
                "--out", blocker, "--in", blocked, "--type", link_type,
                "--yes",
            ])
            .output()
            .context("corriendo acli jira workitem link create")?;
        if !out.status.success() {
            bail!("acli link create fallo: {}", String::from_utf8_lossy(&out.stderr));
        }
        if !already(&links_of(blocked)?) {
            bail!(
                "el link {link_type} {blocker} -> {blocked} no quedo: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(true)
    }
}

impl Creator for AcliCreator {
    fn create_or_find(
        &self,
        title: &str,
        item_type: &str,
        description: &str,
        parent: Option<&str>,
    ) -> Result<Assignment> {
        if let Some(key) = self.search(title)? {
            return Ok(Assignment::Found(key));
        }
        self.create(title, jira_type(item_type)?, description, parent)
            .map(Assignment::Created)
    }

    fn link_relates(&self, a: &str, b: &str) -> Result<bool> {
        self.link(a, b, "Relates")
    }

    fn set_summary(&self, key: &str, title: &str) -> Result<()> {
        acli_json(
            &["jira", "workitem", "edit", "--key", key, "--summary", title, "--yes", "--json"],
            "edit --summary",
        )
        .map(|_| ())
    }

    fn set_description(&self, key: &str, adf: &str) -> Result<()> {
        // Por archivo y no por flag: un ADF de un cuerpo real no entra comodo
        // en una linea de comando.
        let tmp = std::env::temp_dir().join(format!("worklist-desc-{key}.json"));
        std::fs::write(&tmp, adf)?;
        let res = acli_json(
            &[
                "jira", "workitem", "edit", "--key", key,
                "--description-file", tmp.to_str().unwrap_or_default(),
                "--yes", "--json",
            ],
            "edit --description-file",
        );
        let _ = std::fs::remove_file(&tmp);
        res.map(|_| ())
    }

    fn link_blocks(&self, blocker: &str, blocked: &str) -> Result<bool> {
        self.link(blocker, blocked, "Blocks")
    }
}

/// Lo que `--dry-run` imprime, sin llamar a `acli`. El escapado acá es sólo
/// para que la línea se lea como el comando real —`Command` nunca pasa por
/// un shell—, pero mostrar comillas sin escapar rompería la lectura igual.
pub fn dry_run_plan(project: &str, item_type: &str, title: &str, description: &str) -> Result<String> {
    let display = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!(
        "would search: project = {project} AND summary ~ \"{}\"\nwould create: --project {project} --type {} --summary \"{}\" --description \"{}\"",
        search_text(title),
        jira_type(item_type)?,
        display(title),
        display(description),
    ))
}
