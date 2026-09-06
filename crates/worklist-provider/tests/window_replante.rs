//! El cruce entre recortar y propagar: vive aca y no en `worklist-core`
//! porque usa los dos crates — recortar es del cliente, propagar es del
//! servidor. Ver `concepts/distribution.md`.

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
    std::fs::create_dir(r.join("_sprints")).unwrap();
    std::fs::write(
        r.join("_sprints/10.sprint.md"),
        "---\ntitle: El sprint\nstatus: in-progress\nitems: [ACC-2]\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\nplan\n",
    )
    .unwrap();
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-q", "-m", "arbol"]);
    run(&r, &["branch", "-q", "insecure/all"]);
    (dir, r)
}

fn head_de(r: &Path, refname: &str) -> String {
    String::from_utf8(
        Command::new("git").arg("-C").arg(r).args(["rev-parse", refname]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string()
}

/// Y el descarte deja de ser una promesa: si el trabajo ya subio al panorama,
/// el replante queda vacio **solo**, por patch-id. Nadie tiene que acordarse
/// de propagar antes de regenerar.
#[test]
fn el_replante_queda_vacio_si_el_trabajo_ya_subio() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();

    let wt = r.join("w");
    run(&r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/10"]);
    item(&wt, "ACC-3.task.md", Some("ACC-2"));
    std::fs::write(wt.join("ACC-3.task.md"), "---\ntitle: x\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nparent: ACC-2\n---\n\neditado\n").unwrap();
    run(&wt, &["commit", "-aqm", "edito ACC-3"]);
    run(&r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    let tip = head_de(&r, "refs/heads/secure/sprint/10");
    worklist_provider::propagate::propagate(&r, "refs/heads/secure/sprint/10", &tip, "https://ejemplo.atlassian.net", false)
        .unwrap()
        .unwrap();

    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();

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
    // El commit sigue nombrado en el log, y **por el panorama**: la
    // propagacion lo cherry-pickeo alla, y el corte nuevo sale de ahi. Lo que
    // no queda es una copia **encima** del corte.
    assert_eq!(
        log.lines().next(),
        Some("window: sprint/10 recortado desde insecure/all"),
        "el replante quedo vacio solo: {log}"
    );
    let contenido = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["show", "refs/heads/secure/sprint/10:ACC-3.task.md"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(contenido.contains("editado"), "y sin embargo el trabajo esta: {contenido}");
}
