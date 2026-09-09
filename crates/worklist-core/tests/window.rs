//! El recorte: lo que entra, y sobre todo lo que no.
//!
//! El caso que motiva todo esto es el ultimo: parado en una ventana, un item
//! ajeno **no esta**, asi que la pregunta "¿estara actualizado?" no se
//! contesta — desaparece.

use std::path::Path;
use std::process::Command;

fn run(repo: &Path, args: &[&str]) {
    let st = Command::new("git").arg("-C").arg(repo).args(args).status().unwrap();
    assert!(st.success(), "git {args:?}");
}

fn item(repo: &Path, name: &str, parent: Option<&str>) {
    let p = match parent {
        Some(p) => format!("parent: {p}\n"),
        None => String::new(),
    };
    std::fs::write(
        repo.join(name),
        format!("---\ntitle: {name}\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n{p}---\n\ncuerpo\n"),
    )
    .unwrap();
}

/// Una epica, dos US bajo ella, una task bajo la primera, y un item suelto
/// que no es de la ventana. El sprint declara solo la primera US.
fn repo_con_arbol() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    run(r, &["init", "-q"]);
    run(r, &["config", "user.email", "t@t"]);
    run(r, &["config", "user.name", "t"]);

    item(r, "ACC-1.epic.md", None);
    item(r, "ACC-2.user-story.md", Some("ACC-1"));
    item(r, "ACC-3.task.md", Some("ACC-2"));
    item(r, "ACC-8.user-story.md", Some("ACC-1")); // otra US de la misma epica
    item(r, "ACC-9.task.md", Some("ACC-8"));
    item(r, "ACC-7.task.md", None); // suelta, de nadie

    componer(r, "10", "sprint 10", &["ACC-2"]);

    run(r, &["add", "-A"]);
    run(r, &["commit", "-q", "-m", "arbol"]);
    dir
}

/// Como `repo_con_arbol`, pero con el repo en un **subdirectorio** y la rama
/// `insecure/all` ya creada.
///
/// El subdirectorio no es cosmético: `open` arma su worktree temporal en
/// `repo/..` con un nombre derivado del sprint, así que dos tests con el mismo
/// sprint y el repo directo en `/tmp` se pisan entre sí.
fn arbol_aislado() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path().join("repo");
    std::fs::create_dir(&r).unwrap();
    run(&r, &["init", "-q"]);
    run(&r, &["config", "user.email", "t@t"]);
    run(&r, &["config", "user.name", "t"]);
    item(&r, "ACC-1.epic.md", None);
    item(&r, "ACC-2.user-story.md", Some("ACC-1"));
    item(&r, "ACC-3.task.md", Some("ACC-2"));
    // Estos quedan afuera de la ventana: sin ellos el recorte no borra nada y
    // no hay qué commitear.
    item(&r, "ACC-8.user-story.md", Some("ACC-1"));
    item(&r, "ACC-9.task.md", Some("ACC-8"));
    item(&r, "ACC-7.task.md", None);
    componer(&r, "10", "sprint 10", &["ACC-2"]);
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-q", "-m", "arbol"]);
    run(&r, &["branch", "-q", "insecure/all"]);
    (dir, r)
}

/// Simula lo que hace el servidor: commitea encima de la ventana y deja la
/// rama ahí. Devuelve el head resultante.
fn el_servidor_escribe_encima(r: &Path, sprint: &str, mensaje: &str) -> String {
    let wt = r.join(format!("wt-{sprint}"));
    run(r, &["worktree", "add", "--detach", "-q", wt.to_str().unwrap(),
             &format!("secure/sprint/{sprint}")]);
    std::fs::write(wt.join("nuevo.txt"), mensaje).unwrap();
    run(&wt, &["add", "-A"]);
    run(&wt, &["commit", "-qm", mensaje]);
    let head = String::from_utf8(
        Command::new("git").arg("-C").arg(&wt).args(["rev-parse", "HEAD"]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    run(r, &["update-ref", &format!("refs/heads/secure/sprint/{sprint}"), &head]);
    run(r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);
    head
}

fn head_de(r: &Path, refname: &str) -> String {
    String::from_utf8(
        Command::new("git").arg("-C").arg(r).args(["rev-parse", refname]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string()
}

#[test]
fn lleva_el_item_declarado_y_su_subarbol_y_no_la_composicion() {
    let dir = repo_con_arbol();
    let files = worklist_core::window::window_files(dir.path(), "HEAD", "10").unwrap();

    // **La composicion no entra a la ventana.** Es del servidor, y una copia
    // del lado del cliente es una fuente de verdad que solo puede quedarse
    // vieja — el mismo motivo por el que el panorama tampoco baja.
    assert!(!files.iter().any(|f| f.starts_with(".metadata/product")),
        "la ventana se llevo la composicion: {files:?}");
    assert!(files.contains(&"ACC-2.user-story.md".to_string()), "el item declarado");
    assert!(files.contains(&"ACC-3.task.md".to_string()), "su hijo va con el");
}

#[test]
fn lleva_la_epica_como_ancestro_para_que_parent_cierre() {
    let dir = repo_con_arbol();
    let files = worklist_core::window::window_files(dir.path(), "HEAD", "10").unwrap();
    assert!(
        files.contains(&"ACC-1.epic.md".to_string()),
        "sin la epica, el `parent` de ACC-2 no resuelve adentro de la ventana"
    );
}

#[test]
fn no_lleva_lo_que_no_es_de_la_ventana() {
    let dir = repo_con_arbol();
    let files = worklist_core::window::window_files(dir.path(), "HEAD", "10").unwrap();

    // Otra US de la misma epica, y su task: no las declara el sprint.
    assert!(!files.contains(&"ACC-8.user-story.md".to_string()));
    assert!(!files.contains(&"ACC-9.task.md".to_string()));
    // Una task suelta, de nadie.
    assert!(!files.contains(&"ACC-7.task.md".to_string()));
    assert_eq!(files.len(), 3, "declarado + hijo + epica");
}

#[test]
fn un_sprint_que_nombra_algo_que_no_esta_falla() {
    let dir = repo_con_arbol();
    let r = dir.path();
    componer(r, "11", "sprint 11", &["ACC-404"]);
    run(r, &["add", "-A"]);
    run(r, &["commit", "-q", "-m", "sprint roto"]);

    let err = worklist_core::window::window_files(r, "HEAD", "11").unwrap_err();
    assert!(err.to_string().contains("ACC-404"), "el error tiene que decir cuál falta");
}

#[test]
fn open_deja_en_la_rama_solo_esos_archivos() {
    let dir = repo_con_arbol();
    let r = dir.path();
    run(r, &["branch", "-q", "insecure/all"]);

    let (files, _head) = worklist_core::window::open(r, "10", "insecure/all", false, false).unwrap();

    let out = Command::new("git")
        .arg("-C").arg(r)
        .args(["ls-tree", "-r", "--name-only", "refs/heads/secure/sprint/10"])
        .output()
        .unwrap();
    let en_la_rama: Vec<String> =
        String::from_utf8(out.stdout).unwrap().lines().map(str::to_string).collect();

    assert_eq!(en_la_rama, files, "la rama tiene exactamente lo recortado");
    assert!(!en_la_rama.contains(&"ACC-7.task.md".to_string()));
}

/// El defecto de `5o` era que `open` hacia `update-ref` incondicional y se
/// llevaba puesto lo que el servidor habia escrito encima. La salida no es
/// negarse: es **replantar**, que es lo que `64` decidio. El corte se
/// recalcula contra el `items` de hoy, y lo que estaba encima se re-aplica.
#[test]
fn recortar_de_nuevo_replanta_lo_que_estaba_encima() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();
    let head_servidor = el_servidor_escribe_encima(&r, "10", "normalize: ACC-1");

    let (_, head) = worklist_core::window::open(&r, "10", "insecure/all", false, false)
        .expect("regenerar no descarta: replanta");

    let _ = head_servidor;
    assert_eq!(head_de(&r, "refs/heads/secure/sprint/10"), head);
    let log = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["log", "--format=%s", "refs/heads/secure/sprint/10"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(log.contains("normalize: ACC-1"), "el trabajo de la ventana sobrevive: {log}");
    assert_eq!(
        log.matches("window: sprint/10").count(),
        1,
        "el corte se reemplaza, no se apila: {log}"
    );
    assert!(
        log.lines().next() == Some("normalize: ACC-1"),
        "y queda encima del corte, que es donde vive el trabajo: {log}"
    );
}

/// Lo que separa **regenerar** de rebasear el corte, y es la razon de todo
/// esto: re-aplicar el commit del corte no menciona lo que el panorama gano
/// despues, asi que lo nuevo entra. Recalcularlo no.
#[test]
fn regenerar_no_ensancha_la_ventana_con_lo_que_el_panorama_gano() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();
    el_servidor_escribe_encima(&r, "10", "trabajo adentro");

    // El panorama gana un item que no es de esta ventana.
    let wt = r.join("pan");
    run(&r, &["worktree", "add", "-q", wt.to_str().unwrap(), "insecure/all"]);
    item(&wt, "ACC-77.task.md", None);
    run(&wt, &["add", "-A"]);
    run(&wt, &["commit", "-qm", "un item ajeno"]);
    run(&r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();

    let en_la_rama = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["ls-tree", "-r", "--name-only", "refs/heads/secure/sprint/10"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(!en_la_rama.contains("ACC-77"), "lo ajeno no entra por la puerta de atras");
    assert!(en_la_rama.contains("ACC-2.user-story.md"), "y lo suyo sigue: {en_la_rama}");
}

/// `--force` baja de categoria: ya no es el flujo del `items` que cambio —eso
/// es regenerar y ya— sino tirar lo local a sabiendas cuando el replante no
/// entra.
#[test]
fn force_tira_lo_de_la_ventana_a_sabiendas() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();
    let head_servidor = el_servidor_escribe_encima(&r, "10", "encima");

    let (_, head) = worklist_core::window::open(&r, "10", "insecure/all", false, true).unwrap();
    assert_ne!(head, head_servidor, "con --force el corte reemplaza");
    assert_eq!(head_de(&r, "refs/heads/secure/sprint/10"), head);
}

/// Abrir una ventana que no existe no encuentra nada que descartar, y anda.
#[test]
fn opening_a_fresh_window_is_not_blocked() {
    let (_dir, r) = arbol_aislado();
    let (files, head) = worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();
    assert!(!files.is_empty());
    assert!(!head.is_empty());
}

/// Re-cortar sin que nadie haya escrito encima no tiene nada que replantar.
#[test]
fn recutting_an_untouched_window_is_not_blocked() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();
    worklist_core::window::open(&r, "10", "insecure/all", false, false)
        .expect("nada que descartar, nada que impedir");
}

/// La tercera cara del mismo defecto: `update-ref` mueve una rama aunque tenga
/// un worktree checkouteado —`git branch -f` se niega—, y deja ese worktree con
/// el indice del arbol anterior. El sintoma no dice la causa: archivos
/// "modificados" que nadie toco, y un `merge` que se niega por cambios locales
/// que no existen.
#[test]
fn opening_a_window_checked_out_somewhere_refuses() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();

    let wt = r.join("checkout");
    run(&r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/10"]);

    for force in [false, true] {
        let err = worklist_core::window::open(&r, "10", "insecure/all", false, force)
            .unwrap_err()
            .to_string();
        assert!(err.contains("worktree"), "tiene que nombrar el problema: {err}");
        assert!(err.contains("checkout"), "y donde esta: {err}");
    }

    // Y el worktree quedó sano: sin cambios fantasma en el índice.
    let status = String::from_utf8(
        Command::new("git").arg("-C").arg(&wt).args(["status", "--short"]).output().unwrap().stdout,
    )
    .unwrap();
    assert!(status.trim().is_empty(), "el worktree no se toco: {status:?}");
}

/// El vocabulario es del proyecto y vive en el panorama, que el cliente ya no
/// tiene. Sin esto, `state change` parado en la ventana cae al vocabulario por
/// defecto y rechaza un estado que el proyecto si declara.
#[test]
fn la_ventana_lleva_el_vocabulario() {
    let dir = repo_con_arbol();
    let r = dir.path();
    std::fs::create_dir_all(r.join(".metadata")).unwrap();
    std::fs::write(r.join(".metadata/states.yaml"), "states: [open, review, done]\n").unwrap();
    run(r, &["add", "-A"]);
    run(r, &["commit", "-q", "-m", "vocabulario"]);

    let files = worklist_core::window::window_files(r, "HEAD", "10").unwrap();

    assert!(
        files.contains(&".metadata/states.yaml".to_string()),
        "la ventana tiene que cerrar adentro: {files:?}"
    );
}

/// Y un proyecto que no lo declara no gana un archivo que no existe.
#[test]
fn sin_vocabulario_declarado_la_ventana_no_inventa_uno() {
    let dir = repo_con_arbol();
    let files = worklist_core::window::window_files(dir.path(), "HEAD", "10").unwrap();
    assert!(!files.iter().any(|f| f.starts_with(".metadata/")));
}

/// La composicion del producto: de aca lee el recorte.
fn componer(r: &std::path::Path, id: &str, titulo: &str, items: &[&str]) {
    let path = r.join(worklist_core::product::ARCHIVO);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut p = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| worklist_core::product::de_yaml(&t).ok())
        .unwrap_or_default();
    p.sprints.retain(|s| s.id != id);
    p.sprints.push(worklist_core::product::Sprint {
        id: id.into(),
        titulo: titulo.into(),
        status: "in-progress".into(),
        key: None,
        items: items.iter().map(|s| s.to_string()).collect(),
    });
    p.sprints.sort_by(|a, b| a.id.cmp(&b.id));
    std::fs::write(&path, p.to_yaml().unwrap()).unwrap();
}
