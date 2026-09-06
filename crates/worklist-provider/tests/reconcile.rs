//! Reconciliar: adoptar los issues que existen del otro lado y el panorama no
//! registro. Es la pasada 1 del bootstrap **sin la creacion**.
//!
//! Ver `commands/reconcile.md`.

mod common;

use common::{arbol, Spy};
use worklist_provider::assign::reconcile;

const REF: &str = "refs/heads/insecure/all";

/// Un proveedor que ya tiene issues para algunos titulos. El titulo que `item`
/// escribe es el nombre del archivo.
fn con(existentes: &[(&str, &str)]) -> Spy {
    Spy {
        existing: existentes.iter().map(|(t, k)| (t.to_string(), k.to_string())).collect(),
        ..Default::default()
    }
}

#[test]
fn adopta_lo_que_existe_y_deja_lo_demas_como_estaba() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = con(&[("@1.epic.md", "ACC-14"), ("@o.task.md", "ACC-96")]);

    let res = reconcile(&r, REF, &spy, false).unwrap().expect("hay pedidos");

    assert_eq!(res.adopted.len(), 2, "los dos que el proveedor tenia");
    assert_eq!(res.missing.len(), 2, "los otros dos no existen alla");

    let listado = common::show_tree(&r, REF);
    assert!(listado.contains("ACC-14.epic.md"), "la epica quedo adoptada: {listado}");
    assert!(listado.contains("ACC-96.task.md"), "y la task tambien: {listado}");
    assert!(listado.contains("@n.user-story.md"), "lo que no existe no se toco: {listado}");
}

/// La propiedad entera del comando: reparar no puede crear.
#[test]
fn no_crea_nada_aunque_falte_todo() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    let res = reconcile(&r, REF, &spy, false).unwrap().unwrap();

    assert!(res.adopted.is_empty(), "no habia nada que adoptar");
    assert_eq!(res.missing.len(), 4, "y los cuatro se reportan");
    assert!(spy.created.borrow().is_empty(), "**no se creo un solo issue**");
    // Ni una escritura del otro lado: adoptar es de este lado y nada mas.
    assert!(spy.descriptions.borrow().is_empty(), "el cuerpo no viaja");
    assert!(spy.parents.borrow().is_empty(), "ni el padre");
    assert_eq!(res.new_head, res.old_head, "y la ref no se movio");
}

/// No encontrar es el caso normal, y se nombra uno por uno en vez de contarse.
#[test]
fn lo_que_no_encuentra_lo_nombra_con_su_titulo() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = con(&[("@1.epic.md", "ACC-14")]);

    let res = reconcile(&r, REF, &spy, false).unwrap().unwrap();

    let sin: Vec<&str> = res.missing.iter().map(|m| m.slug.as_str()).collect();
    assert_eq!(sin, vec!["@n", "@o", "@q"], "los tres, por su slug");
    for m in &res.missing {
        assert!(!m.title.is_empty(), "y con su titulo, que es lo que se busco");
    }
}

/// La epica se adopta antes, porque su renombre reescribe lo que la nombra.
#[test]
fn la_epica_va_primero_y_su_renombre_alcanza_a_los_hijos() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = con(&[("@1.epic.md", "ACC-14"), ("@n.user-story.md", "ACC-22")]);

    let res = reconcile(&r, REF, &spy, false).unwrap().unwrap();

    assert_eq!(res.adopted[0].slug, "@1", "la epica primero");
    let us = common::show(&r, REF, "ACC-22.user-story.md");
    assert!(us.contains("parent: ACC-14"), "el parent quedo repuntado:\n{us}");
}

#[test]
fn el_dry_run_pregunta_y_no_escribe() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = con(&[("@1.epic.md", "ACC-14")]);

    let res = reconcile(&r, REF, &spy, true).unwrap().unwrap();

    assert_eq!(res.adopted.len(), 1, "dice cual adoptaria");
    assert_eq!(res.new_head, res.old_head, "y no movio nada");
    assert!(common::show_tree(&r, REF).contains("@1.epic.md"), "el archivo sigue con su slug");
}
