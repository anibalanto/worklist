//! Los cuatro casos de aceptacion de `52`, contra un repo real de git en un
//! directorio temporal. Ningun test toca el repo del proyecto ni ningun
//! proveedor.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

fn git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    run(dir.path(), &["init", "-q"]);
    run(dir.path(), &["config", "user.email", "test@test"]);
    run(dir.path(), &["config", "user.name", "test"]);
    dir
}

fn run(repo: &Path, args: &[&str]) {
    let status = Command::new("git").arg("-C").arg(repo).args(args).status().unwrap();
    assert!(status.success(), "git {:?} failed", args);
}

fn write(repo: &Path, name: &str, content: &str) {
    std::fs::write(repo.join(name), content).unwrap();
}

fn head(repo: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn rename_sin_referencias_entrantes() {
    let dir = git_repo();
    let repo = dir.path();
    write(
        repo,
        "slug-solo.task.md",
        "---\ntitle: Solo\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\nNada la referencia.\n",
    );
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);

    let touched = worklist::rename_one(repo, "slug-solo", "ACC-1").unwrap();
    assert!(touched.is_empty());
    assert!(repo.join("ACC-1.task.md").exists());
    assert!(!repo.join("slug-solo.task.md").exists());
}

#[test]
fn rename_reescribe_referencias_delimitadas_y_no_subcadenas() {
    let dir = git_repo();
    let repo = dir.path();
    write(
        repo,
        "slug-a.task.md",
        "---\ntitle: A\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n# A\n",
    );
    write(
        repo,
        "slug-b.task.md",
        "---\ntitle: B\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nrelation.depends: [slug-a]\n---\nDepende de [`slug-a`](slug-a.task.md).\n",
    );
    write(
        repo,
        "unrelated.task.md",
        "---\ntitle: No tocar\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\nMenciona slug-a10 y slug-a sueltos, sin backticks.\n",
    );
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);

    let touched = worklist::rename_one(repo, "slug-a", "ACC-101").unwrap();
    assert_eq!(touched, vec!["slug-b.task.md".to_string()]);

    let b = std::fs::read_to_string(repo.join("slug-b.task.md")).unwrap();
    assert!(b.contains("relation.depends: [ACC-101]"));
    assert!(b.contains("[`ACC-101`](ACC-101.task.md)"));

    let unrelated = std::fs::read_to_string(repo.join("unrelated.task.md")).unwrap();
    assert!(unrelated.contains("slug-a10"), "no debia tocar la subcadena");
    assert!(unrelated.contains("slug-a sueltos"), "no debia tocar la mencion suelta sin backticks");
}

#[test]
fn resolve_batch_respeta_el_orden_topologico() {
    let dir = git_repo();
    let repo = dir.path();
    write(
        repo,
        "slug-c.task.md",
        "---\ntitle: C\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nrelation.depends: [slug-d]\n---\nDepende de [`slug-d`](slug-d.task.md).\n",
    );
    write(
        repo,
        "slug-d.task.md",
        "---\ntitle: D\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\nSin dependencias.\n",
    );
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);

    let mut map = HashMap::new();
    map.insert("slug-c".to_string(), "ACC-201".to_string());
    map.insert("slug-d".to_string(), "ACC-200".to_string());
    worklist::resolve_batch(repo, &map).unwrap();

    let c = std::fs::read_to_string(repo.join("ACC-201.task.md")).unwrap();
    assert!(c.contains("relation.depends: [ACC-200]"));
    assert!(c.contains("[`ACC-200`](ACC-200.task.md)"));
}

#[test]
fn un_ciclo_se_rechaza_sin_escribir_nada() {
    let dir = git_repo();
    let repo = dir.path();
    write(
        repo,
        "slug-e.task.md",
        "---\ntitle: E\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nrelation.depends: [slug-f]\n---\nCiclo.\n",
    );
    write(
        repo,
        "slug-f.task.md",
        "---\ntitle: F\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nrelation.depends: [slug-e]\n---\nCiclo.\n",
    );
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);
    let before = head(repo);

    let mut map = HashMap::new();
    map.insert("slug-e".to_string(), "ACC-301".to_string());
    map.insert("slug-f".to_string(), "ACC-302".to_string());
    let result = worklist::resolve_batch(repo, &map);

    assert!(result.is_err());
    assert_eq!(before, head(repo), "el repo no debia cambiar");
    assert!(repo.join("slug-e.task.md").exists());
    assert!(repo.join("slug-f.task.md").exists());
}
