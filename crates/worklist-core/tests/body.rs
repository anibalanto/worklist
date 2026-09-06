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

/// El defecto de `5g`: GFM deja anidar negrita y codigo, ADF no. Lo que sale
/// del conversor tiene los dos marks y Jira rechaza el documento entero.
#[test]
fn code_and_strong_together_lose_the_strong() {
    let adf = worklist::body::body_to_adf("Cargar **`bilinker`** primero.").unwrap();
    let v: serde_json::Value = serde_json::from_str(&adf).unwrap();
    let mut seen = false;
    fn walk(n: &serde_json::Value, seen: &mut bool) {
        if n.get("type").and_then(|t| t.as_str()) == Some("text") {
            let marks: Vec<&str> = n
                .get("marks")
                .and_then(|m| m.as_array())
                .map(|a| a.iter().filter_map(|m| m["type"].as_str()).collect())
                .unwrap_or_default();
            if marks.contains(&"code") {
                *seen = true;
                assert!(!marks.contains(&"strong"), "quedo strong con code: {marks:?}");
            }
        }
        for c in n.get("content").and_then(|c| c.as_array()).into_iter().flatten() {
            walk(c, seen);
        }
    }
    walk(&v, &mut seen);
    assert!(seen, "el caso no se ejercito: no hubo ningun mark code");
}

/// La negrita sola no se toca: la poda es sobre la combinacion, no sobre el
/// enfasis.
#[test]
fn strong_on_its_own_survives() {
    let adf = worklist::body::body_to_adf("Esto es **importante**.").unwrap();
    assert!(adf.contains("strong"), "{adf}");
}

/// Y el codigo solo tampoco.
#[test]
fn code_on_its_own_survives() {
    let adf = worklist::body::body_to_adf("Corre `bilinker check`.").unwrap();
    assert!(adf.contains("code"), "{adf}");
}

/// La poda entra en cualquier profundidad: el caso que rompio el push real
/// estaba dentro de un item de lista.
#[test]
fn the_pruning_reaches_inside_a_list() {
    let adf = worklist::body::body_to_adf("- **`bilinker`** — el prerequisito\n- otra cosa").unwrap();
    let v: serde_json::Value = serde_json::from_str(&adf).unwrap();
    let s = serde_json::to_string(&v).unwrap();
    assert!(s.contains("bulletList"), "el caso no se ejercito: {s}");
    assert!(!s.contains("\"strong\""), "quedo un strong adentro de la lista: {s}");
}
