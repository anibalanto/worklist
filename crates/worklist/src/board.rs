//! Asignar una clave real a un pedido: buscar por titulo antes de crear, para
//! que un reintento despues de una falla nunca duplique.

use crate::port::{Failure, Op};
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

pub trait Board {
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
    /// Pone la epica de un issue **que ya existe**.
    ///
    /// Es la mitad que `create_or_find` no puede: el padre solo viaja en la
    /// creacion, asi que un item encontrado se quedaba sin la jerarquia que se
    /// le pidio y lo unico que se podia hacer era avisar. Devuelve `false` si
    /// ya estaba puesto.
    fn set_parent(&self, key: &str, epic: &str) -> Result<bool>;
    /// La epica que el proveedor tiene puesta, o `None`.
    ///
    /// Existe porque **el puerto no afirma sobre el board sin mirarlo**: decir
    /// "el padre no quedo" a partir de lo que una corrida pudo hacer es una
    /// inferencia, y una que ya mando a revisar 22 issues que estaban bien.
    fn parent_of(&self, key: &str) -> Result<Option<String>>;
    /// Mete issues en un sprint. **De lote**: el transporte toma decenas por
    /// llamada, y hacerlo de a uno seria una llamada por issue.
    fn add_to_sprint(&self, sprint: &str, keys: &[&str]) -> Result<usize>;
}

/// El texto con el que se **busca**, que no es el titulo.
///
/// `summary ~` no compara texto literal: es full-text, con metacaracteres y con
/// su propia tokenizacion. Se sacan **sólo los que rompen**, medidos contra
/// Jira y no supuestos:
///
/// | | |
/// |---|---|
/// | `[ ] ( ) { } ^ "` | la JQL no parsea |
/// | `*` | parsea y devuelve cero sobre un issue que existe |
///
/// **Y nada mas.** Guiones, puntos, barras, `_`, `:`, `#`, `?`, `+`, `&`, `|`,
/// `!`, `~` y comillas simples estan medidos como seguros, y sacarlos rompe la
/// **tokenizacion**: buscar `bilinker 002 file partition` no encuentra un issue
/// titulado `bilinker-002-file-partition`, y el reintento lo duplica. Ver la
/// task `65`.
///
/// La query no tiene que ser precisa —de eso se ocupa `search`, comparando el
/// `summary` entero— sino **no fallar** y **encontrar**. Y las dos formas de
/// equivocarse no son simetricas: un metacaracter que se cuele hace fallar la
/// JQL y detiene el push; un caracter quitado de mas duplica en silencio.
///
/// Los acentos se conservan: el full-text **no** los normaliza — "Indice" no
/// encuentra un issue titulado "Índice".
pub fn search_text(title: &str) -> String {
    const ROMPEN: [char; 9] = ['[', ']', '(', ')', '{', '}', '^', '"', '*'];
    let mut out = String::with_capacity(title.len());
    let mut space = true;
    for c in title.chars() {
        if ROMPEN.contains(&c) || c == '\\' {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.push(c);
            space = c.is_whitespace();
        }
    }
    out.trim().to_string()
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
pub(crate) fn acli_json(
    op: Op,
    key: Option<&str>,
    what: &str,
    args: &[&str],
) -> Result<serde_json::Value> {
    let out = Command::new("acli")
        .args(args)
        .output()
        .with_context(|| format!("corriendo acli {what}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        return Err(Failure::new(
            op,
            key,
            format!("{what} salio con {:?}: {stderr}{stdout}", out.status.code()),
        )
        .into());
    }
    let parsed: serde_json::Value = serde_json::from_str(&stdout).map_err(|_| {
        Failure::new(op, key, format!("{what} no devolvio JSON: {stdout}{stderr}"))
    })?;
    check_batch(&parsed, op)?;
    Ok(parsed)
}

/// Las operaciones de lote de `acli` —`edit` entre ellas— responden con un
/// `results` donde cada entrada lleva su propio `status`. Donde esa forma
/// esta, el estado de cada item es la unica verdad sobre si se hizo. Donde no
/// esta —`create` devuelve la clave sola— no hay nada que chequear aca.
pub fn check_batch(v: &serde_json::Value, op: Op) -> Result<()> {
    let Some(results) = v.get("results").and_then(|r| r.as_array()) else {
        return Ok(());
    };
    let failed: Vec<(String, String)> = results
        .iter()
        .filter(|r| r.get("status").and_then(|s| s.as_str()) != Some("SUCCESS"))
        .map(|r| {
            let id = r.get("id").and_then(|i| i.as_str()).unwrap_or("?");
            let msg = r.get("message").and_then(|m| m.as_str()).unwrap_or("sin mensaje");
            (id.to_string(), msg.to_string())
        })
        .collect();
    match failed.as_slice() {
        [] => Ok(()),
        // Un solo fallo tiene una clave, y la clave va en su lugar y no
        // enterrada en el texto.
        [(id, msg)] => Err(Failure::new(op, Some(id), msg.clone()).into()),
        many => Err(Failure::new(
            op,
            None,
            format!(
                "el proveedor rechazo {} del lote — {}",
                many.len(),
                many.iter().map(|(i, m)| format!("{i}: {m}")).collect::<Vec<_>>().join("; ")
            ),
        )
        .into()),
    }
}

/// Los links de `key`, como `(tipo, la otra punta)`.
///
/// `link list --json` solo trae la punta de afuera, y viene nula cuando el
/// consultado **es** esa punta: desde ahi no se sabe quien es la otra. Por eso
/// se consulta siempre el lado de adentro, que es el que ve al bloqueador.
fn links_of(key: &str) -> Result<Vec<(String, String)>> {
    let v = acli_json(
        Op::Link,
        Some(key),
        "link list",
        &["jira", "workitem", "link", "list", "--key", key, "--json"],
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

/// Cuantos issues entran en una llamada de `sprint add`.
const SPRINT_BATCH: usize = 50;

/// Corre `jira-cli`, que es el transporte de lo que `acli` no puede.
///
/// **De este todavia no se sabe como informa un fallo.** De `acli` si: sale
/// con 0 y pone el fracaso en el cuerpo, y eso costo cinco descripciones
/// rechazadas en silencio. Averiguarlo aca es leer su codigo, no empujar
/// contra el board — ver `concepts/sync.md` seccion "Dos transportes, dos
/// formas de mentir, una sola respuesta".
///
/// Hasta entonces el codigo de salida es lo unico que hay, y quien llama pide
/// el efecto de vuelta donde puede pedirlo.
fn jira(op: Op, key: Option<&str>, args: &[&str]) -> Result<String> {
    let out = Command::new("jira").args(args).output().map_err(|e| {
        Failure::new(op, key, format!("no se pudo correr `jira`: {e} — jira-cli no esta instalado"))
    })?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        return Err(Failure::new(
            op,
            key,
            format!("jira {} salio con {:?}: {stderr}{stdout}", args.join(" "), out.status.code()),
        )
        .into());
    }
    Ok(stdout)
}

pub struct JiraBoard {
    project: String,
}

impl JiraBoard {
    pub fn new(project: impl Into<String>) -> Self {
        JiraBoard { project: project.into() }
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
            Op::Search,
            None,
            "search",
            &["jira", "workitem", "search", "--jql", &jql, "--json"],
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
        let parsed = acli_json(Op::Create, None, "create", &args)?;
        parsed
            .get("key")
            .and_then(|k| k.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("acli create no devolvio 'key': {parsed}"))
    }
}

impl JiraBoard {
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

impl Board for JiraBoard {
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
            Op::SetSummary,
            Some(key),
            "edit --summary",
            &["jira", "workitem", "edit", "--key", key, "--summary", title, "--yes", "--json"],
        )
        .map(|_| ())
    }

    fn set_description(&self, key: &str, adf: &str) -> Result<()> {
        // Por archivo y no por flag: un ADF de un cuerpo real no entra comodo
        // en una linea de comando.
        let tmp = std::env::temp_dir().join(format!("worklist-desc-{key}.json"));
        std::fs::write(&tmp, adf)?;
        let res = acli_json(
            Op::SetDescription,
            Some(key),
            "edit --description-file",
            &[
                "jira", "workitem", "edit", "--key", key,
                "--description-file", tmp.to_str().unwrap_or_default(),
                "--yes", "--json",
            ],
        );
        let _ = std::fs::remove_file(&tmp);
        res.map(|_| ())
    }

    fn link_blocks(&self, blocker: &str, blocked: &str) -> Result<bool> {
        self.link(blocker, blocked, "Blocks")
    }

    /// Por `view` y no por `search`: `acli` responde `field 'parent' is not
    /// allowed` a un `search --fields parent`. Es una llamada por clave, y es
    /// el precio de no afirmar sin mirar.
    fn parent_of(&self, key: &str) -> Result<Option<String>> {
        let v = acli_json(
            Op::ParentOf,
            Some(key),
            "workitem view --fields parent",
            &["jira", "workitem", "view", key, "--fields", "parent", "--json"],
        )?;
        Ok(v.get("fields")
            .and_then(|f| f.get("parent"))
            .filter(|p| !p.is_null())
            .and_then(|p| p.get("key"))
            .and_then(|k| k.as_str())
            .map(|s| s.to_string()))
    }

    /// `jira epic add` sobre un issue que ya existe, y **se verifica el
    /// efecto**: es la misma razon que en `link` — no confundir "lo intente"
    /// con "esta". Devuelve `false` si ya estaba puesto.
    fn set_parent(&self, key: &str, epic: &str) -> Result<bool> {
        if self.parent_of(key)?.as_deref() == Some(epic) {
            return Ok(false);
        }
        jira(Op::SetParent, Some(key), &["epic", "add", epic, key])?;
        if self.parent_of(key)?.as_deref() != Some(epic) {
            return Err(Failure::new(
                Op::SetParent,
                Some(key),
                format!("`epic add {epic}` no dejo la epica puesta"),
            )
            .into());
        }
        Ok(true)
    }

    /// **Sin verificar el efecto**, y no por olvido: leerlo de vuelta es
    /// `acli jira sprint list-workitems`, que pide `--board` ademas de
    /// `--sprint`. El id del board es configuracion que este sistema todavia
    /// no tiene, y de donde sale la correspondencia entre un sprint del
    /// worklist y uno de Jira es una decision abierta. Mientras tanto lo unico
    /// que hay es el codigo de salida, y esta dicho que no alcanza.
    fn add_to_sprint(&self, sprint: &str, keys: &[&str]) -> Result<usize> {
        for lote in keys.chunks(SPRINT_BATCH) {
            let mut args = vec!["sprint", "add", sprint];
            args.extend_from_slice(lote);
            jira(Op::AddToSprint, None, &args)?;
        }
        Ok(keys.len())
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
