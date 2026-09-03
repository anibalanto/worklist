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

/// El cuerpo, como ADF, listo para `--description`.
pub fn body_to_adf(body: &str) -> Result<String> {
    let out = convert(body, Format::Gfm, Format::Adf, &Options::default())
        .context("convirtiendo el cuerpo a ADF")?;
    Ok(out.text)
}

/// El ADF de vuelta a markdown — esto es lo que se guarda.
pub fn adf_to_body(adf: &str) -> Result<String> {
    let out = convert(adf, Format::Adf, Format::Gfm, &Options::default())
        .context("convirtiendo el ADF a markdown")?;
    Ok(out.text)
}

/// Toma el archivo entero, devuelve `(adf_para_el_proveedor, archivo_a_guardar)`.
///
/// El frontmatter vuelve intacto, byte a byte: se separa antes de convertir y
/// se vuelve a pegar despues.
pub fn round_trip(text: &str) -> Result<(String, String)> {
    let (frontmatter, body) = split_frontmatter(text);
    let adf = body_to_adf(body)?;
    let back = adf_to_body(&adf)?;
    Ok((adf, format!("{frontmatter}{back}")))
}
