//! Asignar una clave real a un pedido: buscar por titulo antes de crear, para
//! que un reintento despues de una falla nunca duplique.

use crate::port::{Failure, Op};
use anyhow::{bail, Context, Result};
use std::process::Command;

/// Como se obtuvo la clave. La distincion existe porque `acli` acepta
/// `--parent` al crear y no al editar: sobre un issue que ya existia, la
/// jerarquia pedida **no viajo en la creacion** y hay que ponerla aparte.
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
    /// Si la jerarquia hay que ponerla en un segundo paso: el issue ya
    /// existia, asi que el `--parent` de la creacion no corrio.
    ///
    /// **No dice que el padre falte** — eso es sobre el board, y sobre el
    /// board no se afirma sin mirarlo. Ver `concepts/sync.md` seccion "Pero
    /// decirlo no es afirmar sobre el board".
    pub fn needs_parent_apart(&self, parent: Option<&str>) -> bool {
        matches!(self, Assignment::Found(_)) && parent.is_some()
    }
}

/// Lo que paso con una transicion pedida.
///
/// **`Rechazada` no es un error**: una transicion ilegal es una respuesta del
/// workflow, no una falla del transporte, y reintentarla la vuelve a rechazar
/// para siempre. Quien llama tiene que poder distinguirla de un rechazo por
/// deriva. Ver `concepts/states.md` seccion "Y hay dos formas de rechazo".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transicion {
    Hecha,
    /// Ya estaba en ese estado. Pedirla de nuevo no es un error.
    YaEstaba,
    Rechazada {
        motivo: String,
        /// Las que el workflow si admite. **`None` es "no se pudieron
        /// listar"**, que no es lo mismo que "no hay ninguna": informar cero
        /// disponibles sin haber podido preguntar seria afirmar sobre el board
        /// sin mirarlo.
        disponibles: Option<Vec<String>>,
    },
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
    /// La clave del issue cuyo `summary` es **exactamente** este titulo, si
    /// existe. **No crea.**
    ///
    /// Es la mitad de `create_or_find` que `reconcile` necesita sola: reparar
    /// se corre cuando algo salio mal, que es cuando menos se quiere estar
    /// decidiendo si ademas se va a escribir. Ver `commands/reconcile.md`.
    fn find(&self, title: &str) -> Result<Option<String>>;
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
    /// El id del sprint del board que se llame asi, creandolo si no esta.
    /// Devuelve `(id, si_lo_creo)`.
    ///
    /// **El id del board es argumento y no un campo del cliente**: un proyecto
    /// puede tener varios boards, asi que guardar uno adentro seria afirmar
    /// algo que no es cierto. Las unicas operaciones que viven en un board son
    /// las de sprint, y son las unicas que lo piden.
    ///
    /// Buscar antes de crear es por lo mismo que `create_or_find`: una corrida
    /// que crea el sprint y se cae antes de anotar su id lo dejo creado y sin
    /// clave, y el reintento lo duplicaria. Ver `concepts/sync.md` seccion "Se
    /// busca por nombre exactamente cuando no hay `key`".
    fn create_or_find_sprint(&self, board: &str, name: &str) -> Result<(String, bool)>;
    /// Mueve el issue al estado que el mapeo pide.
    ///
    /// **Se pide, no se escribe.** El titulo y el cuerpo pisan el valor viejo y
    /// no hay nada que discutir; una transicion tiene reglas del otro lado. De
    /// ahi sale que un cambio de estado sea una propuesta y no un hecho — ver
    /// `commands/state-change.md`.
    fn transition(&self, key: &str, destino: &crate::states::Destino) -> Result<Transicion>;
    /// Las claves que el sprint ya tiene adentro.
    ///
    /// **No es una verificacion**: es para mandar solo lo que falta. Lo que se
    /// pidio no hace falta comprobarlo, porque el codigo de salida de
    /// `jira-cli` es fiel. Ver `concepts/sync.md`.
    fn sprint_items(&self, board: &str, sprint: &str) -> Result<Vec<String>>;
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
/// | `--` (dos o mas guiones seguidos) | la JQL no parsea |
///
/// **Y nada mas.** Puntos, barras, `_`, `:`, `#`, `?`, `+`, `&`, `|`, `!`, `~`
/// y comillas simples estan medidos como seguros, y sacarlos rompe la
/// **tokenizacion**: buscar `bilinker 002 file partition` no encuentra un issue
/// titulado `bilinker-002-file-partition`, y el reintento lo duplica. Ver la
/// task `65`.
///
/// **El guion es el caso fino, y por eso no alcanzaba una lista de
/// caracteres.** Uno solo es seguro y hay que conservarlo, por lo de arriba;
/// dos seguidos rompen el parser. Asi que lo que cae es la **corrida**, no el
/// caracter: `graph --format json` se busca como `graph format json`, que
/// tokeniza igual y encuentra, y `bilinker-002` queda intacto.
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
    let mut chars = title.chars().peekable();
    while let Some(c) = chars.next() {
        // Una corrida de dos o mas guiones cae entera; uno solo se conserva,
        // porque sacarlo rompe la tokenizacion.
        if c == '-' && chars.peek() == Some(&'-') {
            while chars.peek() == Some(&'-') {
                chars.next();
            }
            if !space {
                out.push(' ');
                space = true;
            }
            continue;
        }
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
/// **Su codigo de salida es fiel, y esta leido y no supuesto**: `main`
/// imprime el error en `stderr` y sale con 1, `ExitIfError` hace lo mismo con
/// cualquier error que suba, y la capa de API devuelve error ante cualquier
/// respuesta inesperada. Es al reves que `acli`, que sale con 0 y pone el
/// fracaso en el cuerpo.
///
/// Lo que **no** hay que leer es su `✓`: sobre un proyecto next-gen `epic add`
/// itera issue por issue y lo imprime si al menos uno anduvo, antes de salir
/// con 1. Y el motivo viene en el idioma de quien corre, asi que se reporta y
/// no se matchea. Ver `concepts/sync.md` seccion "Dos transportes, dos formas
/// de mentir, una sola respuesta".
///
/// **Y los argumentos van siempre completos.** `epic add` y `sprint add`
/// preguntan por consola lo que falte en vez de fallar, y un hook no tiene
/// terminal donde contestar: un argumento de menos no es un error, es un
/// proceso colgado.
fn jira(op: Op, key: Option<&str>, args: &[&str]) -> Result<String> {
    let (ok, stdout, fallo) = jira_raw(op, key, args)?;
    if !ok {
        return Err(fallo.into());
    }
    Ok(stdout)
}

/// La corrida cruda: si anduvo, que escribio, y el fracaso ya armado.
///
/// Existe porque hay una operacion —listar— donde el codigo de salida no
/// alcanza para decidir, y necesita ver la salida antes de rendirse.
fn jira_raw(
    op: Op,
    key: Option<&str>,
    args: &[&str],
) -> Result<(bool, String, anyhow::Error)> {
    let out = Command::new("jira").args(args).output().map_err(|e| {
        Failure::new(op, key, format!("no se pudo correr `jira`: {e} — jira-cli no esta instalado"))
    })?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let fallo = Failure::new(
        op,
        key,
        format!("jira {} salio con {:?}: {stderr}{stdout}", args.join(" "), out.status.code()),
    )
    .into();
    Ok((out.status.success(), stdout, fallo))
}

/// Un listado vacio no es un fracaso, y `jira-cli` no los distingue.
///
/// `jira sprint list` sobre un board **sin ningun sprint** escribe
/// `✗ No result found for given query` y sale con 1. Es fiel a *"no encontre
/// nada"* y no a *"algo salio mal"* — y ese es exactamente el estado del que se
/// parte, asi que tratarlo como error vuelve **imposible la primera corrida**
/// sobre cualquier board.
///
/// La distincion se hace sin leer el mensaje, que viene en el idioma de quien
/// corre: **si no salio ninguna fila, no hay nada que leer.** Un fallo que
/// igual imprimio filas sigue siendo un fallo.
///
/// Lo que esto puede confundir es *"no hay ninguno"* con *"no se pudo
/// preguntar"*. El riesgo esta acotado a que la consulta falle de forma
/// transitoria **y** el sprint exista, y ahi se duplicaria. Es el mismo trato
/// que `search_text` ya hace con la JQL: un falso positivo posible pesa menos
/// que un fracaso seguro. Y un problema real reaparece un paso despues, en
/// `sprint create`, con su propio mensaje.
fn jira_listing(op: Op, args: &[&str]) -> Result<String> {
    let (ok, stdout, fallo) = jira_raw(op, None, args)?;
    if listing_survives(ok, &stdout) {
        return Ok(stdout);
    }
    Err(fallo)
}

/// La decision sola, sin proceso de por medio: **un fracaso sin filas es un
/// listado vacio; uno con filas sigue siendo un fracaso.**
///
/// Esta aparte porque es lo unico que hay que poder probar sin un board.
pub fn listing_survives(ok: bool, stdout: &str) -> bool {
    ok || stdout.trim().is_empty()
}

pub struct JiraBoard {
    project: String,
}

impl JiraBoard {
    pub fn new(project: impl Into<String>) -> Self {
        JiraBoard { project: project.into() }
    }

    /// Las transiciones que el workflow admite hoy para este issue.
    ///
    /// Se pide **solo despues de un rechazo**, que es cuando sirve: preguntarla
    /// siempre seria una llamada por item para un dato que casi nunca se usa.
    ///
    /// **Tampoco esta medida contra el board real.** Y por eso su fracaso no se
    /// traga: quien llama convierte el `Err` en `disponibles: None`, que se
    /// lee como *"no se pudieron listar"* y no como *"no hay ninguna"*.
    fn transitions_of(&self, key: &str) -> Result<Vec<String>> {
        let v = acli_json(
            Op::TransitionsOf,
            Some(key),
            "workitem transitions",
            &["jira", "workitem", "transitions", "--key", key, "--json"],
        )?;
        let mut out = Vec::new();
        nombres_en(&v, &mut out);
        Ok(out)
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

    fn find(&self, title: &str) -> Result<Option<String>> {
        self.search(title)
    }

    fn link_relates(&self, a: &str, b: &str) -> Result<bool> {
        self.link(a, b, "Relates")
    }

    /// **La forma exacta de esta llamada no esta medida contra el board real.**
    /// Se escribe con la misma forma que el resto de las escrituras de `acli`
    /// —`workitem` con `--key` y `--json`— y el dia que se corra contra Jira
    /// hay que confirmarla, como se confirmo cada una de las otras. Lo que si
    /// esta decidido es la **forma del resultado**: rechazada no es error.
    fn transition(&self, key: &str, destino: &crate::states::Destino) -> Result<Transicion> {
        let mut args: Vec<String> = vec![
            "jira".into(),
            "workitem".into(),
            "transition".into(),
            "--key".into(),
            key.into(),
            "--status".into(),
            destino.status().into(),
        ];
        // La resolucion es lo que distingue `dropped` de `done`: los dos van a
        // "Done" y en el board se ven distinto solo por este campo.
        if let Some(r) = destino.resolution() {
            args.push("--resolution".into());
            args.push(r.into());
        }
        args.push("--yes".into());
        args.push("--json".into());
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        match acli_json(Op::Transition, Some(key), "workitem transition", &refs) {
            Ok(_) => Ok(Transicion::Hecha),
            // Un fallo de esta operacion es, casi siempre, que el workflow no
            // admite la transicion — que es una respuesta y no una falla. Se
            // pregunta cuales si, y con eso el rechazo informa algo accionable.
            Err(e) => Ok(Transicion::Rechazada {
                motivo: format!("{e}"),
                disponibles: self.transitions_of(key).ok(),
            }),
        }
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

    /// `jira epic add` sobre un issue que ya existe. Devuelve `false` si ya
    /// estaba puesto.
    ///
    /// **No verifica el efecto despues**, y eso esta medido y no supuesto: el
    /// codigo de salida de `jira-cli` es fiel —`ExitIfError` manda el motivo a
    /// `stderr` y sale con 1, y la capa de API devuelve error ante cualquier
    /// respuesta inesperada—. Lo que no es fiel es su `✓`, que sobre un
    /// proyecto next-gen se imprime aunque parte del lote haya fallado; por
    /// eso se mira el codigo y no la salida.
    ///
    /// La lectura de **antes** no es una verificacion: es para no pedir lo que
    /// ya esta, y para que quien reporta sepa como estaba.
    fn set_parent(&self, key: &str, epic: &str) -> Result<bool> {
        if self.parent_of(key)?.as_deref() == Some(epic) {
            return Ok(false);
        }
        jira(Op::SetParent, Some(key), &["epic", "add", epic, key])?;
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

    fn create_or_find_sprint(&self, board: &str, name: &str) -> Result<(String, bool)> {
        if let Some(id) = find_sprint(name)? {
            return Ok((id, false));
        }
        let v = acli_json(
            Op::CreateSprint,
            None,
            "sprint create",
            &["jira", "sprint", "create", "--board", board, "--name", name, "--json"],
        )?;
        // El id sale del JSON de la creacion; si no viniera con esa forma, se
        // lo vuelve a buscar por nombre en vez de fallar. **El sprint ya
        // existe** a esa altura, asi que abortar dejaria creado algo que la
        // proxima corrida duplicaria — el unico error que este camino no puede
        // cometer.
        if let Some(id) = json_id(&v) {
            return Ok((id, true));
        }
        find_sprint(name)?
            .map(|id| (id, true))
            .ok_or_else(|| anyhow::anyhow!("acli sprint create no devolvio 'id' y el sprint {name:?} no aparece en el board: {v}"))
    }

    fn sprint_items(&self, board: &str, sprint: &str) -> Result<Vec<String>> {
        let v = acli_json(
            Op::SprintItems,
            None,
            "sprint list-workitems",
            &[
                "jira", "sprint", "list-workitems",
                "--board", board, "--sprint", sprint,
                "--fields", "key", "--paginate", "--limit", SPRINT_PAGE,
                "--json",
            ],
        )?;
        Ok(keys_in(&v))
    }
}

/// El id de un sprint recien creado, venga como numero o como texto.
fn json_id(v: &serde_json::Value) -> Option<String> {
    let id = v.get("id")?;
    id.as_u64().map(|n| n.to_string()).or_else(|| id.as_str().map(|s| s.to_string()))
}

/// Cuantos issues se piden por pagina al listar un sprint. De mas que el
/// sprint mas grande que este worklist tiene; y si algun dia se quedara corto,
/// lo peor que pasa es re-pedir un `sprint add` de algo que ya estaba, que es
/// lo mismo que hacer nada.
const SPRINT_PAGE: &str = "100";

/// Todas las claves de proveedor que aparecen en un `key` de la respuesta.
///
/// Se recorre el arbol en vez de asumir la envoltura: `list-workitems` es la
/// unica salida de `acli` que este sistema no pudo medir contra el board —no
/// habia ningun sprint con issues adentro contra el cual mirarla— y la forma
/// de `search` no tiene por que ser la suya. Lo que si es seguro es que una
/// clave se reconoce sola.
///
/// **Y la reconoce este archivo, no el core.** `PROJ-123` es la forma de clave
/// de Jira, asi que la pregunta es del proveedor: el core sabe si un id lleva
/// la marca `@`, y eso no alcanza para descartar un `"key": "customfield_1"`
/// que venga en la misma respuesta. Ver `concepts/item.md` § "La marca `@`".
fn es_clave_de_jira(s: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[A-Z]+-\d+$").unwrap()).is_match(s)
}

/// Los `name` que aparezcan en la respuesta, en el orden en que vengan.
///
/// Se recorre el arbol en vez de asumir la envoltura, por lo mismo que
/// `keys_in`: la forma de esta salida no esta medida, y el nombre de una
/// transicion es lo unico que hace falta.
pub(crate) fn nombres_en(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, val) in m {
                if k == "name" {
                    if let Some(s) = val.as_str() {
                        if !out.iter().any(|n| n == s) {
                            out.push(s.to_string());
                        }
                    }
                }
                nombres_en(val, out);
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| nombres_en(x, out)),
        _ => {}
    }
}

fn keys_in(v: &serde_json::Value) -> Vec<String> {
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, val) in m {
                    if k == "key" {
                        if let Some(s) = val.as_str() {
                            if es_clave_de_jira(s) && !out.contains(&s.to_string()) {
                                out.push(s.to_string());
                            }
                        }
                    }
                    walk(val, out);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, &mut out);
    out
}

/// El sprint del board que se llame exactamente asi.
///
/// Por `jira-cli`, que **lee el board de su propia config**: es el unico lugar
/// donde los dos transportes tienen que estar apuntando al mismo board, y que
/// lo esten es del tutorial de instalacion. Si apuntaran a boards distintos
/// esta busqueda no encontraria nada y cada reintento crearia un sprint mas.
///
/// Una fila es `<id>\t<nombre>` y **el id tiene que ser todo digitos**: asi
/// una linea de prosa —el "No result found" de un board vacio, que es el
/// estado del que se parte— no se puede confundir con un sprint.
fn find_sprint(name: &str) -> Result<Option<String>> {
    let out = jira_listing(
        Op::SprintList,
        &[
            "sprint", "list",
            // El default es `active,closed`, y los nuestros nacen `future`.
            "--state", "future,active,closed",
            "--plain", "--no-headers", "--no-truncate",
            "--columns", "ID,NAME",
            "--paginate", "0:100",
        ],
    )?;
    for line in out.lines() {
        let mut campos = line.split('\t');
        let (Some(id), Some(n)) = (campos.next(), campos.next()) else { continue };
        let id = id.trim();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if n.trim() == name {
            return Ok(Some(id.to_string()));
        }
    }
    Ok(None)
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
