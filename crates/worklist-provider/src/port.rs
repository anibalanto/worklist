//! El puerto hacia el proveedor: **las operaciones que este sistema necesita**,
//! y adentro el transporte que a cada una le toca.
//!
//! El puerto anterior se llamo `Provider` y `Board` y quedo dibujado por lo
//! que `acli` sabe hacer, asi que cuando aparecio algo que `acli` no puede no
//! hubo donde ponerlo. Aca la lista es al reves: la operacion primero, el
//! transporte adentro. Ver `concepts/sync.md` seccion "El puerto son las
//! operaciones, no los comandos".

use anyhow::{bail, Result};
use std::process::Command;

/// Con que se habla. **No es una preferencia**: cada operacion tiene el suyo
/// porque el otro no puede hacerla. `acli` trata crear y editar como
/// vocabularios distintos y el de editar es mas chico — y lo que le falta ahi
/// es justo lo que hace falta para reconciliar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Acli,
    JiraCli,
}

impl Transport {
    /// El binario que la corre. Es tambien lo que se nombra en un fallo: quien
    /// lo lee tiene que saber cual de los dos se quejo.
    pub fn binary(self) -> &'static str {
        match self {
            Transport::Acli => "acli",
            Transport::JiraCli => "jira",
        }
    }
}

/// Las operaciones del puerto. **La lista es cerrada y es la misma tabla que
/// esta en la spec**: quien necesita algo que no figura agrega una variante y
/// dice con que se hace, en vez de elegir un comando en el punto de uso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Buscar por titulo. No hay nada que reconciliar: o esta o no esta.
    Search,
    /// Crear el issue — con `--parent`, que ahi si lo acepta.
    Create,
    SetSummary,
    SetDescription,
    /// Crear y listar vinculos.
    Link,
    /// Leer el estado en vivo de N claves en una llamada.
    Snapshot,
    /// Leer la epica que el proveedor tiene puesta. Por `workitem view`: a un
    /// `search --fields parent` `acli` responde que el campo no esta
    /// permitido.
    ParentOf,
    /// Poner la epica de un issue **que ya existe**. `acli edit` no acepta
    /// `parent`; `jira epic add` lo pone sobre issues ya creados.
    SetParent,
    /// Meter issues en un sprint. `acli edit` no acepta `additionalAttributes`;
    /// `jira sprint add` toma hasta 50 por llamada.
    AddToSprint,
    /// Crear el sprint en el board. Pide el id del board, que es de la
    /// instalacion.
    CreateSprint,
    /// Los sprints que el board ya tiene. Por `jira-cli` porque `acli` no
    /// tiene con que: sus comandos de sprint son `create`, `update`, `view`,
    /// `list-workitems` y `delete`, y ninguno lista los del board.
    SprintList,
    /// Que issues tiene un sprint. Se lee **antes** de agregar, para mandar
    /// solo los que faltan.
    SprintItems,
    /// Mover un issue de estado. **No es una escritura**: el workflow del board
    /// decide si esa transicion es legal, y puede negarse. Ver
    /// `concepts/states.md`.
    Transition,
    /// Las transiciones que el workflow admite hoy para un issue. Se pide
    /// **solo cuando una fue rechazada**: un rechazo que no dice cuales si se
    /// puede no informo nada.
    TransitionsOf,
}

impl Op {
    /// **El reparto, en un solo lugar.** La tabla de `sync.md` vive aca y no
    /// desparramada por los puntos de uso: agregar una operacion es elegir con
    /// un criterio, no improvisar.
    pub fn transport(self) -> Transport {
        match self {
            Op::Search
            | Op::Create
            | Op::SetSummary
            | Op::SetDescription
            | Op::Link
            | Op::Snapshot
            | Op::ParentOf
            | Op::CreateSprint
            | Op::SprintItems
            | Op::Transition
            | Op::TransitionsOf => Transport::Acli,
            Op::SetParent | Op::AddToSprint | Op::SprintList => Transport::JiraCli,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Op::Search => "buscar por titulo",
            Op::Create => "crear el issue",
            Op::SetSummary => "poner el titulo",
            Op::SetDescription => "poner la descripcion",
            Op::Link => "vincular",
            Op::Snapshot => "leer el estado en vivo",
            Op::ParentOf => "leer la epica",
            Op::SetParent => "poner la epica",
            Op::AddToSprint => "meter en el sprint",
            Op::CreateSprint => "crear el sprint",
            Op::SprintList => "listar los sprints del board",
            Op::SprintItems => "leer los issues del sprint",
            Op::Transition => "mover de estado",
            Op::TransitionsOf => "listar las transiciones disponibles",
        }
    }
}

/// Un fallo del proveedor **dicho igual venga de donde venga**: la operacion,
/// la clave sobre la que se intento, el transporte que se quejo y el motivo.
///
/// Los dos transportes mienten distinto —`acli` sale con 0 y pone el fracaso
/// en el cuerpo; de `jira-cli` todavia no se sabe— y normalizarlo es lo que
/// evita que agregar un transporte multiplique los modos de falla que hay que
/// conocer rio arriba. Ver `sync.md` seccion "Dos transportes, dos formas de
/// mentir, una sola respuesta".
#[derive(Debug)]
pub struct Failure {
    pub op: Op,
    /// La clave, cuando la operacion es sobre un issue que ya la tiene. Al
    /// crear todavia no hay, y ahi el titulo es lo unico que identifica.
    pub key: Option<String>,
    pub reason: String,
}

impl Failure {
    pub fn new(op: Op, key: Option<&str>, reason: impl Into<String>) -> Self {
        Failure { op, key: key.map(|s| s.to_string()), reason: reason.into() }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sobre = match &self.key {
            Some(k) => format!(" sobre {k}"),
            None => String::new(),
        };
        write!(
            f,
            "{}{sobre} fallo por {}: {}",
            self.op.name(),
            self.op.transport().binary(),
            self.reason
        )
    }
}

impl std::error::Error for Failure {}

/// La variable de entorno donde va el API token de Atlassian.
pub const TOKEN_ENV: &str = "JIRA_API_TOKEN";

/// Lo que hace falta para que el puerto entero funcione.
///
/// **Se verifica al arrancar, no cuando haga falta.** Sin credencial, la mitad
/// de abajo de la tabla no existe, y descubrirlo en el medio de una ventana a
/// medio resolver es la peor forma de enterarse.
#[derive(Debug)]
pub struct Credentials {
    token: String,
}

impl Credentials {
    pub fn token(&self) -> &str {
        &self.token
    }
}

/// Verifica **las dos** formas de estar autenticado y dice **cual** falta.
///
/// Los transportes autentican distinto: `acli` por su sesion del keyring,
/// `jira-cli` por `JIRA_API_TOKEN`. Asi que "no hay credencial" no es una
/// condicion sola, y un mensaje que no distinga manda a mirar la que ya estaba
/// bien.
pub fn preflight() -> Result<Credentials> {
    let token = std::env::var(TOKEN_ENV).unwrap_or_default();
    let faltan = missing(acli_authenticated(), &token);
    if !faltan.is_empty() {
        bail!("el puerto no puede arrancar, falta credencial:\n  {}", faltan.join("\n  "));
    }
    Ok(Credentials { token })
}

/// Cual de las dos falta, dicho por separado. Es una funcion aparte y sin
/// efectos porque **el mensaje es lo que importa**: uno que diga "falta la
/// credencial" sobre un sistema con dos manda a mirar la que ya estaba bien.
pub fn missing(acli_ok: bool, token: &str) -> Vec<String> {
    let mut faltan = Vec::new();
    if !acli_ok {
        faltan.push(format!(
            "{}: sin sesion — se abre con `acli jira auth login`, y queda en el keyring",
            Transport::Acli.binary()
        ));
    }
    if token.trim().is_empty() {
        faltan.push(format!(
            "jira-cli: falta {TOKEN_ENV} en el entorno — un API token de Atlassian, \
             se crea en id.atlassian.com/manage-profile/security/api-tokens"
        ));
    }
    faltan
}

/// Si `acli` tiene sesion abierta. Que el binario no este es tambien "no
/// autenticado": desde afuera del puerto las dos cosas se arreglan igual.
fn acli_authenticated() -> bool {
    Command::new("acli")
        .args(["jira", "auth", "status"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
