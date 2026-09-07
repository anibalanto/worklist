//! Lo que una ventana resolvio sube al panorama, y el panorama sigue siendo
//! el panorama: no pierde lo que el recorte dejo afuera.
//!
//! Ver `concepts/propagation.md`.

mod common;

use common::{item, run, show, show_tree, sprint, Spy};
use std::path::Path;
use worklist_provider::propagate::{propagate, propagated_ref, Step};

/// Cualquiera sirve: los tests de acá no traducen links a ningún host.
const BASE: &str = "https://ejemplo.atlassian.net";

fn rev(repo: &Path, refname: &str) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", refname])
        .output()
        .unwrap();
    assert!(out.status.success(), "git rev-parse {refname}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}


/// Un item con cuerpo propio, para poder citar a otro en la prosa.
fn item_con(repo: &Path, name: &str, parent: Option<&str>, cuerpo: &str) {
    let p = match parent {
        Some(p) => format!("parent: {p}\n"),
        None => String::new(),
    };
    std::fs::write(
        repo.join(name),
        format!("---\ntitle: {name}\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n{p}---\n\n{cuerpo}\n"),
    )
    .unwrap();
}

/// `@1.epic` con dos tasks: `@o` entra al sprint 1, `@q` se queda afuera y **lo
/// cita en la prosa**. Es el caso que separa copiar el renombre de rehacerlo.
fn arbol_con_cita() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = &dir.path().join("repo");
    std::fs::create_dir(r).unwrap();
    run(r, &["init", "-q", "-b", "insecure/all"]);
    run(r, &["config", "user.email", "t@t"]);
    run(r, &["config", "user.name", "t"]);
    item(r, "@1.epic.md", None);
    item(r, "@o.task.md", Some("@1"));
    item_con(r, "@q.task.md", Some("@1"), "sale de [`@o`](@o.task.md), que esta en el sprint 1");
    sprint(r, "1", "el primero", &["@o"], None);
    run(r, &["add", "-A"]);
    run(r, &["commit", "-qm", "arbol"]);
    dir
}

#[test]
fn el_trabajo_de_la_ventana_sube_y_el_panorama_no_pierde_lo_recortado() {
    let dir = arbol_con_cita();
    let r = &dir.path().join("repo");
    worklist_core::window::open(r, "1", "insecure/all", false, false).unwrap();

    // Alguien edita adentro de su ventana y empuja.
    let wt = dir.path().join("w");
    run(r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/1"]);
    item_con(&wt, "@o.task.md", Some("@1"), "cuerpo editado en la ventana");
    run(&wt, &["commit", "-aqm", "edito o"]);
    run(r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    let tip = rev(r, "refs/heads/secure/sprint/1");
    let p = propagate(r, "refs/heads/secure/sprint/1", &tip, BASE, false).unwrap().unwrap();

    assert_eq!(p.steps.len(), 1, "sube el commit de trabajo y no el corte");
    assert!(show(r, "insecure/all", "@o.task.md").contains("editado en la ventana"));
    // Y lo que la ventana no tiene sigue estando: propagar no recorta.
    assert!(show(r, "insecure/all", "@q.task.md").contains("sale de"));
    assert_eq!(rev(r, &propagated_ref("refs/heads/secure/sprint/1")), tip);
}

#[test]
fn el_renombre_se_rehace_y_corrige_las_referencias_de_afuera_del_recorte() {
    let dir = arbol_con_cita();
    let r = &dir.path().join("repo");
    worklist_core::window::open(r, "1", "insecure/all", false, false).unwrap();

    let tip = rev(r, "refs/heads/secure/sprint/1");
    let spy = Spy::default();
    worklist_provider::assign::assign_window(
        r,
        "refs/heads/secure/sprint/1",
        worklist_provider::check_push::ALL_ZEROS,
        &tip,
        "https://x",
        &spy,
        "701",
        false,
    )
    .unwrap()
    .unwrap();

    let tip = rev(r, "refs/heads/secure/sprint/1");
    let p = propagate(r, "refs/heads/secure/sprint/1", &tip, BASE, false).unwrap().unwrap();

    let rehechos: Vec<&Step> =
        p.steps.iter().filter(|s| matches!(s, Step::Renamed { .. })).collect();
    assert!(!rehechos.is_empty(), "el renombre tiene que rehacerse, no copiarse");

    // La clave que le toco a `o`, leida del arbol del panorama.
    let listado = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(r)
            .args(["ls-tree", "-r", "--name-only", "insecure/all"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let clave = listado
        .lines()
        .find(|n| n.starts_with("ACC-") && n.ends_with(".task.md") && !n.contains("q"))
        .map(|n| n.trim_end_matches(".task.md").to_string())
        .expect("el panorama tiene el archivo renombrado");

    assert!(!listado.contains("\n@o.task.md"), "el slug ya no esta en el panorama");
    // Lo que este caso existe para probar: la cita vive **afuera** del recorte,
    // asi que el commit de la ventana no la tocaba.
    let q = show(r, "insecure/all", "@q.task.md");
    assert!(q.contains(&format!("{clave}.task.md")), "la cita de afuera quedo sin corregir: {q}");
    assert!(!q.contains("(@o.task.md)"), "quedo apuntando al slug viejo: {q}");
}

#[test]
fn propagar_dos_veces_no_hace_nada_la_segunda() {
    let dir = arbol_con_cita();
    let r = &dir.path().join("repo");
    worklist_core::window::open(r, "1", "insecure/all", false, false).unwrap();

    let wt = dir.path().join("w");
    run(r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/1"]);
    item_con(&wt, "@o.task.md", Some("@1"), "editado");
    run(&wt, &["commit", "-aqm", "edito o"]);
    run(r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    let tip = rev(r, "refs/heads/secure/sprint/1");
    propagate(r, "refs/heads/secure/sprint/1", &tip, BASE, false).unwrap().unwrap();
    let panorama = rev(r, "insecure/all");

    assert!(propagate(r, "refs/heads/secure/sprint/1", &tip, BASE, false).unwrap().is_none());
    assert_eq!(rev(r, "insecure/all"), panorama, "la segunda no mueve nada");
}

#[test]
fn un_choque_no_mueve_el_panorama_ni_la_marca() {
    let dir = arbol_con_cita();
    let r = &dir.path().join("repo");
    worklist_core::window::open(r, "1", "insecure/all", false, false).unwrap();

    // La ventana escribe una cosa…
    let wt = dir.path().join("w");
    run(r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/1"]);
    item_con(&wt, "@o.task.md", Some("@1"), "lo que dice la ventana");
    run(&wt, &["commit", "-aqm", "edito o en la ventana"]);
    run(r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    // …y el panorama otra, sobre el mismo archivo.
    item_con(r, "@o.task.md", Some("@1"), "lo que dice el panorama");
    run(r, &["commit", "-aqm", "edito o en el panorama"]);

    let panorama = rev(r, "insecure/all");
    let tip = rev(r, "refs/heads/secure/sprint/1");
    let e = propagate(r, "refs/heads/secure/sprint/1", &tip, BASE, false).unwrap_err();

    let msg = format!("{e:#}");
    assert!(msg.contains("@o.task.md"), "el mensaje dice en que archivo choco: {msg}");
    assert_eq!(rev(r, "insecure/all"), panorama, "el panorama no avanza a medias");
    // La marca la deja el recorte apuntando al corte, que es *"nada subio
    // todavia"* dicho explicitamente en vez de por ausencia. Lo que importa es
    // que **no se movio**: sigue abajo del trabajo, no en el tip.
    let marca = rev(r, &propagated_ref("refs/heads/secure/sprint/1"));
    assert_ne!(marca, tip, "la marca no se mueve si el panorama no recibio");
    assert_eq!(
        marca,
        worklist_core::git::cut_commit(r, "refs/heads/secure/sprint/1", "insecure/all").unwrap(),
        "y sigue en el corte, que es de donde `pending` arranca"
    );
}

#[test]
fn sin_el_corte_no_se_propaga_nada() {
    let dir = arbol_con_cita();
    let r = &dir.path().join("repo");
    // Una rama segura que **no** se corto: nacio de un checkout, asi que su
    // primer commit sobre el panorama no es un `window:`.
    run(r, &["branch", "secure/sprint/1", "insecure/all"]);
    let wt = dir.path().join("w");
    run(r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/1"]);
    item_con(&wt, "@o.task.md", Some("@1"), "editado");
    run(&wt, &["commit", "-aqm", "edito o"]);
    run(r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    let tip = rev(r, "refs/heads/secure/sprint/1");
    let e = propagate(r, "refs/heads/secure/sprint/1", &tip, BASE, false).unwrap_err();
    assert!(format!("{e:#}").contains("no encuentro el corte"));
}

/// El paso del `pre-receive`: probar el cherry-pick sin aplicarlo. Es lo que
/// convierte "el conflicto se ve" en "el push se rechaza", y lo que evita
/// tener que anotar un conflicto en el tronco.
#[test]
fn el_prechequeo_ve_el_choque_sin_escribir_nada() {
    let dir = arbol_con_cita();
    let r = &dir.path().join("repo");
    worklist_core::window::open(r, "1", "insecure/all", false, false).unwrap();

    let wt = dir.path().join("w");
    run(r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/1"]);
    item_con(&wt, "@o.task.md", Some("@1"), "lo que dice la ventana");
    run(&wt, &["commit", "-aqm", "edito o en la ventana"]);
    run(r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    item_con(r, "@o.task.md", Some("@1"), "lo que dice el panorama");
    run(r, &["commit", "-aqm", "edito o en el panorama"]);

    let panorama = rev(r, "insecure/all");
    let tip = rev(r, "refs/heads/secure/sprint/1");
    let worklist_provider::propagate::Verdict::Conflict { files, .. } =
        worklist_provider::propagate::would_conflict(r, "refs/heads/secure/sprint/1", &tip).unwrap()
    else {
        panic!("el prechequeo tiene que verlo")
    };
    assert_eq!(files, vec!["@o.task.md".to_string()]);
    assert_eq!(rev(r, "insecure/all"), panorama, "probar no escribe");
}

/// Y no rechaza lo que si entra: dos ventanas que tocan archivos distintos no
/// se estorban, que es el caso normal.
#[test]
fn el_prechequeo_deja_pasar_lo_que_entra() {
    let dir = arbol_con_cita();
    let r = &dir.path().join("repo");
    worklist_core::window::open(r, "1", "insecure/all", false, false).unwrap();

    let wt = dir.path().join("w");
    run(r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/1"]);
    item_con(&wt, "@o.task.md", Some("@1"), "lo que dice la ventana");
    run(&wt, &["commit", "-aqm", "edito o"]);
    run(r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    // El panorama avanza sobre otro archivo.
    item_con(r, "@q.task.md", Some("@1"), "otra cosa, en otro archivo");
    run(r, &["commit", "-aqm", "edito q en el panorama"]);

    let tip = rev(r, "refs/heads/secure/sprint/1");
    assert!(matches!(
        worklist_provider::propagate::would_conflict(r, "refs/heads/secure/sprint/1", &tip).unwrap(),
        worklist_provider::propagate::Verdict::Applies
    ));
    // Y de hecho entra.
    propagate(r, "refs/heads/secure/sprint/1", &tip, BASE, false).unwrap().unwrap();
    assert!(show(r, "insecure/all", "@o.task.md").contains("lo que dice la ventana"));
    assert!(show(r, "insecure/all", "@q.task.md").contains("otra cosa"));
}

/// **No poder probar no es que entre.** El repo de la instalacion no tiene
/// panorama —16 ventanas y ningun `insecure/all`—, y ahi devolver "entra"
/// seria aceptar en silencio algo que nadie miro. Ver la task `77`.
#[test]
fn sin_panorama_el_prechequeo_lo_dice_en_vez_de_dejar_pasar() {
    let dir = arbol_con_cita();
    let r = &dir.path().join("repo");
    worklist_core::window::open(r, "1", "insecure/all", false, false).unwrap();
    let tip = rev(r, "refs/heads/secure/sprint/1");

    // El servidor que solo recibio ventanas: el panorama nunca llego.
    run(r, &["checkout", "-q", "--detach"]);
    run(r, &["branch", "-q", "-D", "insecure/all"]);

    assert!(matches!(
        worklist_provider::propagate::would_conflict(r, "refs/heads/secure/sprint/1", &tip).unwrap(),
        worklist_provider::propagate::Verdict::NoPanorama
    ));
}

/// Sin estar parado en el panorama —un bare, o un HEAD suelto— el trabajo va a
/// un worktree temporal y la ref se mueve al final. Es el camino del servidor.
#[test]
fn desde_afuera_del_panorama_usa_un_worktree_temporal() {
    let dir = arbol_con_cita();
    let r = &dir.path().join("repo");
    worklist_core::window::open(r, "1", "insecure/all", false, false).unwrap();

    let wt = dir.path().join("w");
    run(r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/1"]);
    item_con(&wt, "@o.task.md", Some("@1"), "editado en la ventana");
    run(&wt, &["commit", "-aqm", "edito o"]);
    run(r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    // Nadie tiene el panorama abierto.
    run(r, &["checkout", "-q", "--detach"]);

    let tip = rev(r, "refs/heads/secure/sprint/1");
    let p = propagate(r, "refs/heads/secure/sprint/1", &tip, BASE, false).unwrap().unwrap();

    assert_eq!(p.steps.len(), 1);
    assert!(show(r, "insecure/all", "@o.task.md").contains("editado en la ventana"));
    assert_eq!(rev(r, "insecure/all"), p.panorama_new, "la ref quedo movida");
}

/// Y no la mueve si otro worktree la tiene abierta: ese indice quedaria
/// apuntando al arbol anterior, y el sintoma no dice la causa.
#[test]
fn no_mueve_el_panorama_que_otro_worktree_tiene_abierto() {
    let dir = arbol_con_cita();
    let r = &dir.path().join("repo");
    worklist_core::window::open(r, "1", "insecure/all", false, false).unwrap();

    let wt = dir.path().join("w");
    run(r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/1"]);
    item_con(&wt, "@o.task.md", Some("@1"), "editado");
    run(&wt, &["commit", "-aqm", "edito o"]);
    run(r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    run(r, &["checkout", "-q", "--detach"]);
    let otro = dir.path().join("otro");
    run(r, &["worktree", "add", "-q", otro.to_str().unwrap(), "insecure/all"]);

    let panorama = rev(r, "insecure/all");
    let tip = rev(r, "refs/heads/secure/sprint/1");
    let e = propagate(r, "refs/heads/secure/sprint/1", &tip, BASE, false).unwrap_err().to_string();

    assert!(e.contains("otro worktree"), "{e}");
    assert_eq!(rev(r, "insecure/all"), panorama, "no se movio");
}

/// La épica cita a sus dos tasks, y cada una entra a un sprint distinto. Es el
/// caso que separa copiar el `normalize:` de rehacerlo: cada ventana ve una de
/// las dos citas resuelta y la otra no.
fn arbol_con_dos_ventanas() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = &dir.path().join("repo");
    std::fs::create_dir(r).unwrap();
    run(r, &["init", "-q", "-b", "insecure/all"]);
    run(r, &["config", "user.email", "t@t"]);
    run(r, &["config", "user.name", "t"]);
    item_con(
        r,
        "@1.epic.md",
        None,
        "la epica nombra a [`@o`](@o.task.md) y a [`@q`](@q.task.md), y ninguna ventana ve las dos",
    );
    item(r, "@o.task.md", Some("@1"));
    item(r, "@q.task.md", Some("@1"));
    sprint(r, "1", "el primero", &["@o"], None);
    sprint(r, "2", "el segundo", &["@q"], None);
    run(r, &["add", "-A"]);
    run(r, &["commit", "-qm", "arbol"]);
    dir
}

/// El caso que hizo caer `propagate --all-windows` sobre el repo real: ocho
/// ventanas habían normalizado la misma épica, cada una con su renombre
/// parcial, y el cherry-pick de la segunda chocaba contra la primera.
#[test]
fn el_normalize_de_dos_ventanas_sobre_el_mismo_ancestro_no_choca() {
    let dir = arbol_con_dos_ventanas();
    let r = &dir.path().join("repo");

    let ramas = ["refs/heads/secure/sprint/1", "refs/heads/secure/sprint/2"];

    // Las dos se cortan **antes** de propagar nada, que es como estaban las 16
    // del repo real: cada una ve el panorama sin claves, y resuelve la suya.
    let spy = Spy::default();
    for (sprint_id, rama) in ["1", "2"].iter().zip(ramas) {
        worklist_core::window::open(r, sprint_id, "insecure/all", false, false).unwrap();
        let tip = rev(r, rama);
        worklist_provider::assign::assign_window(
            r,
            rama,
            worklist_provider::check_push::ALL_ZEROS,
            &tip,
            "https://x",
            &spy,
            "701",
            false,
        )
        .unwrap()
        .unwrap();
    }

    // Y recién ahora suben. Antes del arreglo la segunda moría acá con
    // "el panorama no recibe … normalize: ACC-…".
    for rama in ramas {
        let tip = rev(r, rama);
        propagate(r, rama, &tip, BASE, false).unwrap().unwrap();
    }

    // Y el panorama no se queda con el parcial de ninguna de las dos: las dos
    // citas quedan resueltas, que es la unión y no una de las mitades.
    let epica_file = show_tree(r, "insecure/all")
        .lines()
        .find(|n| n.ends_with(".epic.md"))
        .expect("el panorama tiene la epica")
        .to_string();
    let epica = show(r, "insecure/all", &epica_file);
    assert!(!epica.contains("(@o.task.md)"), "la cita a `@o` quedó sin renombrar:\n{epica}");
    assert!(!epica.contains("(@q.task.md)"), "la cita a `@q` quedó sin renombrar:\n{epica}");
}

/// El otro que hizo caer `propagate --all-windows`: el `.sprint.md` del
/// panorama se planifica —se cierra el sprint, se mueven ítems— así que su
/// contexto difiere del de la ventana por trabajo legítimo, y el parche que
/// sólo quería agregar `key:` chocaba contra eso.
#[test]
fn la_clave_del_sprint_se_reescribe_sobre_el_sprint_md_del_panorama() {
    let dir = arbol_con_cita();
    let r = &dir.path().join("repo");
    worklist_core::window::open(r, "1", "insecure/all", false, false).unwrap();

    let tip = rev(r, "refs/heads/secure/sprint/1");
    let spy = Spy::default();
    worklist_provider::assign::assign_window(
        r,
        "refs/heads/secure/sprint/1",
        worklist_provider::check_push::ALL_ZEROS,
        &tip,
        "https://x",
        &spy,
        "701",
        false,
    )
    .unwrap()
    .unwrap();

    // Y mientras tanto el sprint se cierra en el panorama, que es donde se
    // planifica: cambia `status`, y con él el contexto del parche.
    let sp = r.join("_sprints/1.sprint.md");
    let texto = std::fs::read_to_string(&sp).unwrap();
    std::fs::write(&sp, texto.replace("status: in-progress", "status: done")).unwrap();
    run(r, &["commit", "-aqm", "sprint 1 cerrado"]);

    let tip = rev(r, "refs/heads/secure/sprint/1");
    let p = propagate(r, "refs/heads/secure/sprint/1", &tip, BASE, false).unwrap().unwrap();

    let clave = p
        .steps
        .iter()
        .find_map(|s| match s {
            Step::SprintKeyed { key, .. } => Some(key.clone()),
            _ => None,
        })
        .expect("la clave del sprint se rehace, no se copia");

    let sprint = show(r, "insecure/all", "_sprints/1.sprint.md");
    assert!(sprint.contains(&format!("key: {clave}")), "falta la clave:\n{sprint}");
    // Y lo que el panorama había planificado sigue ahí: sube la clave, no el
    // `.sprint.md` de la ventana.
    assert!(sprint.contains("status: done"), "el cierre se perdió:\n{sprint}");
}
