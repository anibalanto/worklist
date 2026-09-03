//! El cuerpo del item viaja al proveedor convertido a ADF, y lo que se guarda
//! es la vuelta — no el markdown que llego.
//!
//! Ver `concepts/sync.md` § "El cuerpo viaja, y vuelve convertido". La
//! propiedad que lo hace viable es que la conversion converge en una pasada:
//! guardar la vuelta deja el archivo en forma canonica, y a partir de ahi
//! ningun ciclo produce un diff que nadie escribio.

use amdc::{convert, format::Options, Format};
use anyhow::{Context, Result};

/// Frontmatter y cuerpo. El frontmatter **no** pasa por el conversor: no es
/// markdown ni vive en la descripcion del proveedor.
pub fn split_frontmatter(text: &str) -> (&str, &str) {
    if !text.starts_with("---\n") {
        return ("", text);
    }
    match text[4..].find("\n---\n") {
        Some(i) => text.split_at(4 + i + 5),
        None => ("", text),
    }
}

/// Poda los marks que el schema del proveedor no admite combinados.
///
/// **En ADF `code` es exclusivo**: no convive con `strong` ni con `em`. GFM sí
/// los deja anidar, y un `**`x`**` —negrita sobre un identificador— produce un
/// nodo con los dos. Jira rechaza el documento **entero** con `INVALID_INPUT`,
/// no ese nodo: un solo caso deja la descripcion sin subir.
///
/// Se quedan los `code` y se van los de enfasis, porque el `code` es el que
/// lleva informacion —dice que eso es un identificador— y el enfasis se puede
/// perder sin cambiar lo que la frase significa.
///
/// Esto es de la frontera y no del conversor: que marks se pueden combinar es
/// del vocabulario del proveedor. Ver `concepts/sync.md` seccion "El schema
/// del proveedor poda".
pub fn prune_marks(node: &mut serde_json::Value) {
    const EXCLUDED_BY_CODE: [&str; 2] = ["strong", "em"];

    if let Some(marks) = node.get_mut("marks").and_then(|m| m.as_array_mut()) {
        let has_code = marks
            .iter()
            .any(|m| m.get("type").and_then(|t| t.as_str()) == Some("code"));
        if has_code {
            marks.retain(|m| {
                let t = m.get("type").and_then(|t| t.as_str()).unwrap_or_default();
                !EXCLUDED_BY_CODE.contains(&t)
            });
        }
    }
    if let Some(content) = node.get_mut("content").and_then(|c| c.as_array_mut()) {
        for child in content.iter_mut() {
            prune_marks(child);
        }
    }
}

/// El cuerpo, como ADF, listo para `--description`.
pub fn body_to_adf(body: &str) -> Result<String> {
    let out = convert(body, Format::Gfm, Format::Adf, &Options::default())
        .context("convirtiendo el cuerpo a ADF")?;
    let mut doc: serde_json::Value = serde_json::from_str(&out.text)
        .with_context(|| format!("el ADF que salio del conversor no es JSON: {}", out.text))?;
    prune_marks(&mut doc);
    Ok(serde_json::to_string(&doc)?)
}

/// El ADF de vuelta a markdown — esto es lo que se guarda.
pub fn adf_to_body(adf: &str) -> Result<String> {
    let out = convert(adf, Format::Adf, Format::Gfm, &Options::default())
        .context("convirtiendo el ADF a markdown")?;
    Ok(out.text)
}

/// `<clave>.<tipo>.md` -> `<base>/browse/<clave>`, en los destinos de link.
///
/// En el repo un item referencia a otro por su archivo; en el proveedor eso
/// no significa nada. **No se convierte en un vinculo del proveedor**: una
/// cita en prosa no es una dependencia declarada — para eso esta `relation.*`.
pub fn links_out(body: &str, base: &str) -> String {
    let types = crate::TYPES.join("|");
    let re = regex::Regex::new(&format!(r"\]\(([A-Z]+-\d+)\.(?:{types})\.md\)")).unwrap();
    re.replace_all(body, |c: &regex::Captures| {
        format!("]({}/browse/{})", base.trim_end_matches('/'), &c[1])
    })
    .to_string()
}

/// La vuelta: `<base>/browse/<clave>` -> `<clave>.<tipo>.md`.
///
/// El tipo no esta en la URL: se resuelve mirando que `<clave>.*.md` existe en
/// la ventana. **Si no esta —una referencia a otra ventana— la URL se queda
/// como URL**, que es la forma correcta para algo que no vive aca.
pub fn links_in(body: &str, base: &str, repo: &std::path::Path) -> String {
    let base = regex::escape(base.trim_end_matches('/'));
    let re = regex::Regex::new(&format!(r"\]\({base}/browse/([A-Z]+-\d+)\)")).unwrap();
    re.replace_all(body, |c: &regex::Captures| {
        let key = &c[1];
        match crate::find_file(repo, key) {
            Ok((_, item_type)) => format!("]({key}.{item_type}.md)"),
            Err(_) => c[0].to_string(),
        }
    })
    .to_string()
}

/// Toma el archivo entero, devuelve `(adf_para_el_proveedor, archivo_a_guardar)`.
///
/// El frontmatter vuelve intacto, byte a byte: se separa antes de convertir y
/// se vuelve a pegar despues. `base` y `repo` traducen los links a otros items
/// en el borde: salen como URL del proveedor, vuelven como nombre de archivo.
pub fn round_trip(text: &str, base: &str, repo: &std::path::Path) -> Result<(String, String)> {
    let (frontmatter, body) = split_frontmatter(text);
    let adf = body_to_adf(&links_out(body, base))?;
    let back = links_in(&adf_to_body(&adf)?, base, repo);
    Ok((adf, format!("{frontmatter}{back}")))
}
