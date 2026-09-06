//! El vocabulario de estados y la propuesta que escribe un cambio de estado.
//!
//! Ver `concepts/states.md`.

use worklist_core::states;

#[test]
fn sin_archivo_vale_el_vocabulario_que_worklist_trae() {
    assert_eq!(states::vocabulario(None), states::POR_DEFECTO);
}

#[test]
fn el_proyecto_declara_el_suyo_y_reemplaza_entero() {
    let v = states::vocabulario(Some("states: [open, review, done]\n"));
    assert_eq!(v, vec!["open", "review", "done"]);
    // Y no hereda: `dropped` estaba en el de por defecto y no sobrevive. Un
    // vocabulario a medias es peor que uno chico.
    assert!(!v.iter().any(|s| s == "dropped"));
}

#[test]
fn un_archivo_sin_states_no_deja_al_proyecto_sin_vocabulario() {
    // Fallar hacia lo que worklist trae, no hacia una lista vacia: con cero
    // estados declarados ningun item podria tener uno valido.
    assert_eq!(states::vocabulario(Some("otra_cosa: 1\n")), states::POR_DEFECTO);
    assert_eq!(states::vocabulario(Some("states: []\n")), states::POR_DEFECTO);
}

const ITEM: &str = "---\ntitle: X\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\ncuerpo\n";

#[test]
fn proponer_escribe_el_estado_y_la_fecha_y_devuelve_el_anterior() {
    let (nuevo, anterior) = states::proponer(ITEM, "done", "2026-09-06T20:00:00Z").unwrap();
    assert_eq!(anterior, "open");
    assert!(nuevo.contains("status: done"), "{nuevo}");
    assert!(nuevo.contains("updated_at: 2026-09-06T20:00:00Z"), "{nuevo}");
    assert!(!nuevo.contains("status: open"), "{nuevo}");
}

/// El defecto de la task `5i`, que ya se pago una vez con `parent`: un campo
/// pegado al delimitador convierte todo el archivo en cuerpo.
#[test]
fn el_frontmatter_sigue_cerrando_y_el_cuerpo_queda_intacto() {
    let (nuevo, _) = states::proponer(ITEM, "dropped", "2026-09-06T20:00:00Z").unwrap();
    let (fm, cuerpo) = worklist_core::body::split_frontmatter(&nuevo);
    assert!(fm.ends_with("---\n"), "el frontmatter no cierra: {fm:?}");
    assert!(fm.contains("status: dropped"), "{fm}");
    assert_eq!(cuerpo.trim(), "cuerpo");
}

#[test]
fn un_item_sin_status_no_se_puede_proponer() {
    let sin = "---\ntitle: X\ncreated_at: 2026-09-04T00:00:00Z\n---\n";
    assert!(states::proponer(sin, "done", "2026-09-06T20:00:00Z").is_err());
    assert!(states::proponer("sin frontmatter\n", "done", "x").is_err());
}
