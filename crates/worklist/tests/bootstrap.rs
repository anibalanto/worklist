//! El bootstrap: clave del proveedor para lo que no la tiene, sobre el
//! panorama y sin prometer que la rama se verifique.
//!
//! Es la separacion que pide `5n`: *tener clave* es del item, *verificarse
//! entera* es de la rama. Ver `concepts/sync.md`.

mod common;

use common::{arbol, Spy};
use worklist::assign::bootstrap;

const REF: &str = "refs/heads/insecure/all";

fn correr(dir: &std::path::Path, spy: &Spy) -> Option<worklist::assign::BootstrapResult> {
    bootstrap(dir, REF, "https://x", spy, false).unwrap()
}

#[test]
fn le_da_clave_al_panorama_entero_sin_pedirle_un_sprint() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    let res = correr(&r, &spy).expect("hay cuatro pedidos");

    assert_eq!(res.assigned.len(), 4, "la epica, la US y las dos tasks");
    let listado = common::show_tree(&r, REF);
    assert!(listado.contains("ACC-"), "los archivos quedaron renombrados: {listado}");
    assert!(!listado.contains("\n1.epic.md"), "el slug ya no esta: {listado}");
    // Y nada de lo que promete sincronizacion: ni sprint, ni vinculos.
    assert!(spy.sprinted.borrow().is_empty(), "el bootstrap no toca sprints");
    assert!(spy.blocks.borrow().is_empty(), "ni vinculos");
}

#[test]
fn la_epica_va_primero_y_los_hijos_le_cuelgan() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    let res = correr(&r, &spy).unwrap();

    assert_eq!(res.order[0], "1", "el orden topologico pone la epica adelante");
    let epica = &res.assigned.iter().find(|a| a.slug == "1").unwrap().key;
    for slug in ["n", "o", "q"] {
        let a = res.assigned.iter().find(|a| a.slug == slug).unwrap();
        assert_eq!(a.parent.as_deref(), Some(epica.as_str()), "{slug} cuelga de la epica");
    }
}

#[test]
fn correrlo_de_nuevo_no_encuentra_nada_que_hacer() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    correr(&r, &spy).unwrap();
    let creados = spy.created.borrow().len();

    assert!(bootstrap(&r, REF, "https://x", &spy, false).unwrap().is_none());
    assert_eq!(spy.created.borrow().len(), creados, "no le pidio nada mas al proveedor");
}

/// **Encontrado no es creado.** Sobre un issue que ya existia no hay
/// compare-and-swap detras que pruebe que partimos del estado actual, asi que
/// el cuerpo no viaja: escribirlo seria pisar lo que alguien edito en el board.
#[test]
fn el_cuerpo_viaja_solo_donde_el_issue_se_creo() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy { existing: vec![("o.task.md".into(), "ACC-99".into())], ..Spy::default() };

    let res = correr(&r, &spy).unwrap();

    let encontrado = res.assigned.iter().find(|a| a.slug == "o").unwrap();
    assert_eq!(encontrado.key, "ACC-99");
    assert!(!encontrado.created, "lo encontro, no lo creo");

    let descripciones = spy.descriptions.borrow();
    assert!(
        !descripciones.iter().any(|(k, _)| k == "ACC-99"),
        "sobre lo que ya existia el cuerpo no se toca: {descripciones:?}"
    );
    assert_eq!(descripciones.len(), 3, "y sobre los tres creados si: {descripciones:?}");
}

/// Y el panorama sigue avanzando solo por escritura del servidor: el bootstrap
/// mueve la ref, no empuja.
#[test]
fn deja_la_ref_del_panorama_movida() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    let res = correr(&r, &spy).unwrap();
    assert_ne!(res.new_head, res.old_head);

    let head = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["rev-parse", REF])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(head.trim(), res.new_head);
}
