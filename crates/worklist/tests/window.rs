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

    std::fs::create_dir(r.join("_sprints")).unwrap();
    std::fs::write(
        r.join("_sprints/10.sprint.md"),
        "---\ntitle: El sprint\nstatus: in-progress\nitems: [ACC-2]\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\nplan\n",
    )
    .unwrap();

    run(r, &["add", "-A"]);
    run(r, &["commit", "-q", "-m", "arbol"]);
    dir
}

#[test]
fn lleva_el_sprint_el_item_declarado_y_su_subarbol() {
    let dir = repo_con_arbol();
    let files = worklist::window::window_files(dir.path(), "HEAD", "10").unwrap();

    assert!(files.contains(&"_sprints/10.sprint.md".to_string()));
    assert!(files.contains(&"ACC-2.user-story.md".to_string()), "el item declarado");
    assert!(files.contains(&"ACC-3.task.md".to_string()), "su hijo va con el");
}

#[test]
fn lleva_la_epica_como_ancestro_para_que_parent_cierre() {
    let dir = repo_con_arbol();
    let files = worklist::window::window_files(dir.path(), "HEAD", "10").unwrap();
    assert!(
        files.contains(&"ACC-1.epic.md".to_string()),
        "sin la epica, el `parent` de ACC-2 no resuelve adentro de la ventana"
    );
}

#[test]
fn no_lleva_lo_que_no_es_de_la_ventana() {
    let dir = repo_con_arbol();
    let files = worklist::window::window_files(dir.path(), "HEAD", "10").unwrap();

    // Otra US de la misma epica, y su task: no las declara el sprint.
    assert!(!files.contains(&"ACC-8.user-story.md".to_string()));
    assert!(!files.contains(&"ACC-9.task.md".to_string()));
    // Una task suelta, de nadie.
    assert!(!files.contains(&"ACC-7.task.md".to_string()));
    assert_eq!(files.len(), 4, "sprint + declarado + hijo + epica");
}

#[test]
fn un_sprint_que_nombra_algo_que_no_esta_falla() {
    let dir = repo_con_arbol();
    let r = dir.path();
    std::fs::write(
        r.join("_sprints/11.sprint.md"),
        "---\ntitle: Roto\nstatus: open\nitems: [ACC-404]\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n",
    )
    .unwrap();
    run(r, &["add", "-A"]);
    run(r, &["commit", "-q", "-m", "sprint roto"]);

    let err = worklist::window::window_files(r, "HEAD", "11").unwrap_err();
    assert!(err.to_string().contains("ACC-404"), "el error tiene que decir cuál falta");
}

#[test]
fn open_deja_en_la_rama_solo_esos_archivos() {
    let dir = repo_con_arbol();
    let r = dir.path();
    run(r, &["branch", "-q", "insecure/all"]);

    let (files, _head) = worklist::window::open(r, "10", "insecure/all", false).unwrap();

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
