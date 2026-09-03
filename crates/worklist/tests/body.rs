//! La propiedad que sostiene guardar la vuelta en vez de lo empujado: el
//! round-trip converge. Si no convergiera, cada ciclo produciria un diff que
//! nadie escribio y el compare-and-swap empezaria a rechazar de mentira.

use worklist::body::{round_trip, split_frontmatter};

const ITEM: &str = "---\ntitle: Con estructura\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\n# Titulo\n\nCon *cursiva*, **negrita** y `SNAKE_CASE` adentro de un code span.\n\n| Campo | Dueño |\n|---|---|\n| `status` | proveedor |\n\n> Un blockquote.\n\n- una lista\n- con dos items\n";

#[test]
fn el_frontmatter_no_pasa_por_el_conversor() {
    let (fm, body) = split_frontmatter(ITEM);
    assert!(fm.starts_with("---\n") && fm.ends_with("---\n"));
    assert!(fm.contains("title: Con estructura"));
    assert!(!body.contains("title:"));

    let (_adf, saved) = round_trip(ITEM).unwrap();
    assert!(saved.starts_with(fm), "el frontmatter tiene que volver intacto, byte a byte");
}

#[test]
fn converge_en_una_pasada() {
    let (_, once) = round_trip(ITEM).unwrap();
    let (_, twice) = round_trip(&once).unwrap();
    assert_eq!(once, twice, "la normalizacion pasa una vez; despues es punto fijo");
}

#[test]
fn la_estructura_llega_al_adf() {
    let (adf, _) = round_trip(ITEM).unwrap();
    for node in ["heading", "table", "blockquote", "bulletList"] {
        assert!(adf.contains(node), "falta {node} en el ADF");
    }
}

#[test]
fn snake_case_en_code_span_sobrevive() {
    // Un guion bajo leido como cursiva destrozaria los identificadores de
    // este repo. Es la razon por la que se midio antes de adoptar el crate.
    let (adf, saved) = round_trip(ITEM).unwrap();
    assert!(adf.contains("SNAKE_CASE"));
    assert!(saved.contains("`SNAKE_CASE`"));
}
