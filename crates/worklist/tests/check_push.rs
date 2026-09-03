//! El compare-and-swap: coincide -> nada que rechazar; el proveedor se movio
//! -> se rechaza; una ref que no es ventana, o una rama nueva, no se chequea.

use std::path::Path;
use std::process::Command;
use worklist::check_push::check_one;
use worklist::provider::FileProvider;

fn run(repo: &Path, args: &[&str]) {
    let status = Command::new("git").arg("-C").arg(repo).args(args).status().unwrap();
    assert!(status.success(), "git {:?} failed", args);
}

fn rev_parse(repo: &Path, rev: &str) -> String {
    let out = Command::new("git").arg("-C").arg(repo).args(["rev-parse", rev]).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn seed_repo() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    run(repo, &["init", "-q"]);
    run(repo, &["config", "user.email", "test@test"]);
    run(repo, &["config", "user.name", "test"]);
    std::fs::write(
        repo.join("ACC-101.task.md"),
        "---\ntitle: X\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n",
    )
    .unwrap();
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);
    let tip = rev_parse(repo, "HEAD");
    (dir, tip)
}

#[test]
fn coincide_no_rechaza_nada() {
    let (dir, tip) = seed_repo();
    let repo = dir.path();
    let provider_file = repo.join("provider.json");
    let provider = FileProvider::new(&provider_file);
    provider.set_status("ACC-101", "open").unwrap();

    let rejected = check_one(repo, &tip, "refs/heads/sprint/10", &provider).unwrap();
    assert!(rejected.is_empty());
}

#[test]
fn el_proveedor_se_movio_y_se_rechaza() {
    let (dir, tip) = seed_repo();
    let repo = dir.path();
    let provider_file = repo.join("provider.json");
    let provider = FileProvider::new(&provider_file);
    provider.set_status("ACC-101", "done").unwrap(); // alguien lo cerro en Jira

    let rejected = check_one(repo, &tip, "refs/heads/sprint/10", &provider).unwrap();
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].key, "ACC-101");
    assert_eq!(rejected[0].tip_status, "open");
    assert_eq!(rejected[0].live_status, "done");
}

#[test]
fn una_rama_que_no_es_ventana_no_se_chequea() {
    let (dir, tip) = seed_repo();
    let repo = dir.path();
    let provider = FileProvider::new(repo.join("provider.json"));
    provider.set_status("ACC-101", "done").unwrap();

    let rejected = check_one(repo, &tip, "refs/heads/main", &provider).unwrap();
    assert!(rejected.is_empty(), "una rama fuera de sprint/* o backlog no se valida");
}

#[test]
fn una_rama_nueva_sin_tip_anterior_no_se_chequea() {
    let (dir, _tip) = seed_repo();
    let repo = dir.path();
    let provider = FileProvider::new(repo.join("provider.json"));

    let all_zeros = "0".repeat(40);
    let rejected = check_one(repo, &all_zeros, "refs/heads/sprint/11", &provider).unwrap();
    assert!(rejected.is_empty());
}
