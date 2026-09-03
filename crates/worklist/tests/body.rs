//! La propiedad que sostiene guardar la vuelta en vez de lo empujado: el
//! round-trip converge. Si no convergiera, cada ciclo produciria un diff que
//! nadie escribio y el compare-and-swap empezaria a rechazar de mentira.

use worklist::body::{links_in, links_out, round_trip, split_frontmatter};

const BASE: &str = "https://ejemplo.atlassian.net";

/// `round_trip` mira el repo para reconstruir el tipo de un link; con un
/// directorio vacio, una URL sin archivo correspondiente se queda como URL.
fn vacio() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

const ITEM: &str = "---\ntitle: Con estructura\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\n# Titulo\n\nCon *cursiva*, **negrita** y `SNAKE_CASE` adentro de un code span.\n\n| Campo | Dueño |\n|---|---|\n| `status` | proveedor |\n\n> Un blockquote.\n\n- una lista\n- con dos items\n";

#[test]
fn el_frontmatter_no_pasa_por_el_conversor() {
    let (fm, body) = split_frontmatter(ITEM);
    assert!(fm.starts_with("---\n") && fm.ends_with("---\n"));
    assert!(fm.contains("title: Con estructura"));
    assert!(!body.contains("title:"));

    let (_adf, saved) = round_trip(ITEM, BASE, vacio().path()).unwrap();
    assert!(saved.starts_with(fm), "el frontmatter tiene que volver intacto, byte a byte");
}

#[test]
fn converge_en_una_pasada() {
    let (_, once) = round_trip(ITEM, BASE, vacio().path()).unwrap();
    let (_, twice) = round_trip(&once, BASE, vacio().path()).unwrap();
    assert_eq!(once, twice, "la normalizacion pasa una vez; despues es punto fijo");
}

#[test]
fn la_estructura_llega_al_adf() {
    let (adf, _) = round_trip(ITEM, BASE, vacio().path()).unwrap();
    for node in ["heading", "table", "blockquote", "bulletList"] {
        assert!(adf.contains(node), "falta {node} en el ADF");
    }
}

#[test]
fn snake_case_en_code_span_sobrevive() {
    // Un guion bajo leido como cursiva destrozaria los identificadores de
    // este repo. Es la razon por la que se midio antes de adoptar el crate.
    let (adf, saved) = round_trip(ITEM, BASE, vacio().path()).unwrap();
    assert!(adf.contains("SNAKE_CASE"));
    assert!(saved.contains("`SNAKE_CASE`"));
}

#[test]
fn un_link_a_otro_item_sale_como_url_del_proveedor() {
    let body = "Mira [`ACC-8`](ACC-8.task.md), que va primero.\n";
    let out = links_out(body, BASE);
    assert!(out.contains("](https://ejemplo.atlassian.net/browse/ACC-8)"));
    assert!(!out.contains("ACC-8.task.md"));
}

#[test]
fn y_vuelve_como_nombre_de_archivo_si_el_item_esta_en_la_ventana() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ACC-8.task.md"), "---\ntitle: x\n---\n").unwrap();

    let back = links_in("Mira [`ACC-8`](https://ejemplo.atlassian.net/browse/ACC-8).", BASE, dir.path());
    assert!(back.contains("](ACC-8.task.md)"), "tiene que reconstruir el tipo mirando el archivo");
}

#[test]
fn una_url_a_algo_de_otra_ventana_se_queda_como_url() {
    let dir = tempfile::tempdir().unwrap();
    let texto = "Mira [`ACC-99`](https://ejemplo.atlassian.net/browse/ACC-99).";
    // ACC-99 no existe en este directorio: no hay tipo que reconstruir.
    assert_eq!(links_in(texto, BASE, dir.path()), texto);
}
