//! La composicion del producto: que items lleva cada sprint y como esta
//! ordenado el backlog.
//!
//! Vive en `.metadata/product.yaml`, **en el panorama y de un solo lado**. Ver
//! `concepts/composition.md`.
//!
//! El `.sprint.md` era tres cosas mezcladas —la composicion, la identidad del
//! proveedor y la prosa— y esto se lleva las dos primeras. La tercera va a
//! graviton.
//!
//! **Se lee con `serde_yaml_ng`, que es el que bilinker ya eligio.** No es una
//! preferencia: el proyecto no elige dos veces lo mismo, y dos parsers de YAML
//! en el mismo arbol son dos comportamientos que pueden diferir en un borde.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Donde vive, y el punto adelante por lo mismo que `_sprints` llevaba `_`:
/// garantiza que nunca choque con un id.
pub const ARCHIVO: &str = ".metadata/product.yaml";

/// Un sprint: lo que se planifica de el, sin una linea de prosa.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sprint {
    pub id: String,
    /// **Menos de 20 caracteres.** Con eso `<id> <nombre>` da 23 como maximo y
    /// el recorte que Jira obligaba no ocurre nunca: el limite deja de ser un
    /// trim y pasa a ser estructural. El titulo largo vive en graviton.
    pub name: String,
    pub status: String,
    /// La coordenada del proveedor. **El unico campo que no escribe una
    /// persona**: lo pone el servidor cuando el sprint existe del otro lado, y
    /// su ausencia significa que todavia no.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Los ids que el sprint referencia **directamente**, y ni uno mas: un item
    /// entra con su subarbol entero, asi que esto nunca nombra una task cuya
    /// user story ya esta en la lista.
    #[serde(default)]
    pub items: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Product {
    #[serde(default)]
    pub sprints: Vec<Sprint>,
    /// El orden del backlog. **Es una lista y no un calculo** porque el orden
    /// es una decision: lo que se calcula es la pertenencia, no la prioridad.
    #[serde(default)]
    pub backlog: Vec<String>,
}

impl Product {
    pub fn sprint(&self, id: &str) -> Option<&Sprint> {
        self.sprints.iter().find(|s| s.id == id)
    }

    /// El sprint en curso, o el proximo `open` por numero.
    ///
    /// Es la primera mitad de *"que sigue"*, y **vive aca y no en el cliente**:
    /// contestarla necesita la composicion entera, que es del servidor.
    pub fn en_curso(&self) -> Option<&Sprint> {
        self.sprints
            .iter()
            .find(|s| s.status == "in-progress")
            .or_else(|| {
                let mut abiertos: Vec<&Sprint> =
                    self.sprints.iter().filter(|s| s.status == "open").collect();
                abiertos.sort_by_key(|s| s.id.parse::<u32>().unwrap_or(u32::MAX));
                abiertos.into_iter().next()
            })
    }

    pub fn to_yaml(&self) -> Result<String> {
        Ok(serde_yaml_ng::to_string(self)?)
    }
}

/// La composicion tal como esta en `rev`.
///
/// **Que falte no es que este vacia.** Un panorama sin `product.yaml` es uno
/// que todavia no migro, y devolver un producto vacio ahi haria que cada
/// ventana se recortara a nada sin que nada lo diga — que es la peor forma de
/// enterarse.
pub fn leer(repo: &Path, rev: &str) -> Result<Product> {
    let texto = crate::git::git_output(repo, &["show", &format!("{rev}:{ARCHIVO}")])
        .with_context(|| format!("{rev} no tiene {ARCHIVO}: este panorama no esta migrado"))?;
    de_yaml(&texto)
}

pub fn de_yaml(texto: &str) -> Result<Product> {
    let p: Product = serde_yaml_ng::from_str(texto).context("leyendo la composicion")?;
    // Dos sprints con el mismo id serian dos respuestas a la misma pregunta, y
    // `sprint()` devolveria la primera sin decir que habia otra.
    let mut vistos = std::collections::HashSet::new();
    for s in &p.sprints {
        if !vistos.insert(s.id.as_str()) {
            bail!("la composicion tiene dos sprints con id `{}`", s.id);
        }
    }
    Ok(p)
}

/// Reemplaza un id en la composicion, donde sea que este.
///
/// **Es otra regla que la del renombre en prosa, y por eso vive aca.** En un
/// `.md` una referencia es un link —`](@o.task.md)`— y se reescribe con el
/// texto; en la composicion es una **entrada de una lista**, sin sintaxis
/// alrededor. Un renombre que sólo mire markdown deja el sprint nombrando un
/// slug que ya no existe, y el proximo recorte falla con *"la composicion
/// nombra a `@o`, y no esta"*.
///
/// Se hace sobre la estructura y no con una expresion regular: `@o` como texto
/// tambien aparece adentro de `@otro`, y ahi el reemplazo textual rompe un id
/// que nadie pidio tocar.
pub fn renombrar_en(repo: &Path, viejo: &str, nuevo: &str) -> Result<bool> {
    let path = repo.join(ARCHIVO);
    let Ok(texto) = std::fs::read_to_string(&path) else { return Ok(false) };
    let mut p = de_yaml(&texto)?;

    let mut cambio = false;
    let mut reemplazar = |lista: &mut Vec<String>| {
        for id in lista.iter_mut() {
            if id == viejo {
                *id = nuevo.to_string();
                cambio = true;
            }
        }
    };
    for s in &mut p.sprints {
        reemplazar(&mut s.items);
    }
    reemplazar(&mut p.backlog);

    if cambio {
        std::fs::write(&path, p.to_yaml()?)?;
    }
    Ok(cambio)
}

/// La clave del proveedor de un sprint, anotada en la composicion.
///
/// **Es el unico campo del sprint que escribe el servidor**, y desde que la
/// composicion existe se anota aca y no en el frontmatter de un archivo: el
/// `.sprint.md` era [el unico que las dos direcciones de la propagacion
/// tocaban](../../../concepts/propagation.md), y el motivo era justamente que
/// mezclaba la clave con la planificacion.
pub fn anotar_key(repo: &Path, sprint_id: &str, key: &str) -> Result<bool> {
    let path = repo.join(ARCHIVO);
    let Ok(texto) = std::fs::read_to_string(&path) else { return Ok(false) };
    let mut p = de_yaml(&texto)?;
    let Some(s) = p.sprints.iter_mut().find(|s| s.id == sprint_id) else { return Ok(false) };
    if s.key.as_deref() == Some(key) {
        return Ok(false);
    }
    s.key = Some(key.to_string());
    std::fs::write(&path, p.to_yaml()?)?;
    Ok(true)
}
