//! El vocabulario de estados del proyecto.
//!
//! **Vive en el core y no en el proveedor**, y el corte es el mismo que el de
//! los archivos: el vocabulario es del proyecto —esta en git, en
//! `.metadata/states.yaml`, y viaja con cada recorte— y el **mapeo** a los
//! estados del proveedor es de la instalacion, asi que vive del lado que habla
//! con el.
//!
//! De ahi sale que el cliente pueda validar un estado sin enlazar el proveedor:
//! saber que `done` existe es de este lado; saber que del otro lado se llama
//! `"Done"` no. Ver `concepts/states.md`.

/// Los estados que worklist trae si el proyecto no declara otros.
pub const POR_DEFECTO: &[&str] = &["open", "in-progress", "done", "dropped"];

/// El estado en el que se pone un item que se saca del arbol.
pub const DESCARTADO: &str = "dropped";

/// Donde vive el vocabulario, adentro del panorama.
pub const ARCHIVO: &str = ".metadata/states.yaml";

/// El `states: [...]` del archivo. Sin archivo, el vocabulario por defecto: un
/// proyecto que no declara nada usa el que worklist trae.
pub fn vocabulario(texto: Option<&str>) -> Vec<String> {
    let Some(texto) = texto else { return por_defecto() };
    // La misma forma que el `items:` de un sprint, y por la misma razon: es lo
    // que ya se sabe leer de un frontmatter sin traer un parser de YAML.
    let re = regex::Regex::new(r"(?m)^states:\s*\[([^\]]*)\]").unwrap();
    let Some(c) = re.captures(texto) else { return por_defecto() };
    let out: Vec<String> = c[1]
        .split(',')
        .map(|s| s.trim().trim_matches(['"', '\'']).to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if out.is_empty() {
        por_defecto()
    } else {
        out
    }
}

fn por_defecto() -> Vec<String> {
    POR_DEFECTO.iter().map(|s| s.to_string()).collect()
}

/// El vocabulario de la vista donde uno esta parado.
///
/// **Se lee de la vista y no del panorama**, que es lo que hacia antes: el
/// panorama ya no esta del lado del cliente, asi que `insecure/all` no es una
/// ref que este a mano. El vocabulario sigue siendo del proyecto entero —lo
/// que cambio es de donde se lo lee—, y para eso `window_files` lo mete en
/// cada recorte. Ver `concepts/states.md`.
pub fn de_la_vista(repo: &std::path::Path) -> Option<String> {
    let out = crate::git_command(repo)
        .args(["show", &format!("HEAD:{ARCHIVO}")])
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Escribe el estado propuesto en el frontmatter. Devuelve el texto nuevo y el
/// estado que habia.
///
/// **Reemplaza dos lineas que ya existen**, y por eso no puede repetir el
/// defecto de pegar un campo contra el delimitador de cierre: `status` y
/// `updated_at` son obligatorios, asi que no hay nada que insertar.
pub fn proponer(text: &str, nuevo: &str, ahora: &str) -> anyhow::Result<(String, String)> {
    let (fm, resto) = crate::body::split_frontmatter(text);
    if fm.is_empty() {
        anyhow::bail!("el item no tiene frontmatter");
    }
    let re_status = regex::Regex::new(r"(?m)^status:[ \t]*(\S+)[ \t]*$").unwrap();
    let anterior = re_status
        .captures(fm)
        .map(|c| c[1].to_string())
        .ok_or_else(|| anyhow::anyhow!("el item no declara `status`"))?;
    let fm = re_status.replace(fm, format!("status: {nuevo}")).to_string();
    let re_updated = regex::Regex::new(r"(?m)^updated_at:[ \t]*\S+[ \t]*$").unwrap();
    let fm = re_updated.replace(&fm, format!("updated_at: {ahora}")).to_string();
    Ok((format!("{fm}{resto}"), anterior))
}
