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

/// Con que se habla.
///
/// Para los dos CLIs **no es una preferencia**: cada operacion tiene el suyo
/// porque el otro no puede hacerla. `acli` trata crear y editar como
/// vocabularios distintos y el de editar es mas chico — y lo que le falta ahi
/// es justo lo que hace falta para reconciliar.
///
/// `Api` si es una preferencia, y es **la** preferencia: una operacion nueva
/// nace ahi salvo que haya razon para lo contrario. Los dos CLIs entraron por
/// lo que sabian hacer, asi que cada operacion nueva reabria la pregunta "cual
/// de los dos puede esta"; REST no tiene agujeros conocidos, asi que puede ser
/// el default sin que la pregunta vuelva. Ver ADR-0001.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Acli,
    JiraCli,
    Api,
}

impl Transport {
    /// Quien la corre, **dicho como lo va a leer quien mire un fallo**: tiene
    /// que saber cual de los tres se quejo.
    ///
    /// Para los CLIs es el nombre del binario. Para `Api` no hay binario, y
    /// nombrar uno seria mandar a mirar un proceso que no existe.
    pub fn quien(self) -> &'static str {
        match self {
            Transport::Acli => "acli",
            Transport::JiraCli => "jira",
            Transport::Api => "la API REST",
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
    ///
    /// Se pide **por id de transicion**, no por nombre de status: en un board
    /// los dos nombres pueden no coincidir —medido: la transicion `Listo` deja
    /// el item en `Finalizada`— y ninguno de los dos CLIs documenta cual
    /// espera. El id sale de `TransitionsOf`.
    Transition,
    /// Las transiciones que el workflow admite hoy para un issue.
    ///
    /// **Precede a cada intento**, porque es de donde sale el id. Antes se
    /// pedia solo tras un rechazo, para no gastar una llamada en un dato que
    /// casi nunca se usa; con la transicion por id el orden se da vuelta, y a
    /// cambio un rechazo por regla **siempre** puede decir cuales si.
    TransitionsOf,
}

impl Op {
    /// **El reparto, en un solo lugar.** La tabla de `sync.md` vive aca y no
    /// desparramada por los puntos de uso: agregar una operacion es elegir con
    /// un criterio, no improvisar.
    ///
    /// **Y lo que se agrega va a `Api`**, salvo razon para lo contrario. Lo que
    /// ya esta medido andando por un CLI se queda: mudarlo cuesta una medicion
    /// nueva y no compra nada. Ver ADR-0001.
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
            | Op::SprintItems => Transport::Acli,
            Op::SetParent | Op::AddToSprint | Op::SprintList => Transport::JiraCli,
            // Ninguno de los dos CLIs puede listar las transiciones de un
            // issue: `acli jira workitem` no tiene el subcomando, y la unica
            // forma de `jira-cli` es un selector interactivo, que un hook no
            // tiene donde contestar. Y sin listar no hay id con que pedir la
            // transicion, asi que las dos van juntas.
            Op::Transition | Op::TransitionsOf => Transport::Api,
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
/// Los transportes mienten distinto —`acli` sale con 0 y pone el fracaso en el
/// cuerpo, `jira-cli` imprime el exito y sale con 1— y normalizarlo es lo que
/// evita que agregar un transporte multiplique los modos de falla que hay que
/// conocer rio arriba. Es lo que hizo que el tercero costara una vez y no una
/// por llamador. Ver `sync.md` seccion "Tres transportes, dos formas de
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
            self.op.transport().quien(),
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
///
/// El token es uno solo para los dos transportes que lo usan: `jira-cli` lo
/// lee del entorno y REST lo manda en `Basic`. Lo que REST agrega no es otro
/// secreto sino **el email**, que no es secreto y por eso no viaja con el
/// token: es un dato de la instalacion, del mismo lado que la URL base y el id
/// del board. Ver ADR-0001 § 5.
#[derive(Debug)]
pub struct Credentials {
    token: String,
    account: String,
}

impl Credentials {
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    /// El header `Authorization` de REST, ya armado.
    ///
    /// Vive aca y no en el cliente HTTP para que **el unico lugar donde el
    /// token se concatena con algo** sea el mismo que lo verifico.
    pub fn basic_auth(&self) -> String {
        format!("Basic {}", base64(format!("{}:{}", self.account, self.token).as_bytes()))
    }
}

/// Verifica **las tres** formas de estar autenticado y dice **cual** falta.
///
/// Los transportes autentican distinto: `acli` por su sesion del keyring,
/// `jira-cli` por `JIRA_API_TOKEN`, REST por el par email mas token. Asi que
/// "no hay credencial" no es una condicion sola, y un mensaje que no distinga
/// manda a mirar la que ya estaba bien.
pub fn preflight(account: &str) -> Result<Credentials> {
    let token = std::env::var(TOKEN_ENV).unwrap_or_default();
    let faltan = missing(acli_authenticated(), &token, account);
    if !faltan.is_empty() {
        bail!("el puerto no puede arrancar, falta credencial:\n  {}", faltan.join("\n  "));
    }
    Ok(Credentials { token, account: account.trim().to_string() })
}

/// Cual de las tres falta, dicho por separado. Es una funcion aparte y sin
/// efectos porque **el mensaje es lo que importa**: uno que diga "falta la
/// credencial" sobre un sistema con tres manda a mirar las dos que ya estaban
/// bien.
pub fn missing(acli_ok: bool, token: &str, account: &str) -> Vec<String> {
    let mut faltan = Vec::new();
    if !acli_ok {
        faltan.push(format!(
            "{}: sin sesion — se abre con `acli jira auth login`, y queda en el keyring",
            Transport::Acli.quien()
        ));
    }
    if token.trim().is_empty() {
        faltan.push(format!(
            "jira-cli y la API REST: falta {TOKEN_ENV} en el entorno — un API token de \
             Atlassian, se crea en id.atlassian.com/manage-profile/security/api-tokens"
        ));
    }
    // El email se dice aparte del token aunque los dos sean de REST: falta por
    // motivos distintos —uno es del entorno de quien empuja, el otro de como
    // se instalaron los hooks— y se arreglan en lugares distintos.
    if account.trim().is_empty() {
        faltan.push(
            "la API REST: falta el email de la cuenta — es `--account` de los hooks, y se \
             pone al instalarlos"
                .to_string(),
        );
    }
    faltan
}

/// Base64 estandar, que es lo unico que `Basic` necesita.
///
/// A mano y no por un crate: son doce lineas contra una dependencia mas, y el
/// alfabeto es fijo desde 1987. Sin `=` de relleno no lo acepta nadie, asi que
/// el padding va.
fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i)) as usize & 0x3f] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    /// El `Basic` de REST es `base64(email:token)`, armado en el mismo lugar
    /// que verifico la credencial. Va aca y no en `tests/` porque construir
    /// una `Credentials` sin pasar por `preflight` es justo lo que el tipo no
    /// deja hacer desde afuera.
    #[test]
    fn el_basic_lleva_el_par_y_no_solo_el_token() {
        let c = Credentials { token: "t0ken".into(), account: "yo@ej.com".into() };
        assert_eq!(c.basic_auth(), "Basic eW9AZWouY29tOnQwa2Vu");
    }

    /// El relleno con `=` no es opcional: sin el, `Basic` no lo acepta nadie.
    /// Los tres largos de resto son los tres casos del algoritmo.
    #[test]
    fn el_base64_rellena() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b"abcd"), "YWJjZA==");
    }
}
