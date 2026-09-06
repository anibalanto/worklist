//! La invariante del corte, verificada y no confiada.
//!
//! Que el cliente no pueda escribir en el proveedor es **una linea de un
//! `Cargo.toml`**: el dia que alguien agregue `worklist-provider` a las
//! dependencias, el corte se deshace sin que nada lo diga y sin que ningun
//! otro test se ponga rojo — el codigo compilaria igual.
//!
//! Ver `concepts/distribution.md`.

/// Mira **las secciones de dependencias**, no el archivo entero: el manifiesto
/// nombra a `worklist-provider` en un comentario, justamente para decir que no
/// lo enlaza. Un test que busca la cadena suelta se enciende con eso — y lo
/// hizo, la primera vez que corrio.
fn dependencias(manifest: &str) -> String {
    let mut dentro = false;
    let mut out = String::new();
    for linea in manifest.lines() {
        let l = linea.trim();
        if l.starts_with('[') {
            dentro = l.contains("dependencies");
            continue;
        }
        if dentro {
            let sin_comentario = l.split('#').next().unwrap_or("");
            out.push_str(sin_comentario);
            out.push('\n');
        }
    }
    out
}

#[test]
fn el_cliente_no_enlaza_el_proveedor() {
    let manifest = include_str!("../Cargo.toml");
    assert!(
        !dependencias(manifest).contains("worklist-provider"),
        "worklist-cli enlazo worklist-provider: el binario del cliente pasa a tener \
         el codigo que habla con el proveedor, y la invariante de \
         concepts/distribution.md deja de sostenerse.\n\n\
         Lo que el cliente necesita del proveedor se lo pide al servidor."
    );
}
