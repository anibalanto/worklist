//! El compare-and-swap: coincide -> nada que rechazar; el proveedor se movio
//! -> se rechaza; una ref que no es ventana, o una rama nueva, no se chequea.

use std::path::Path;
use std::process::Command;
use worklist_provider::check_push::check_one;
use worklist_provider::provider::FileProvider;
use worklist_provider::states::Estados;

/// La base del proveedor. Sale de la comparacion del cuerpo, que traduce los
/// links del borde antes de comparar. Ver `concepts/sync.md`.
const BASE: &str = "https://ejemplo.atlassian.net";

/// El mapeo del proveedor de prueba **es** la identidad: su archivo lleva
/// valores con la forma del worklist. Ver `concepts/states.md`.
fn estados() -> Estados {
    Estados::identidad(&worklist_provider::states::vocabulario(None))
}

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

    let rejected = check_one(repo, &tip, &tip, "refs/heads/secure/sprint/10", &provider, &estados(), BASE).unwrap();
    assert!(rejected.is_empty());
}

#[test]
fn el_proveedor_se_movio_y_se_rechaza() {
    let (dir, tip) = seed_repo();
    let repo = dir.path();
    let provider_file = repo.join("provider.json");
    let provider = FileProvider::new(&provider_file);
    provider.set_status("ACC-101", "done").unwrap(); // alguien lo cerro en Jira

    let rejected = check_one(repo, &tip, &tip, "refs/heads/secure/sprint/10", &provider, &estados(), BASE).unwrap();
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].key, "ACC-101");
    assert_eq!(rejected[0].tip, "open");
    assert_eq!(rejected[0].live, "done");
}

#[test]
fn una_rama_que_no_es_ventana_no_se_chequea() {
    let (dir, tip) = seed_repo();
    let repo = dir.path();
    let provider = FileProvider::new(repo.join("provider.json"));
    provider.set_status("ACC-101", "done").unwrap();

    let rejected = check_one(repo, &tip, &tip, "refs/heads/main", &provider, &estados(), BASE).unwrap();
    assert!(rejected.is_empty(), "una rama fuera de sprint/* o backlog no se valida");
}

#[test]
fn una_rama_nueva_sin_tip_anterior_no_se_chequea() {
    let (dir, _tip) = seed_repo();
    let repo = dir.path();
    let provider = FileProvider::new(repo.join("provider.json"));

    let all_zeros = "0".repeat(40);
    let rejected = check_one(repo, &all_zeros, &all_zeros, "refs/heads/secure/sprint/11", &provider, &estados(), BASE).unwrap();
    assert!(rejected.is_empty());
}

#[test]
fn las_tres_clases_de_rama_se_distinguen_por_el_prefijo() {
    use worklist_provider::check_push::{classify, RefClass};
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

    let rejected = check_one(repo, &tip, &tip, "refs/heads/insecure/all", &provider, &estados(), BASE).unwrap();
    assert!(rejected.is_empty());
}

// ─── el contenido, no solo el status ───────────────────────────────────────
//
// Task `61`. Mientras el cuerpo no viajaba, un push no podia pisar una
// descripcion editada en el proveedor: no la tocaba. La task `60` lo hizo
// viajar, y esto es lo que cierra el hueco.

use std::collections::HashMap;
use worklist_provider::provider::{Provider, Snapshot};

/// Un proveedor que sí informa título y cuerpo, como Jira.
struct Rico(HashMap<String, Snapshot>);

impl Provider for Rico {
    fn snapshot(&self, keys: &[String]) -> anyhow::Result<HashMap<String, Snapshot>> {
        Ok(keys.iter().filter_map(|k| self.0.get(k).map(|s| (k.clone(), s.clone()))).collect())
    }
}

fn adf(texto: &str) -> String {
    worklist_core::body::body_to_adf(texto).unwrap()
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
    let r = check_one(dir.path(), &old, &new, "refs/heads/secure/sprint/1", &p, &estados(), BASE).unwrap();
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
    let r = check_one(dir.path(), &old, &new, "refs/heads/secure/sprint/1", &p, &estados(), BASE).unwrap();
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
    let r = check_one(dir.path(), &old, &new, "refs/heads/secure/sprint/1", &p, &estados(), BASE).unwrap();
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
    let r = check_one(repo, &old, &new, "refs/heads/secure/sprint/1", &p, &estados(), BASE).unwrap();
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
    let r = check_one(repo, &old, &new, "refs/heads/secure/sprint/1", &p, &estados(), BASE).unwrap();
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
    let r = check_one(dir.path(), &old, &new, "refs/heads/secure/sprint/1", &p, &estados(), BASE).unwrap();
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
    let r = check_one(dir.path(), &tip, &ceros, "refs/heads/secure/sprint/10", &provider, &estados(), BASE).unwrap();
    assert!(r.is_empty(), "un borrado no verifica nada");
}

// ─── los estados: traducidos, y la transicion propuesta ────────────────────
//
// `ACC-276`. El `status` del worklist y el del proveedor no son el mismo campo:
// compararlos crudos rechazaba TODAS las ventanas, que es lo que tenia a la
// instalacion apuntando al proveedor de prueba.

use std::collections::BTreeMap;
use worklist_provider::states::Destino;

/// El mapeo real de una instalacion con Jira.
fn con_jira() -> Estados {
    let mapeo = BTreeMap::from([
        ("open".to_string(), Destino::Status("To Do".into())),
        ("in-progress".to_string(), Destino::Status("In Progress".into())),
        ("done".to_string(), Destino::Status("Done".into())),
        (
            "dropped".to_string(),
            Destino::Completo { status: "Done".into(), resolution: Some("Won't Do".into()) },
        ),
    ]);
    Estados::new(&worklist_provider::states::vocabulario(None), mapeo).unwrap()
}

/// Un proveedor que informa status y las transiciones que su workflow admite.
struct ConWorkflow {
    status: String,
    disponibles: Option<Vec<String>>,
}

impl Provider for ConWorkflow {
    fn snapshot(&self, keys: &[String]) -> anyhow::Result<HashMap<String, Snapshot>> {
        Ok(keys
            .iter()
            .map(|k| {
                (k.clone(), Snapshot { status: Some(self.status.clone()), ..Default::default() })
            })
            .collect())
    }
    fn available_transitions(&self, _key: &str) -> anyhow::Result<Option<Vec<String>>> {
        Ok(self.disponibles.clone())
    }
}

/// Un repo con un item en `open`, y un push que lo mueve a `estado`.
fn repo_que_mueve(estado: &str) -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    run(repo, &["init", "-q"]);
    run(repo, &["config", "user.email", "test@test"]);
    run(repo, &["config", "user.name", "test"]);
    let escribir = |st: &str| {
        std::fs::write(
            repo.join("ACC-101.task.md"),
            format!("---\ntitle: X\nstatus: {st}\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\ncuerpo\n"),
        )
        .unwrap()
    };
    escribir("open");
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);
    let old = rev_parse(repo, "HEAD");
    escribir(estado);
    run(repo, &["add", "-A"]);
    // `--allow-empty`: mover a un estado que ya estaba es un push que no
    // cambia el archivo, y es justo el caso que prueba la traduccion.
    run(repo, &["commit", "-q", "--allow-empty", "-m", "state change"]);
    let new = rev_parse(repo, "HEAD");
    (dir, old, new)
}

/// Lo que el mapeo arregla: el tip dice `open`, Jira dice `"To Do"`, y son lo
/// mismo. Sin traducir, esto rechazaba.
#[test]
fn el_status_se_compara_traducido_y_no_crudo() {
    let (dir, old, _) = repo_que_mueve("open");
    let p = ConWorkflow { status: "To Do".into(), disponibles: None };
    let r = check_one(dir.path(), &old, &old, "refs/heads/secure/sprint/1", &p, &con_jira(), BASE).unwrap();
    assert!(r.is_empty(), "traducido coincide: {:?}", r.iter().map(|x| x.field).collect::<Vec<_>>());

    // Y con el mapeo identidad —el del proveedor de prueba— el mismo estado
    // rechaza, que es exactamente el defecto que este item cierra.
    let r = check_one(dir.path(), &old, &old, "refs/heads/secure/sprint/1", &p, &estados(), BASE).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].field, "status");
}

/// Un rechazo **por regla**: no es que el proveedor se movio, es que lo que se
/// pide no es una transicion legal. Y dice cuales si.
#[test]
fn una_transicion_ilegal_rechaza_el_push_y_dice_cuales_si() {
    let (dir, old, new) = repo_que_mueve("done");
    let p = ConWorkflow {
        status: "To Do".into(),
        disponibles: Some(vec!["Ready for Review".into()]),
    };
    let r = check_one(dir.path(), &old, &new, "refs/heads/secure/sprint/1", &p, &con_jira(), BASE).unwrap();
    assert_eq!(r.len(), 1, "{r:?}", r = r.iter().map(|x| x.field).collect::<Vec<_>>());
    assert_eq!(r[0].field, "transicion", "no es una deriva: es una regla");
    assert_eq!(r[0].tip, "Done", "el destino traducido");
    assert_eq!(r[0].disponibles.as_deref(), Some(&["Ready for Review".to_string()][..]));
}

/// Y si el workflow la admite, el push entra.
#[test]
fn una_transicion_legal_no_rechaza() {
    let (dir, old, new) = repo_que_mueve("done");
    let p = ConWorkflow { status: "To Do".into(), disponibles: Some(vec!["Done".into()]) };
    let r = check_one(dir.path(), &old, &new, "refs/heads/secure/sprint/1", &p, &con_jira(), BASE).unwrap();
    assert!(r.is_empty(), "{:?}", r.iter().map(|x| x.field).collect::<Vec<_>>());
}

/// **No poder listar no es que no haya ninguna.** Un proveedor que no informa
/// transiciones no puede rechazar por regla: no hay nada que verificar.
#[test]
fn un_proveedor_sin_workflow_no_rechaza_por_regla() {
    let (dir, old, new) = repo_que_mueve("done");
    let p = ConWorkflow { status: "To Do".into(), disponibles: None };
    let r = check_one(dir.path(), &old, &new, "refs/heads/secure/sprint/1", &p, &con_jira(), BASE).unwrap();
    assert!(r.is_empty(), "{:?}", r.iter().map(|x| x.field).collect::<Vec<_>>());
}

/// Sacar un item del arbol **es** proponer su transicion a `dropped`, y se lee
/// del diff: ni campo, ni lapida, ni commit especial.
#[test]
fn un_borrado_propone_dropped_con_su_resolucion() {
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
    let old = rev_parse(repo, "HEAD");
    std::fs::remove_file(repo.join("ACC-101.task.md")).unwrap();
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "remove"]);
    let new = rev_parse(repo, "HEAD");

    let propuestas =
        worklist_provider::check_push::transiciones_propuestas(repo, &old, &new, &con_jira())
            .unwrap();
    assert_eq!(propuestas, vec![("ACC-101".to_string(), "Done".to_string())]);

    // Y el destino lleva la resolucion, que es lo unico que lo distingue de
    // `done` en el board.
    assert_eq!(con_jira().destino("dropped").unwrap().resolution(), Some("Won't Do"));
}

/// Un status que el vocabulario no declara se **rechaza**, no se saltea: no
/// poder traducirlo es no poder decir nada, y callar seria confundirlo con
/// "esta bien".
#[test]
fn un_status_fuera_del_vocabulario_se_rechaza() {
    let (dir, old, _) = repo_que_mueve("inventado");
    let p = ConWorkflow { status: "To Do".into(), disponibles: None };
    let r = check_one(dir.path(), &old, &old, "refs/heads/secure/sprint/1", &p, &con_jira(), BASE).unwrap();
    // El tip de `old` dice `open`; el que quedo fuera del vocabulario es el de
    // `new`, asi que este caso se arma al reves: el tip **es** el inventado.
    let (dir2, _, new2) = repo_que_mueve("inventado");
    let r2 = check_one(dir2.path(), &new2, &new2, "refs/heads/secure/sprint/1", &p, &con_jira(), BASE)
        .unwrap();
    assert!(r.is_empty(), "el tip de old estaba bien");
    assert_eq!(r2.len(), 1);
    assert!(r2[0].tip.contains("no esta en el vocabulario"), "{}", r2[0].tip);
}

// ── ACC-318: comparar sin rechazar.
//
// Los dos recortes del push —la rama insegura, y el titulo y el cuerpo solo
// sobre lo que se escribe— existen por el push. Sin push no hay ninguno de los
// dos, y lo que se quiere saber es cuanto difiere el inventario entero.

use worklist_provider::check_push::check_ref;

/// Un panorama con dos items, ninguno tocado por nadie.
fn panorama(cuerpo_101: &str, cuerpo_102: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    run(repo, &["init", "-q"]);
    run(repo, &["config", "user.email", "test@test"]);
    run(repo, &["config", "user.name", "test"]);
    let fm = "---\ntitle: X\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\n";
    std::fs::write(repo.join("ACC-101.task.md"), format!("{fm}{cuerpo_101}\n")).unwrap();
    std::fs::write(repo.join("ACC-102.task.md"), format!("{fm}{cuerpo_102}\n")).unwrap();
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);
    run(repo, &["branch", "-f", "insecure/all", "HEAD"]);
    dir
}

/// El primer recorte: a una rama insegura no se le empuja, pero leerla no es
/// empujarle. `check_one` la saltea; `check_ref` la mide.
#[test]
fn una_rama_insegura_se_lee_en_vez_de_saltearse() {
    let dir = panorama("igual", "igual");
    let p = Rico(HashMap::from([
        ("ACC-101".to_string(), Snapshot { status: Some("done".into()), ..Default::default() }),
        ("ACC-102".to_string(), Snapshot { status: Some("open".into()), ..Default::default() }),
    ]));
    let salteada =
        check_one(dir.path(), "HEAD", "HEAD", "refs/heads/insecure/all", &p, &estados(), BASE).unwrap();
    assert!(salteada.is_empty(), "verificando un push, la insegura no se compara");

    let r = check_ref(dir.path(), "refs/heads/insecure/all", &p, &estados(), BASE).unwrap();
    assert_eq!(r.compared, 2);
    assert_eq!(r.differences.len(), 1, "ACC-101 difiere en status");
    assert_eq!(r.differences[0].key, "ACC-101");
    assert_eq!(r.differences[0].field, "status");
}

/// El segundo recorte: sin push, el cuerpo se mira sobre **todas** las claves
/// del tip. Es la diferencia que `check_one` no encuentra por diseño.
#[test]
fn el_cuerpo_se_compara_sobre_todas_las_claves_del_tip() {
    let dir = panorama("lo que el tip tiene", "y este igual");
    let p = Rico(HashMap::from([
        (
            "ACC-101".to_string(),
            Snapshot {
                status: Some("open".into()),
                description: Some(adf("otra cosa, editada en el board")),
                ..Default::default()
            },
        ),
        (
            "ACC-102".to_string(),
            Snapshot {
                status: Some("open".into()),
                description: Some(adf("y este igual")),
                ..Default::default()
            },
        ),
    ]));
    let r = check_ref(dir.path(), "refs/heads/insecure/all", &p, &estados(), BASE).unwrap();
    assert_eq!(r.differences.len(), 1, "{:?}", r.differences.iter().map(|d| &d.key).collect::<Vec<_>>());
    assert_eq!(r.differences[0].key, "ACC-101");
    assert_eq!(r.differences[0].field, "cuerpo");
}

/// Un cuerpo que difiere dice **donde**: decir solo "difiere" deja al que
/// empuja comparando los dos cuerpos a ojo.
#[test]
fn un_cuerpo_que_difiere_dice_en_que_linea() {
    let dir = panorama("uno\n\ndos\n\ntres", "igual");
    let p = Rico(HashMap::from([
        (
            "ACC-101".to_string(),
            Snapshot {
                status: Some("open".into()),
                description: Some(adf("uno\n\nDOS\n\ntres")),
                ..Default::default()
            },
        ),
        ("ACC-102".to_string(), Snapshot { status: Some("open".into()), ..Default::default() }),
    ]));
    let r = check_ref(dir.path(), "refs/heads/insecure/all", &p, &estados(), BASE).unwrap();
    assert_eq!(r.differences.len(), 1);
    let d = &r.differences[0];
    assert_eq!(d.field, "cuerpo");
    assert_eq!(d.line, Some(3), "la tercera linea es la que cambia");
    assert_eq!(d.tip, "dos");
    assert_eq!(d.live, "DOS");
}

/// Una clave que el proveedor no informa **no se cuenta como comparada**: no es
/// una que coincida, es una que no se vio. Ver `commands/push-states.md`.
#[test]
fn una_clave_que_el_proveedor_no_informa_no_cuenta_como_comparada() {
    let dir = panorama("igual", "igual");
    let p = Rico(HashMap::from([(
        "ACC-101".to_string(),
        Snapshot { status: Some("open".into()), ..Default::default() },
    )]));
    let r = check_ref(dir.path(), "refs/heads/insecure/all", &p, &estados(), BASE).unwrap();
    assert_eq!(r.compared, 1);
    assert_eq!(r.uninformed, vec!["ACC-102".to_string()]);
    assert!(r.differences.is_empty());
}

/// **El borde traduce.** En el arbol un item cita a otro por su archivo; en el
/// proveedor eso es una URL. Comparar el archivo crudo contra lo que el
/// proveedor devuelve los llama distintos — y medido el 2026-09-07 eso hacia
/// "diferir" a 248 de 295 items. Ver `concepts/sync.md`.
#[test]
fn un_link_a_otro_item_no_es_una_diferencia() {
    let dir = panorama("mira [`ACC-102`](ACC-102.task.md) que lo dice", "igual");
    let p = Rico(HashMap::from([
        (
            "ACC-101".to_string(),
            Snapshot {
                status: Some("open".into()),
                description: Some(adf(&format!(
                    "mira [`ACC-102`]({BASE}/browse/ACC-102) que lo dice"
                ))),
                ..Default::default()
            },
        ),
        ("ACC-102".to_string(), Snapshot { status: Some("open".into()), ..Default::default() }),
    ]));
    let r = check_ref(dir.path(), "refs/heads/insecure/all", &p, &estados(), BASE).unwrap();
    assert!(
        r.differences.is_empty(),
        "el link traducido es el mismo cuerpo: {:?}",
        r.differences.first().map(|d| (&d.key, d.line, &d.tip, &d.live))
    );
}
