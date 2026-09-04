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

    let rejected = check_one(repo, &tip, &tip, "refs/heads/secure/sprint/10", &provider).unwrap();
    assert!(rejected.is_empty());
}

#[test]
fn el_proveedor_se_movio_y_se_rechaza() {
    let (dir, tip) = seed_repo();
    let repo = dir.path();
    let provider_file = repo.join("provider.json");
    let provider = FileProvider::new(&provider_file);
    provider.set_status("ACC-101", "done").unwrap(); // alguien lo cerro en Jira

    let rejected = check_one(repo, &tip, &tip, "refs/heads/secure/sprint/10", &provider).unwrap();
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

    let rejected = check_one(repo, &tip, &tip, "refs/heads/main", &provider).unwrap();
    assert!(rejected.is_empty(), "una rama fuera de sprint/* o backlog no se valida");
}

#[test]
fn una_rama_nueva_sin_tip_anterior_no_se_chequea() {
    let (dir, _tip) = seed_repo();
    let repo = dir.path();
    let provider = FileProvider::new(repo.join("provider.json"));

    let all_zeros = "0".repeat(40);
    let rejected = check_one(repo, &all_zeros, &all_zeros, "refs/heads/secure/sprint/11", &provider).unwrap();
    assert!(rejected.is_empty());
}

#[test]
fn las_tres_clases_de_rama_se_distinguen_por_el_prefijo() {
    use worklist::check_push::{classify, RefClass};
    assert_eq!(classify("refs/heads/secure/sprint/10"), RefClass::Secure);
    assert_eq!(classify("refs/heads/insecure/all"), RefClass::Insecure);
    assert_eq!(classify("refs/heads/insecure/backlog"), RefClass::Insecure);
    // Lo que no es del worklist no es de nadie: los hooks no opinan.
    assert_eq!(classify("refs/heads/main"), RefClass::Other);
    assert_eq!(classify("refs/heads/sprint/10"), RefClass::Other);
}

#[test]
fn una_insegura_no_se_verifica_aunque_el_proveedor_se_haya_movido() {
    // No es que este limpia: es que no se puede preguntar por ella. Quien la
    // rechaza es el hook, antes de llegar aca.
    let (dir, tip) = seed_repo();
    let repo = dir.path();
    let provider = FileProvider::new(repo.join("provider.json"));
    provider.set_status("ACC-101", "done").unwrap();

    let rejected = check_one(repo, &tip, &tip, "refs/heads/insecure/all", &provider).unwrap();
    assert!(rejected.is_empty());
}

// ─── el contenido, no solo el status ───────────────────────────────────────
//
// Task `61`. Mientras el cuerpo no viajaba, un push no podia pisar una
// descripcion editada en el proveedor: no la tocaba. La task `60` lo hizo
// viajar, y esto es lo que cierra el hueco.

use std::collections::HashMap;
use worklist::provider::{Provider, Snapshot};

/// Un proveedor que sí informa título y cuerpo, como Jira.
struct Rico(HashMap<String, Snapshot>);

impl Provider for Rico {
    fn snapshot(&self, keys: &[String]) -> anyhow::Result<HashMap<String, Snapshot>> {
        Ok(keys.iter().filter_map(|k| self.0.get(k).map(|s| (k.clone(), s.clone()))).collect())
    }
}

fn adf(texto: &str) -> String {
    worklist::body::body_to_adf(texto).unwrap()
}

/// Un repo con un ítem cuyo cuerpo se conoce, y un push que lo toca.
fn repo_con_cuerpo(cuerpo: &str) -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    run(repo, &["init", "-q"]);
    run(repo, &["config", "user.email", "test@test"]);
    run(repo, &["config", "user.name", "test"]);
    let escribir = |c: &str| {
        std::fs::write(
            repo.join("ACC-101.task.md"),
            format!("---\ntitle: X\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\n{c}\n"),
        )
        .unwrap()
    };
    escribir(cuerpo);
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);
    let old = rev_parse(repo, "HEAD");
    // el push toca ese mismo ítem
    escribir("lo que estoy empujando");
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "push"]);
    let new = rev_parse(repo, "HEAD");
    (dir, old, new)
}

/// El caso del ítem: alguien editó la descripción en el proveedor, y el push
/// que la pisaría se rechaza.
#[test]
fn un_cuerpo_editado_en_el_proveedor_rechaza_el_push() {
    let (dir, old, new) = repo_con_cuerpo("lo que el tip tiene");
    let p = Rico(HashMap::from([(
        "ACC-101".to_string(),
        Snapshot {
            status: Some("open".into()),
            description: Some(adf("otra cosa, editada en el board")),
            ..Default::default()
        },
    )]));
    let r = check_one(dir.path(), &old, &new, "refs/heads/secure/sprint/1", &p).unwrap();
    assert_eq!(r.len(), 1, "tiene que rechazar");
    assert_eq!(r[0].field, "cuerpo");
    assert_eq!(r[0].key, "ACC-101");
}

/// Y si el proveedor tiene lo mismo que el tip, el push entra.
#[test]
fn un_cuerpo_que_coincide_no_rechaza() {
    let (dir, old, new) = repo_con_cuerpo("lo que el tip tiene");
    let p = Rico(HashMap::from([(
        "ACC-101".to_string(),
        Snapshot {
            status: Some("open".into()),
            description: Some(adf("lo que el tip tiene")),
            ..Default::default()
        },
    )]));
    let r = check_one(dir.path(), &old, &new, "refs/heads/secure/sprint/1", &p).unwrap();
    assert!(r.is_empty(), "coincide, no hay nada que rechazar: {:?}", r[0].key);
}

/// El título también se compara.
#[test]
fn un_titulo_editado_en_el_proveedor_rechaza_el_push() {
    let (dir, old, new) = repo_con_cuerpo("da igual");
    let p = Rico(HashMap::from([(
        "ACC-101".to_string(),
        Snapshot {
            status: Some("open".into()),
            summary: Some("Un titulo que alguien cambio en el board".into()),
            ..Default::default()
        },
    )]));
    let r = check_one(dir.path(), &old, &new, "refs/heads/secure/sprint/1", &p).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].field, "titulo");
}

/// **El alcance**: un push que no toca el ítem no prueba nada sobre su cuerpo,
/// aunque el proveedor lo tenga distinto. Lo que no se escribe no se puede
/// pisar.
#[test]
fn un_item_que_el_push_no_toca_no_se_compara_por_cuerpo() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    run(repo, &["init", "-q"]);
    run(repo, &["config", "user.email", "test@test"]);
    run(repo, &["config", "user.name", "test"]);
    let fm = "---\ntitle: X\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\n";
    std::fs::write(repo.join("ACC-101.task.md"), format!("{fm}intacto\n")).unwrap();
    std::fs::write(repo.join("ACC-102.task.md"), format!("{fm}yo si cambio\n")).unwrap();
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);
    let old = rev_parse(repo, "HEAD");
    std::fs::write(repo.join("ACC-102.task.md"), format!("{fm}cambiado\n")).unwrap();
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "push"]);
    let new = rev_parse(repo, "HEAD");

    // El proveedor tiene el cuerpo de ACC-101 distinto, pero el push no lo toca.
    let p = Rico(HashMap::from([
        (
            "ACC-101".to_string(),
            Snapshot {
                status: Some("open".into()),
                description: Some(adf("el board dice otra cosa de ACC-101")),
                ..Default::default()
            },
        ),
        (
            "ACC-102".to_string(),
            Snapshot { status: Some("open".into()), ..Default::default() },
        ),
    ]));
    let r = check_one(repo, &old, &new, "refs/heads/secure/sprint/1", &p).unwrap();
    assert!(r.is_empty(), "no se escribe ACC-101, no hay nada que probar: {:?}", r.first().map(|x| &x.key));
}

/// Pero el `status` sí se compara sobre **todas**: esa es la otra promesa, y la
/// rama se verifica entera.
#[test]
fn el_status_se_compara_aunque_el_push_no_toque_el_item() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    run(repo, &["init", "-q"]);
    run(repo, &["config", "user.email", "test@test"]);
    run(repo, &["config", "user.name", "test"]);
    let fm = "---\ntitle: X\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\n";
    std::fs::write(repo.join("ACC-101.task.md"), format!("{fm}intacto\n")).unwrap();
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);
    let old = rev_parse(repo, "HEAD");
    std::fs::write(repo.join("otro.txt"), "no es un item").unwrap();
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "push que no toca items"]);
    let new = rev_parse(repo, "HEAD");

    let p = Rico(HashMap::from([(
        "ACC-101".to_string(),
        Snapshot { status: Some("in-progress".into()), ..Default::default() },
    )]));
    let r = check_one(repo, &old, &new, "refs/heads/secure/sprint/1", &p).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].field, "status");
}

/// Un proveedor que **no informa** un campo no hace fallar la comparación:
/// `None` es "no lo sé", no "está vacío".
#[test]
fn un_proveedor_que_no_informa_el_cuerpo_no_rechaza() {
    let (dir, old, new) = repo_con_cuerpo("lo que el tip tiene");
    let p = Rico(HashMap::from([(
        "ACC-101".to_string(),
        Snapshot { status: Some("open".into()), ..Default::default() },
    )]));
    let r = check_one(dir.path(), &old, &new, "refs/heads/secure/sprint/1", &p).unwrap();
    assert!(r.is_empty(), "sin dato no hay divergencia que afirmar");
}

/// Task `5j`: borrar una rama llega con `new` en ceros. No escribe nada en el
/// proveedor, asi que no hay nada que verificar — y antes de esto el chequeo
/// de contenido le pedia a git el arbol del sha nulo y **rechazaba el borrado**.
#[test]
fn borrar_una_rama_no_se_verifica() {
    let (dir, tip) = seed_repo();
    let ceros = "0".repeat(40);
    let provider = FileProvider::new(dir.path().join("provider.json"));
    provider.set_status("ACC-101", "in-progress").unwrap();  // divergiria, si se mirara
    let r = check_one(dir.path(), &tip, &ceros, "refs/heads/secure/sprint/10", &provider).unwrap();
    assert!(r.is_empty(), "un borrado no verifica nada");
}
