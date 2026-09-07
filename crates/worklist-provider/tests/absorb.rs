//! Lo que cambió en el board entra a la ventana — y lo que no se puede
//! absorber sin adivinar, no entra.
//!
//! Ver `commands/absorb.md`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use worklist_provider::absorb::{absorb, Paso};
use worklist_provider::provider::{FileProvider, Provider, Snapshot};
use worklist_provider::states::{Destino, Estados};

fn run(repo: &Path, args: &[&str]) {
    let st = Command::new("git").arg("-C").arg(repo).args(args).status().unwrap();
    assert!(st.success(), "git {args:?}");
}

/// Un proveedor que dice exactamente lo que el test quiere que diga.
struct Board(BTreeMap<String, Snapshot>);

impl Provider for Board {
    fn snapshot(
        &self,
        keys: &[String],
    ) -> anyhow::Result<std::collections::HashMap<String, Snapshot>> {
        Ok(keys.iter().filter_map(|k| self.0.get(k).map(|s| (k.clone(), s.clone()))).collect())
    }
}

fn item(repo: &Path, name: &str, title: &str, status: &str) {
    std::fs::write(
        repo.join(name),
        format!("---\ntitle: {title}\nstatus: {status}\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\ncuerpo\n"),
    )
    .unwrap();
}

/// El mapeo de esta instalación, que **no es inyectivo**: `done` y `dropped`
/// caen los dos en `Finalizada`.
fn estados_del_board() -> Estados {
    let mut m = BTreeMap::new();
    m.insert("open".into(), Destino::Status("Tareas por hacer".into()));
    m.insert("in-progress".into(), Destino::Status("En curso".into()));
    m.insert("done".into(), Destino::Status("Finalizada".into()));
    m.insert("dropped".into(), Destino::Status("Finalizada".into()));
    Estados::new(&["open".into(), "in-progress".into(), "done".into(), "dropped".into()], m)
        .unwrap()
}

fn ventana() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path().join("repo");
    std::fs::create_dir(&r).unwrap();
    run(&r, &["init", "-q", "-b", "secure/sprint/1"]);
    run(&r, &["config", "user.email", "t@t"]);
    run(&r, &["config", "user.name", "t"]);
    item(&r, "ACC-1.task.md", "el titulo de aca", "open");
    item(&r, "ACC-2.task.md", "otro", "in-progress");
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-qm", "arbol"]);
    (dir, r)
}

fn texto(r: &Path, archivo: &str) -> String {
    String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(r)
            .args(["show", &format!("refs/heads/secure/sprint/1:{archivo}")])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
}

#[test]
fn el_titulo_del_board_entra_a_la_ventana() {
    let (_d, r) = ventana();
    let mut board = BTreeMap::new();
    board.insert(
        "ACC-1".to_string(),
        Snapshot {
            status: Some("Tareas por hacer".into()),
            summary: Some("el titulo de alla".into()),
            description: None,
        },
    );

    let out =
        absorb(&r, "refs/heads/secure/sprint/1", &Board(board), &estados_del_board(), false)
            .unwrap();

    assert_eq!(out.absorbidos(), 1, "{:?}", out.pasos);
    assert!(out.commit.is_some(), "no dejó commit");
    assert!(
        texto(&r, "ACC-1.task.md").contains("el titulo de alla"),
        "no entró: {}",
        texto(&r, "ACC-1.task.md")
    );
}

/// **El límite que no es un defecto del comando.**
///
/// `Finalizada` vuelve a `done` **y** a `dropped`, así que absorberlo sería
/// elegir entre dos. Se reporta con las dos candidatas y no se toca nada.
#[test]
fn un_status_con_dos_vueltas_se_reporta_y_no_se_elige() {
    let (_d, r) = ventana();
    let mut board = BTreeMap::new();
    board.insert(
        "ACC-1".to_string(),
        Snapshot {
            status: Some("Finalizada".into()),
            summary: Some("el titulo de aca".into()),
            description: None,
        },
    );

    let out =
        absorb(&r, "refs/heads/secure/sprint/1", &Board(board), &estados_del_board(), false)
            .unwrap();

    assert_eq!(out.absorbidos(), 0, "eligió por su cuenta: {:?}", out.pasos);
    let Some(Paso::Reportado { porque, campo, .. }) = out.pasos.first() else {
        panic!("no reportó nada: {:?}", out.pasos)
    };
    assert_eq!(*campo, "status");
    assert!(porque.contains("done") && porque.contains("dropped"), "{porque}");
    assert!(texto(&r, "ACC-1.task.md").contains("status: open"), "tocó el archivo igual");
}

/// Y cuando la vuelta **es** única, sí se absorbe: `En curso` sólo puede venir
/// de `in-progress`.
#[test]
fn un_status_con_vuelta_unica_si_se_absorbe() {
    let (_d, r) = ventana();
    let mut board = BTreeMap::new();
    board.insert(
        "ACC-1".to_string(),
        Snapshot {
            status: Some("En curso".into()),
            summary: Some("el titulo de aca".into()),
            description: None,
        },
    );

    let out =
        absorb(&r, "refs/heads/secure/sprint/1", &Board(board), &estados_del_board(), false)
            .unwrap();

    assert_eq!(out.absorbidos(), 1, "{:?}", out.pasos);
    assert!(texto(&r, "ACC-1.task.md").contains("status: in-progress"));
}

/// Absorber pisa, así que no decide solo: si el ítem cambió de este lado
/// **después de la última propagación**, los dos lados escribieron.
#[test]
fn lo_que_cambio_de_este_lado_desde_la_marca_no_se_pisa() {
    let (_d, r) = ventana();
    // La marca queda donde está hoy: lo que venga después es trabajo sin subir.
    let marca = String::from_utf8(
        Command::new("git").arg("-C").arg(&r).args(["rev-parse", "HEAD"]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    run(
        &r,
        &["update-ref", "refs/worklist/propagated/secure/sprint/1", &marca],
    );
    item(&r, "ACC-1.task.md", "lo que escribi yo", "open");
    run(&r, &["commit", "-aqm", "edito ACC-1 y no lo empujo"]);

    let mut board = BTreeMap::new();
    board.insert(
        "ACC-1".to_string(),
        Snapshot {
            status: Some("Tareas por hacer".into()),
            summary: Some("lo que escribieron alla".into()),
            description: None,
        },
    );

    let out =
        absorb(&r, "refs/heads/secure/sprint/1", &Board(board), &estados_del_board(), false)
            .unwrap();

    assert_eq!(out.absorbidos(), 0, "pisó trabajo sin propagar: {:?}", out.pasos);
    assert!(texto(&r, "ACC-1.task.md").contains("lo que escribi yo"));
}

/// `--dry-run` dice y no escribe.
#[test]
fn el_dry_run_no_escribe() {
    let (_d, r) = ventana();
    let antes = texto(&r, "ACC-1.task.md");
    let mut board = BTreeMap::new();
    board.insert(
        "ACC-1".to_string(),
        Snapshot {
            status: Some("Tareas por hacer".into()),
            summary: Some("otro titulo".into()),
            description: None,
        },
    );

    let out =
        absorb(&r, "refs/heads/secure/sprint/1", &Board(board), &estados_del_board(), true).unwrap();

    assert_eq!(out.absorbidos(), 1, "no dijo qué haría");
    assert!(out.commit.is_none(), "escribió igual");
    assert_eq!(texto(&r, "ACC-1.task.md"), antes);
}

/// El de prueba no informa título ni cuerpo, y `None` **no es** "coinciden":
/// no hay nada contra qué comparar, así que no se absorbe nada.
#[test]
fn un_campo_que_el_proveedor_no_informa_no_se_absorbe() {
    let (_d, r) = ventana();
    let dir = tempfile::tempdir().unwrap();
    let archivo = dir.path().join("provider.json");
    std::fs::write(&archivo, r#"{"ACC-1":"open","ACC-2":"in-progress"}"#).unwrap();

    let out = absorb(
        &r,
        "refs/heads/secure/sprint/1",
        &FileProvider::new(&archivo),
        &Estados::identidad(&["open".into(), "in-progress".into()]),
        false,
    )
    .unwrap();

    assert_eq!(out.absorbidos(), 0, "{:?}", out.pasos);
    assert_eq!(out.reportados(), 0, "reportó sobre campos que nadie informó: {:?}", out.pasos);
}
