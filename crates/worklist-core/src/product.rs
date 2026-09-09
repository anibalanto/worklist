//! La composicion del producto: que items lleva cada sprint y como esta
//! ordenado el backlog.
//!
//! Vive en `.metadata/product.yaml`, **en el panorama y de un solo lado**. Ver
//! `concepts/composition.md`.
//!
//! El `.sprint.md` era la composicion, la identidad del proveedor, el titulo
//! y la prosa mezclados en un solo archivo. Esto se lleva todo lo operativo
//! —composicion, identidad, titulo—; la prosa del cuerpo no se preserva en
//! ningun lado, y el archivo se borra entero.
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
    /// El titulo tal como se escribio, sin normalizar.
    ///
    /// Es lo unico que hace falta para las dos formas que un sprint necesita
    /// del otro lado: el que crea o busca el sprint en el proveedor arma
    /// `<id> <titulo>` a partir de esto, y lo recorta a 29 porque el limite es
    /// de Jira. Ni la clave: en su interfaz el id de un sprint no se muestra
    /// ni se puede buscar, asi que nombrarlo `6524` lo vuelve imposible de
    /// encontrar.
    pub titulo: String,
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

/// Como `leer`, pero un panorama sin `product.yaml` da una composicion
/// vacia en vez de un error.
///
/// Para donde la ausencia es un estado valido y no "todavia no migro" — un
/// arbol que genuinamente no tiene ni un sprint. `leer` sigue siendo la que
/// corresponde donde la composicion es obligatoria, como cortar una ventana
/// puntual: ahi la ausencia si es un error.
pub fn leer_o_vacio(repo: &Path, rev: &str) -> Result<Product> {
    if crate::git::git_output(repo, &["cat-file", "-e", &format!("{rev}:{ARCHIVO}")]).is_err() {
        return Ok(Product::default());
    }
    leer(repo, rev)
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

/// Le pone a cada sprint de la composicion el titulo que su `.sprint.md`
/// todavia tiene, leyendo `rev`. Es lo unico que faltaba de ese archivo para
/// poder borrarlo — `key`, `status` e `items` ya son de la composicion.
///
/// **Edita, no reconstruye.** A diferencia de una migracion que arma el
/// `Product` desde cero, esto parte de `leer(repo, rev)` y le agrega un campo:
/// `key`/`status`/`items` pueden haber divergido del `.sprint.md` desde que la
/// composicion es la fuente, y reconstruir los pisaria con la copia vieja.
///
/// Un sprint cuyo `.sprint.md` ya no esta en `rev` se deja igual: no todos los
/// sprints tienen ventana, y esto solo agrega lo que encuentra.
pub fn agregar_titulos(repo: &Path, rev: &str) -> Result<Product> {
    let mut p = leer(repo, rev)?;
    for s in &mut p.sprints {
        let file = format!("_sprints/{}.sprint.md", s.id);
        let Ok(texto) = crate::git::git_output(repo, &["show", &format!("{rev}:{file}")]) else {
            continue;
        };
        let titulo = regex::Regex::new(r"(?m)^title:\s*(.+)$")
            .unwrap()
            .captures(&texto)
            .map(|c| c[1].trim().trim_matches('\'').trim_matches('"').to_string())
            .unwrap_or_default();
        s.titulo = titulo;
    }
    Ok(p)
}

/// Saca un id de la composicion, de donde sea que este.
///
/// Va con [`removes`]: sacar el archivo de un item sin sacar la referencia deja
/// el sprint nombrando algo que no esta, y el proximo recorte falla. Devuelve
/// **de donde salio**, porque quien lo corre necesita saber que sprint cambio.
pub fn sacar_de(repo: &Path, id: &str) -> Result<Vec<String>> {
    let path = repo.join(ARCHIVO);
    let Ok(texto) = std::fs::read_to_string(&path) else { return Ok(Vec::new()) };
    let mut p = de_yaml(&texto)?;

    let mut de = Vec::new();
    for s in &mut p.sprints {
        if s.items.iter().any(|i| i == id) {
            s.items.retain(|i| i != id);
            de.push(format!("sprint {}", s.id));
        }
    }
    if p.backlog.iter().any(|i| i == id) {
        p.backlog.retain(|i| i != id);
        de.push("el backlog".into());
    }
    if !de.is_empty() {
        std::fs::write(&path, p.to_yaml()?)?;
    }
    Ok(de)
}
