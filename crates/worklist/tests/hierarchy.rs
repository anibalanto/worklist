//! La jerarquia que viaja al proveedor, que no es la del worklist.
//!
//! Jira no admite `parent` entre tipos del mismo nivel, asi que de los tres
//! escalones del worklist entra uno como jerarquia y el otro como link. Ver
//! `concepts/sync.md` seccion "La jerarquia entra hasta donde el proveedor la
//! tiene".

mod common;

use common::{arbol, item, resolve, run, Spy};
use std::cell::RefCell;
use std::path::Path;
use std::process::Command;
use worklist::board::Board;

/// El `--parent` de una task **no** es su user story: es la epica, por lejos
/// que quede. Jira rechaza `Tarea` bajo `Historia`.
#[test]
fn the_parent_sent_is_the_epic_not_the_direct_one() {
    let dir = arbol();
    let spy = Spy::default();
    resolve(&dir.path().join("repo"), &spy);
    let created = spy.created.borrow();
    let epic_key = "ACC-1";
    for (title, _, parent) in created.iter() {
        match title.as_str() {
            "1.epic.md" => assert_eq!(*parent, None, "la epica no cuelga de nada"),
            _ => assert_eq!(
                parent.as_deref(),
                Some(epic_key),
                "{title} tendria que colgar de la epica"
            ),
        }
    }
}

/// El escalon del medio viaja como `Relates`, y **solo** cuando el padre
/// directo no es la epica: una task suelta bajo la epica ya quedo anidada.
#[test]
fn the_middle_step_travels_as_a_relates_link() {
    let dir = arbol();
    let spy = Spy::default();
    let res = resolve(&dir.path().join("repo"), &spy);
    let key_of = |slug: &str| {
        res.assigned.iter().find(|a| a.slug == slug).map(|a| a.key.clone()).unwrap()
    };
    let rel = spy.relates.borrow();
    assert_eq!(
        *rel,
        vec![(key_of("n"), key_of("o"))],
        "solo la task que cuelga de la user story"
    );
}

/// Un item que el proveedor ya tenia no recibe el padre en la creacion
/// —`acli` acepta `--parent` al crear y no al editar—, asi que se pone aparte
/// con el otro transporte. **Se arregla, no se avisa.**
#[test]
fn a_found_item_gets_its_parent_in_a_second_step() {
    let dir = arbol();
    let spy = Spy {
        existing: vec![("o.task.md".into(), "ACC-99".into())],
        ..Default::default()
    };
    let res = resolve(&dir.path().join("repo"), &spy);
    let o = res.assigned.iter().find(|a| a.slug == "o").unwrap();
    assert_eq!(o.key, "ACC-99");
    let epica = o.parent.clone().expect("se le pidio un padre");
    assert!(o.parent_fixed, "y se corrigio en vez de avisar");
    assert_eq!(
        spy.parent_of("ACC-99").unwrap().as_deref(),
        Some(epica.as_str()),
        "el proveedor tiene la epica puesta"
    );

    let epic = res.assigned.iter().find(|a| a.slug == "1").unwrap();
    assert!(!epic.parent_fixed, "la epica no pedia padre");
}

/// Y si el proveedor **ya lo tenia bien**, no se toca ni se reporta: corregir
/// algo que estaba bien es la otra forma de mentir en el reporte.
#[test]
fn a_parent_already_right_is_not_touched_nor_reported() {
    let dir = arbol();
    let spy = Spy {
        existing: vec![("o.task.md".into(), "ACC-99".into())],
        parents: RefCell::new(vec![("ACC-99".into(), "ACC-1".into())]),
        ..Default::default()
    };
    let res = resolve(&dir.path().join("repo"), &spy);
    let o = res.assigned.iter().find(|a| a.slug == "o").unwrap();
    assert_eq!(o.parent.as_deref(), Some("ACC-1"), "la epica es la misma que ya tenia");
    assert!(!o.parent_fixed, "no habia nada que corregir");
}

/// Un ciclo en los `parent` es un error del worklist, y se reporta **antes**
/// de tocar el proveedor: lo caza el orden topologico, que es el primer paso.
/// Nada se crea a medias, y el mensaje nombra el item.
#[test]
fn a_cycle_in_the_parents_is_reported_before_touching_the_provider() {
    let dir = tempfile::tempdir().unwrap();
    let r = &dir.path().join("repo");
    std::fs::create_dir(r).unwrap();
    run(r, &["init", "-q", "-b", "insecure/all"]);
    run(r, &["config", "user.email", "t@t"]);
    run(r, &["config", "user.name", "t"]);
    item(r, "a.task.md", Some("b"));
    item(r, "b.task.md", Some("a"));
    run(r, &["add", "-A"]);
    run(r, &["commit", "-qm", "ciclo"]);
    let spy = Spy::default();
    let rev = String::from_utf8(
        Command::new("git").arg("-C").arg(r).args(["rev-parse", "HEAD"]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let err = worklist::assign::assign_window(
        r,
        "refs/heads/insecure/all",
        worklist::check_push::ALL_ZEROS,
        &rev,
        "https://x",
        &spy,
        "701",
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("ciclo"), "tiene que decir que hay un ciclo: {err}");
    assert!(err.contains('a') || err.contains('b'), "y cual: {err}");
    assert!(
        spy.created.borrow().is_empty(),
        "no se creo nada en el proveedor: {:?}",
        spy.created.borrow()
    );
}

/// El defecto de `5k`: una dependencia que apunta fuera de la ventana no tiene
/// clave que mandar. Se informa y **no aborta**: exigir que toda dependencia
/// caiga adentro seria pedirle al backlog que se ordene por el recorte.
#[test]
fn a_dependency_outside_the_window_is_reported_and_does_not_abort() {
    let dir = tempfile::tempdir().unwrap();
    let r = &dir.path().join("repo");
    std::fs::create_dir(r).unwrap();
    run(r, &["init", "-q", "-b", "insecure/all"]);
    run(r, &["config", "user.email", "t@t"]);
    run(r, &["config", "user.name", "t"]);
    item(r, "1.epic.md", None);
    // `c` depende de `9`, que es de otra ventana, y de `d`, que esta en esta.
    std::fs::write(
        r.join("c.user-story.md"),
        "---\ntitle: c\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nparent: 1\nrelation.depends: [9, d]\n---\n\ncuerpo\n",
    )
    .unwrap();
    item(r, "d.task.md", Some("1"));
    run(r, &["add", "-A"]);
    run(r, &["commit", "-qm", "arbol"]);

    let spy = Spy::default();
    let res = resolve(r, &spy);

    let key_of = |slug: &str| {
        res.assigned.iter().find(|a| a.slug == slug).map(|a| a.key.clone()).unwrap()
    };
    assert_eq!(
        res.untranslated,
        vec![("9".to_string(), key_of("c"))],
        "la que apunta afuera se informa"
    );
    assert_eq!(
        *spy.blocks.borrow(),
        vec![(key_of("d"), key_of("c"))],
        "y la de adentro se crea igual, ya traducida"
    );
    assert_eq!(res.assigned.len(), 3, "la ventana se resolvio entera");
}

// ─── lo que ya tiene clave se actualiza ────────────────────────────────────
//
// Task `60`. El sintoma: editar un item resuelto y empujar dejaba a git y al
// proveedor divergiendo, con el hook informando `sin pedidos`.

/// Un repo donde **todo** ya tiene clave: no hay un solo pedido.
fn resuelto() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path().join("repo");
    std::fs::create_dir(&r).unwrap();
    run(&r, &["init", "-q", "-b", "insecure/all"]);
    run(&r, &["config", "user.email", "t@t"]);
    run(&r, &["config", "user.name", "t"]);
    item(&r, "ACC-1.epic.md", None);
    item(&r, "ACC-2.task.md", Some("ACC-1"));
    item(&r, "ACC-3.task.md", Some("ACC-1"));
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-qm", "todo resuelto"]);
    (dir, r)
}

fn rev(r: &Path, what: &str) -> String {
    String::from_utf8(
        Command::new("git").arg("-C").arg(r).args(["rev-parse", what]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string()
}

fn resolver(r: &Path, old: &str, spy: &Spy) -> Option<worklist::assign::WindowResult> {
    let new = rev(r, "HEAD");
    worklist::assign::assign_window(
        r, "refs/heads/secure/sprint/1", old, &new, "https://x", spy, "701", false,
    )
    .unwrap()
}

/// El caso del ítem: se edita uno que ya tiene clave y el cambio **viaja**.
#[test]
fn editing_a_resolved_item_reaches_the_provider() {
    let (_dir, r) = resuelto();
    let antes = rev(&r, "HEAD");

    let p = r.join("ACC-2.task.md");
    let texto = std::fs::read_to_string(&p).unwrap().replace("cuerpo", "cuerpo editado");
    std::fs::write(&p, texto).unwrap();
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-qm", "edito ACC-2"]);

    let spy = Spy::default();
    let res = resolver(&r, &antes, &spy).expect("hay algo que actualizar");

    assert_eq!(res.updated, vec!["ACC-2".to_string()]);
    assert!(res.assigned.is_empty(), "no habia ningun pedido");
    let descs = spy.descriptions.borrow();
    assert_eq!(descs.len(), 1, "una sola descripcion: {descs:?}");
    assert_eq!(descs[0].0, "ACC-2");
}

/// Y **sólo** el que se tocó: actualizar uno no puede costar ochenta llamadas.
#[test]
fn only_what_the_push_touched_is_updated() {
    let (_dir, r) = resuelto();
    let antes = rev(&r, "HEAD");
    let p = r.join("ACC-3.task.md");
    let texto = std::fs::read_to_string(&p).unwrap().replace("cuerpo", "otra cosa");
    std::fs::write(&p, texto).unwrap();
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-qm", "edito ACC-3"]);

    let spy = Spy::default();
    let res = resolver(&r, &antes, &spy).unwrap();
    assert_eq!(res.updated, vec!["ACC-3".to_string()]);
    assert_eq!(spy.descriptions.borrow().len(), 1, "ni ACC-1 ni ACC-2 se re-subieron");
}

/// Un push que no toca ningún ítem no habla con el proveedor.
#[test]
fn a_push_that_changes_nothing_talks_to_nobody() {
    let (_dir, r) = resuelto();
    let antes = rev(&r, "HEAD");
    std::fs::write(r.join("LEEME.txt"), "no es un item").unwrap();
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-qm", "algo que no es un item"]);

    let spy = Spy::default();
    assert!(resolver(&r, &antes, &spy).is_none(), "nada que resolver ni actualizar");
    assert!(spy.descriptions.borrow().is_empty());
    assert!(spy.summaries.borrow().is_empty());
}

/// El título también sube: es lo que se ve en el board.
#[test]
fn the_title_travels_too() {
    let (_dir, r) = resuelto();
    let antes = rev(&r, "HEAD");
    let p = r.join("ACC-2.task.md");
    let texto = std::fs::read_to_string(&p)
        .unwrap()
        .replace("title: ACC-2.task.md", "title: Un titulo nuevo");
    std::fs::write(&p, texto).unwrap();
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-qm", "cambio el titulo"]);

    let spy = Spy::default();
    resolver(&r, &antes, &spy).unwrap();
    assert_eq!(
        *spy.summaries.borrow(),
        vec![("ACC-2".to_string(), "Un titulo nuevo".to_string())]
    );
}

/// Una rama nueva no tiene con qué comparar: todo lo que trae ya viene
/// resuelto de otra ventana, y no hay nada que actualizar.
#[test]
fn a_brand_new_branch_updates_nothing() {
    let (_dir, r) = resuelto();
    let spy = Spy::default();
    let res = resolver(&r, worklist::check_push::ALL_ZEROS, &spy);
    assert!(res.is_none(), "sin pedidos y sin diff, no hay nada que hacer");
}

/// Un ítem borrado en el mismo push no se intenta actualizar: no hay cuerpo
/// que subir, y borrar el issue no es de este comando.
#[test]
fn an_item_deleted_in_the_push_is_not_updated() {
    let (_dir, r) = resuelto();
    let antes = rev(&r, "HEAD");
    std::fs::remove_file(r.join("ACC-3.task.md")).unwrap();
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-qm", "borro ACC-3"]);

    let spy = Spy::default();
    let res = resolver(&r, &antes, &spy);
    assert!(res.map(|r| r.updated.is_empty()).unwrap_or(true), "no se toco el proveedor");
    assert!(spy.descriptions.borrow().is_empty());
}
