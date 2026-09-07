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
    /// **El numero y el titulo, en minuscula y con guiones medios. Entero.**
    ///
    /// Sin tope: si recortar rompe el nombre, lo que sobra es el tope y no el
    /// titulo. Y **no es el nombre que ve el proveedor** —ese es `<id>
    /// <titulo>` y sigue recortandose a 29, porque el limite es de Jira— ni la
    /// clave: en su interfaz el id de un sprint no se muestra ni se puede
    /// buscar, asi que nombrarlo `6524` lo vuelve imposible de encontrar.
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

/// El nombre de un sprint a partir de su numero y su titulo.
///
/// Mecanico a proposito: es lo que hace que migrar 22 sprints no sean 22
/// decisiones a mano. Ver `concepts/composition.md`.
pub fn nombre(id: &str, titulo: &str) -> String {
    let mut out = format!("{id}-");
    let mut guion = false;
    for c in titulo.chars() {
        // Los acentos se bajan a su letra: un nombre es un identificador que
        // alguien tipea, y `migracion` se tipea mas facil que `migración`.
        let c = match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            otro => otro,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            guion = false;
        } else if !guion && !out.ends_with('-') {
            out.push('-');
            guion = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Arma la composicion leyendo los `_sprints/*.sprint.md` que haya en `rev`.
///
/// Es la migracion de `ACC-304`, y es **mecanica**: el nombre sale del titulo
/// entero, el `key`, el `status` y el `items` se copian. La prosa del cuerpo no
/// se toca — se va a graviton, que es otra mitad.
///
/// **No escribe nada**: devuelve la composicion y quien llama decide. Migrar el
/// panorama es una escritura del servidor.
pub fn desde_los_sprint_md(repo: &Path, rev: &str) -> Result<Product> {
    let listing = crate::git::git_output(repo, &["ls-tree", "-r", "--name-only", rev])?;
    let mut p = Product::default();
    for name in listing.lines() {
        let Some(id) = name
            .strip_prefix("_sprints/")
            .and_then(|n| n.strip_suffix(".sprint.md"))
        else {
            continue;
        };
        let texto = crate::git::git_output(repo, &["show", &format!("{rev}:{name}")])?;
        let campo = |k: &str| {
            regex::Regex::new(&format!(r"(?m)^{k}:\s*(.+)$"))
                .unwrap()
                .captures(&texto)
                .map(|c| c[1].trim().trim_matches('\'').trim_matches('"').to_string())
        };
        let titulo = campo("title").unwrap_or_default();
        let items = regex::Regex::new(r"(?m)^items:\s*\[([^\]]*)\]")
            .unwrap()
            .captures(&texto)
            .map(|c| {
                regex::Regex::new(r"@?[A-Za-z0-9_-]+")
                    .unwrap()
                    .find_iter(&c[1])
                    .map(|m| m.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default();
        p.sprints.push(Sprint {
            id: id.to_string(),
            name: nombre(id, &titulo),
            status: campo("status").unwrap_or_else(|| "open".into()),
            key: campo("key"),
            items,
        });
    }
    // Por numero, que es como se los nombra y como se los lee.
    p.sprints.sort_by_key(|s| s.id.parse::<u32>().unwrap_or(u32::MAX));
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
