//! El tercer transporte: la API REST de Jira.
//!
//! **Una operacion nueva del puerto nace aca.** Los dos CLIs entraron por lo
//! que sabian hacer, asi que cada operacion nueva reabria la pregunta "cual de
//! los dos puede esta"; REST no tiene agujeros conocidos, asi que puede ser el
//! default sin que la pregunta vuelva. Ver ADR-0001.
//!
//! Hoy lleva las dos operaciones de estado, que son las que **ningun CLI
//! puede**: ni `acli` ni `jira-cli` saben listar las transiciones de un issue.

use crate::port::{Credentials, Failure, Op};
use crate::states::Destino;
use anyhow::{Context, Result};

/// La direccion del proveedor y con que se le habla.
///
/// La base es un dato de la instalacion —viaja como argumento del hook, igual
/// que el id del board— y no se adivina.
#[derive(Debug)]
pub struct Api {
    base: String,
    /// `None` es **mudo**: se construyo para un camino que no habla. Ver
    /// `Api::mudo`.
    creds: Option<Credentials>,
}

/// Una transicion del workflow, tal como el proveedor la ofrece.
///
/// **El nombre de la transicion no es el del status al que lleva**: medido en
/// este board, la transicion `Listo` deja el item en `Finalizada`. Por eso los
/// dos campos estan, y por eso quien elige mira `to` y quien pide manda `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disponible {
    pub id: String,
    pub name: String,
    /// El status en el que queda el issue. Es lo que el mapeo de la
    /// instalacion nombra, asi que es contra esto que se busca.
    pub to: String,
}

impl Api {
    pub fn new(base: impl Into<String>, creds: Credentials) -> Self {
        Api { base: base.into().trim_end_matches('/').to_string(), creds: Some(creds) }
    }

    /// Uno que **no puede hablar**, para los caminos que no hablan.
    ///
    /// `--dry-run` arma el board igual que la corrida real y despues no le
    /// pide nada — el plan sale del arbol, no del proveedor. Exigirle
    /// credencial seria pedir configuracion para planificar, que es lo que la
    /// corrida en seco existe para no necesitar.
    ///
    /// **Y no es un `Api` a medias que falle raro**: cualquier llamada falla
    /// diciendo exactamente esto, asi que un camino que crea que no habla y
    /// hable se entera con el motivo puesto, no con un `401`.
    pub fn mudo(base: impl Into<String>) -> Self {
        Api { base: base.into().trim_end_matches('/').to_string(), creds: None }
    }

    /// Las transiciones que el workflow admite **hoy** para este issue.
    ///
    /// Se pide antes de cada intento, porque es de donde sale el id. Lo que
    /// antes se ahorraba —una llamada por rechazo— se paga ahora por cambio de
    /// estado, y a cambio un rechazo por regla siempre puede decir cuales si.
    pub fn transitions_of(&self, key: &str) -> Result<Vec<Disponible>> {
        let v = self.json(
            Op::TransitionsOf,
            Some(key),
            "GET",
            &format!("/rest/api/3/issue/{key}/transitions"),
            None,
        )?;
        let arr = v.get("transitions").and_then(|t| t.as_array()).ok_or_else(|| {
            Failure::new(Op::TransitionsOf, Some(key), "la respuesta no trae `transitions`")
        })?;
        Ok(arr
            .iter()
            .filter_map(|t| {
                Some(Disponible {
                    id: t.get("id")?.as_str()?.to_string(),
                    name: t.get("name")?.as_str().unwrap_or_default().to_string(),
                    to: t.get("to")?.get("name")?.as_str()?.to_string(),
                })
            })
            .collect())
    }

    /// Si el proveedor todavia tiene esta clave, **decidido por el codigo**.
    ///
    /// Su mensaje dice *"no existe o no tienes permiso para verla"* — una sola
    /// frase para dos casos que no se parecen en nada. El status si los
    /// distingue, y de eso depende que un item se saque del arbol.
    pub fn existe(&self, key: &str) -> Result<crate::provider::Existencia> {
        let url = format!("{}/rest/api/3/issue/{key}?fields=key", self.base);
        let Some(creds) = &self.creds else {
            return Err(Failure::new(Op::Snapshot, Some(key), "sin credencial").into());
        };
        let (status, _) = http("GET", &url, &creds.basic_auth(), None)
            .with_context(|| format!("hablando con {url}"))?;
        Ok(match status {
            404 => crate::provider::Existencia::Borrada,
            403 | 401 => crate::provider::Existencia::SinPermiso,
            s if (200..300).contains(&s) => crate::provider::Existencia::Si,
            // Cualquier otra cosa **no es una respuesta**: un 500 no dice que
            // el item no este, y tratarlo como tal lo borraria por un mal dia
            // del servidor.
            s => {
                return Err(Failure::new(
                    Op::Snapshot,
                    Some(key),
                    format!("preguntando si existe salio {s}, que no es ni si ni no"),
                )
                .into())
            }
        })
    }

    /// Los status a los que este issue puede ir hoy.
    ///
    /// Es lo que el compare-and-swap compara, porque el mapeo de la
    /// instalacion habla de status y no de nombres de transicion.
    pub fn reachable_statuses(&self, key: &str) -> Result<Vec<String>> {
        Ok(self.transitions_of(key)?.into_iter().map(|d| d.to).collect())
    }

    /// Mueve el issue al status que el destino pide, **por id de transicion**.
    ///
    /// Devuelve `Ok(None)` cuando el workflow no ofrece ninguna transicion que
    /// lleve a ese status: **eso no es un error, es la respuesta**, y quien
    /// llama la convierte en un rechazo por regla que dice cuales si.
    pub fn transition(&self, key: &str, destino: &Destino) -> Result<Option<Vec<String>>> {
        let disponibles = self.transitions_of(key)?;
        let Some(elegida) = disponibles.iter().find(|d| d.to == destino.status()) else {
            return Ok(Some(disponibles.into_iter().map(|d| d.to).collect()));
        };
        let body = serde_json::json!({ "transition": { "id": elegida.id } });
        self.json(
            Op::Transition,
            Some(key),
            "POST",
            &format!("/rest/api/3/issue/{key}/transitions"),
            Some(body),
        )?;
        Ok(None)
    }

    /// Una llamada, normalizada a la misma forma que el resto del puerto.
    ///
    /// **Un `2xx` sin cuerpo es exito y devuelve `null`.** La transicion
    /// contesta `204`, y tratar la ausencia de JSON como un fallo de parseo
    /// convertiria el caso normal en un error.
    fn json(
        &self,
        op: Op,
        key: Option<&str>,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let url = format!("{}{path}", self.base);
        let Some(creds) = &self.creds else {
            return Err(Failure::new(
                op,
                key,
                "esta corrida se armo sin credencial porque no iba a hablar con el proveedor \
                 — si llego aca, el que decidio que no hablaba se equivoco",
            )
            .into());
        };
        let (status, text) = http(method, &url, &creds.basic_auth(), body)
            .with_context(|| format!("hablando con {url}"))?;
        if !(200..300).contains(&status) {
            return Err(Failure::new(op, key, format!("{method} {path} devolvio {status}: {text}"))
                .into());
        }
        if text.trim().is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_str(&text).map_err(|_| {
            Failure::new(op, key, format!("{method} {path} devolvio {status} y no era JSON: {text}"))
                .into()
        })
    }
}

/// **Lo unico que toca `ureq`.** Aparte para que cambiar de cliente HTTP —o de
/// version mayor— sea cambiar esta funcion y nada mas.
///
/// Un `4xx` o `5xx` no es un `Err`: es una respuesta con su codigo y su
/// cuerpo, y el cuerpo es donde Jira dice que campo no le gusto. `Err` queda
/// para lo que impidio preguntar — DNS, TLS, la conexion.
fn http(
    method: &str,
    url: &str,
    auth: &str,
    body: Option<serde_json::Value>,
) -> Result<(u16, String)> {
    let req = ureq::request(method, url)
        .set("Authorization", auth)
        .set("Accept", "application/json");
    // `send_string` y no `send_json`: el segundo es una feature aparte de
    // `ureq` que solo agrega el serializado, y el cuerpo ya viene armado.
    let res = match body {
        Some(b) => req.set("Content-Type", "application/json").send_string(&b.to_string()),
        None => req.call(),
    };
    match res {
        Ok(r) => {
            let status = r.status();
            Ok((status, r.into_string().unwrap_or_default()))
        }
        Err(ureq::Error::Status(status, r)) => Ok((status, r.into_string().unwrap_or_default())),
        Err(e) => Err(anyhow::anyhow!("{e}")),
    }
}
