//! Lo que el proveedor perdió sale del árbol, y **no se borra**.
//!
//! Ver `concepts/composition.md`.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::Command;
use worklist_provider::provider::{Existencia, Provider, Snapshot};
use worklist_provider::removes::{recolectar, Paso, DIR};

fn run(repo: &Path, args: &[&str]) {
    let st = Command::new("git").arg("-C").arg(repo).args(args).status().unwrap();
    assert!(st.success(), "git {args:?}");
}

/// Un proveedor que contesta lo que el test quiere sobre cada clave.
struct Board(BTreeMap<String, Existencia>);

impl Provider for Board {
    fn snapshot(&self, _keys: &[String]) -> anyhow::Result<HashMap<String, Snapshot>> {
        Ok(HashMap::new())
    }
    fn existe(&self, key: &str) -> anyhow::Result<Option<Existencia>> {
        Ok(self.0.get(key).copied())
    }
}

fn ventana() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path().join("repo");
    std::fs::create_dir(&r).unwrap();
    run(&r, &["init", "-q", "-b", "secure/sprint/1"]);
    run(&r, &["config", "user.email", "t@t"]);
    run(&r, &["config", "user.name", "t"]);
    for k in ["ACC-1", "ACC-2"] {
        std::fs::write(
            r.join(format!("{k}.task.md")),
            format!("---\ntitle: {k}\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\ncuerpo de {k}\n"),
        )
        .unwrap();
    }
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-qm", "arbol"]);
    (dir, r)
}

fn en_rama(r: &Path, archivo: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(r)
        .args(["show", &format!("refs/heads/secure/sprint/1:{archivo}")])
        .output()
        .unwrap();
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Un 404 saca el ítem del árbol, y **su contenido queda a la vista**: el
/// archivo se mueve, no se destruye.
#[test]
fn un_404_saca_el_item_y_conserva_su_contenido() {
    let (_d, r) = ventana();
    let mut board = BTreeMap::new();
    board.insert("ACC-1".to_string(), Existencia::Borrada);
    board.insert("ACC-2".to_string(), Existencia::Si);

    let out = recolectar(&r, "refs/heads/secure/sprint/1", &Board(board), false).unwrap();

    assert_eq!(out.sacados(), 1, "{:?}", out.pasos);
    assert!(en_rama(&r, "ACC-1.task.md").is_none(), "sigue siendo un item del arbol");
    let guardado = en_rama(&r, &format!("{DIR}/ACC-1.task.md")).expect("no quedo en removes");
    assert!(guardado.contains("cuerpo de ACC-1"), "se perdio el contenido: {guardado}");
    // Y el que sí existe no se toca.
    assert!(en_rama(&r, "ACC-2.task.md").is_some());
}

/// **El caso que no puede fallar.** El proveedor dice *"no existe o no tienes
/// permiso para verla"* para los dos, y el código los distingue: un 403 es una
/// credencial sin permiso, y sacar un ítem por eso destruiría trabajo.
#[test]
fn un_403_no_saca_nada_y_lo_dice() {
    let (_d, r) = ventana();
    let mut board = BTreeMap::new();
    board.insert("ACC-1".to_string(), Existencia::SinPermiso);
    board.insert("ACC-2".to_string(), Existencia::SinPermiso);

    let out = recolectar(&r, "refs/heads/secure/sprint/1", &Board(board), false).unwrap();

    assert_eq!(out.sacados(), 0, "saco un item por un permiso: {:?}", out.pasos);
    assert!(out.commit.is_none(), "escribio igual");
    assert!(en_rama(&r, "ACC-1.task.md").is_some(), "borro por falta de permiso");
    assert!(
        out.pasos.iter().all(|p| matches!(p, Paso::SinPermiso { .. })),
        "no lo reporto: {:?}",
        out.pasos
    );
}

/// Un proveedor que no puede contestar **no dice que existan**: no se pregunto.
/// Es la misma regla que `sin verificar` contra `coincide`, con el costo subido
/// a destruir.
#[test]
fn un_proveedor_que_no_contesta_no_saca_nada() {
    let (_d, r) = ventana();
    // El board vacío: no sabe de ninguna de las dos claves.
    let out = recolectar(&r, "refs/heads/secure/sprint/1", &Board(BTreeMap::new()), false).unwrap();

    assert_eq!(out.sacados(), 0);
    assert_eq!(out.claves, 2);
    assert!(
        out.pasos.iter().all(|p| matches!(p, Paso::NoSePudoPreguntar { .. })),
        "callo que no pudo preguntar: {:?}",
        out.pasos
    );
    assert!(en_rama(&r, "ACC-1.task.md").is_some());
}

/// `--dry-run` dice y no mueve.
#[test]
fn el_dry_run_no_mueve() {
    let (_d, r) = ventana();
    let mut board = BTreeMap::new();
    board.insert("ACC-1".to_string(), Existencia::Borrada);

    let out = recolectar(&r, "refs/heads/secure/sprint/1", &Board(board), true).unwrap();

    assert_eq!(out.sacados(), 1, "no dijo que sacaria");
    assert!(out.commit.is_none());
    assert!(en_rama(&r, "ACC-1.task.md").is_some(), "movio igual");
}

/// Y devolverlo es moverlo de nuevo: git lo registra como rename, así que el
/// contenido y la historia siguen ahí.
#[test]
fn devolverlo_es_moverlo_de_nuevo() {
    let (_d, r) = ventana();
    let mut board = BTreeMap::new();
    board.insert("ACC-1".to_string(), Existencia::Borrada);
    recolectar(&r, "refs/heads/secure/sprint/1", &Board(board), false).unwrap();

    // La rama ya está checkouteada acá, así que se mueve en el propio árbol.
    run(&r, &["reset", "--hard", "--quiet", "refs/heads/secure/sprint/1"]);
    run(&r, &["mv", &format!("{DIR}/ACC-1.task.md"), "ACC-1.task.md"]);
    run(&r, &["commit", "-aqm", "el 404 era un error: vuelve"]);

    let vuelto = en_rama(&r, "ACC-1.task.md").expect("no volvio");
    assert!(vuelto.contains("cuerpo de ACC-1"));
}

/// **Sacar el archivo sin sacar la referencia deja el sprint roto**: el próximo
/// recorte falla con *"la composición nombra a X, y no está"*. No es una
/// mejora, es un requisito.
#[test]
fn el_item_sacado_tambien_sale_del_items() {
    let (_d, r) = ventana();
    // La composición nombra a las dos, como el panorama de verdad.
    std::fs::create_dir_all(r.join(".metadata")).unwrap();
    let mut p = worklist_core::product::Product::default();
    p.sprints.push(worklist_core::product::Sprint {
        id: "1".into(),
        name: "1-el-primero".into(),
        status: "in-progress".into(),
        key: Some("6505".into()),
        items: vec!["ACC-1".into(), "ACC-2".into()],
    });
    p.backlog.push("ACC-1".into());
    std::fs::write(r.join(worklist_core::product::ARCHIVO), p.to_yaml().unwrap()).unwrap();
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-qm", "la composicion"]);

    let mut board = BTreeMap::new();
    board.insert("ACC-1".to_string(), Existencia::Borrada);
    board.insert("ACC-2".to_string(), Existencia::Si);
    recolectar(&r, "refs/heads/secure/sprint/1", &Board(board), false).unwrap();

    let yaml = en_rama(&r, worklist_core::product::ARCHIVO).expect("quedo la composicion");
    let quedó = worklist_core::product::de_yaml(&yaml).unwrap();
    assert_eq!(quedó.sprints[0].items, vec!["ACC-2".to_string()], "sigue nombrando la sacada");
    assert!(quedó.backlog.is_empty(), "quedo en el backlog: {:?}", quedó.backlog);
}

// --- la membresía: el proveedor manda ---

/// El board con un sprint que tiene estas claves adentro.
///
/// Se usa el `Spy` de `common`, que ya modela la membresía: entrar a un sprint
/// es salir del anterior. Reimplementar el trait acá sería un segundo board que
/// puede diferir del que usan los demás tests.
fn board_con(sprint: &str, keys: &[&str]) -> common::Spy {
    let spy = common::Spy::default();
    spy.inside
        .borrow_mut()
        .push((sprint.to_string(), keys.iter().map(|k| k.to_string()).collect()));
    spy
}

fn con_composicion(items: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    let (d, r) = ventana();
    let mut p = worklist_core::product::Product::default();
    p.sprints.push(worklist_core::product::Sprint {
        id: "21".into(),
        name: "21-el-sprint".into(),
        status: "in-progress".into(),
        key: Some("6525".into()),
        items: items.iter().map(|s| s.to_string()).collect(),
    });
    std::fs::create_dir_all(r.join(".metadata")).unwrap();
    std::fs::write(r.join(worklist_core::product::ARCHIVO), p.to_yaml().unwrap()).unwrap();
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-qm", "la composicion"]);
    (d, r)
}

fn items_del_21(r: &Path) -> Vec<String> {
    let yaml = en_rama(r, worklist_core::product::ARCHIVO).unwrap();
    worklist_core::product::de_yaml(&yaml).unwrap().sprints[0].items.clone()
}

/// **El caso que el usuario encontró**: sacó tareas del sprint en Jira y
/// volvían, porque la pasada agrega lo que la composición nombra y el board no
/// tiene. Medido: seis volvieron en una sola corrida.
#[test]
fn una_baja_en_el_board_sale_del_items() {
    let (_d, r) = con_composicion(&["ACC-1", "ACC-2", "ACC-3"]);
    let (pasos, commit) = worklist_provider::absorb::membresia(
        &r,
        "refs/heads/secure/sprint/1",
        &board_con("6525", &["ACC-1", "ACC-3"]),
        "701",
        &SinLector,
        None,
        false,
    )
    .unwrap();

    assert!(commit.is_some(), "no escribio: {pasos:?}");
    assert_eq!(items_del_21(&r), vec!["ACC-1".to_string(), "ACC-3".to_string()]);
}

/// **Y un alta también la manda el proveedor.**
///
/// Esto primero se reportaba y no se aplicaba, con el argumento de que entrar a
/// un sprint es planificar. Era acotar la regla: si el proveedor manda, mover
/// un ítem *hacia* un sprint es el proveedor moviendo algo igual que sacarlo.
#[test]
fn un_alta_en_el_board_entra_al_items() {
    let (_d, r) = con_composicion(&["ACC-1"]);
    let (_pasos, commit) = worklist_provider::absorb::membresia(
        &r,
        "refs/heads/secure/sprint/1",
        &board_con("6525", &["ACC-1", "ACC-2"]),
        "701",
        &SinLector,
        None,
        false,
    )
    .unwrap();

    assert!(commit.is_some(), "no aplico el alta");
    assert_eq!(items_del_21(&r), vec!["ACC-1".to_string(), "ACC-2".to_string()]);
}

/// **Un issue que nace en el board se adopta y entra.**
///
/// El proveedor es la autoridad, así que uno que existe allá y no acá es un
/// ítem que falta. Antes esto se reportaba y no se hacía, con el argumento de
/// que el worklist era la fuente de la existencia — una premisa que nadie
/// decidió, y que la spec ya había marcado como tal una vez.
#[test]
fn un_issue_que_nace_en_el_board_se_adopta_y_entra() {
    let (_d, r) = con_composicion(&["ACC-1"]);
    let mut dice = BTreeMap::new();
    dice.insert(
        "ACC-9".to_string(),
        Snapshot {
            status: Some("Tareas por hacer".into()),
            summary: Some("nacio en el board".into()),
            description: None,
            issue_type: Some("Tarea".into()),
        },
    );

    let (_pasos, commit) = worklist_provider::absorb::membresia(
        &r,
        "refs/heads/secure/sprint/1",
        &board_con("6525", &["ACC-1", "ACC-9"]),
        "701",
        &Lector(dice),
        Some("21"),
        false,
    )
    .unwrap();

    assert!(commit.is_some(), "no escribio");
    assert!(items_del_21(&r).iter().any(|i| i == "ACC-9"), "no entro al items");
    let texto = en_rama(&r, "ACC-9.task.md").expect("no nacio el archivo");
    assert!(texto.contains("nacio en el board"), "sin titulo: {texto}");
    // El cuerpo del board **no** baja todavia: el round-trip no cierra.
    assert!(texto.contains("El cuerpo no bajo todavia"), "{texto}");
}

/// Y un tipo que el worklist no modela **no se adopta**: pediría decidir a qué
/// se parece, y eso lo decide una persona.
#[test]
fn un_tipo_que_el_worklist_no_modela_no_se_adopta() {
    let (_d, r) = con_composicion(&["ACC-1"]);
    let mut dice = BTreeMap::new();
    dice.insert(
        "ACC-9".to_string(),
        Snapshot {
            status: None,
            summary: Some("un bug del board".into()),
            description: None,
            issue_type: Some("Bug".into()),
        },
    );

    let (pasos, _) = worklist_provider::absorb::membresia(
        &r,
        "refs/heads/secure/sprint/1",
        &board_con("6525", &["ACC-1", "ACC-9"]),
        "701",
        &Lector(dice),
        Some("21"),
        false,
    )
    .unwrap();

    assert!(!items_del_21(&r).iter().any(|i| i == "ACC-9"), "adopto un tipo que no modela");
    assert!(format!("{pasos:?}").contains("Bug"), "no dijo por que: {pasos:?}");
}

/// **La guarda que más importa.** El board contesta vacío sobre un sprint que
/// el `items` dice que tiene tres, y **no se saca ninguno**.
///
/// Sacar todos es de otra magnitud que sacar uno, y una lectura vacía no se
/// distingue de una que no anduvo: un 200 con lista vacía se ve igual que un
/// sprint que existe y está vacío de verdad.
#[test]
fn un_board_que_contesta_vacio_no_vacia_el_items() {
    let (_d, r) = con_composicion(&["ACC-1", "ACC-2", "ACC-3"]);

    let (pasos, commit) = worklist_provider::absorb::membresia(
        &r,
        "refs/heads/secure/sprint/1",
        &common::Spy::default(),
        "701",
        &SinLector,
        None,
        false,
    )
    .unwrap();

    assert!(commit.is_none(), "vacio el items entero");
    assert_eq!(items_del_21(&r).len(), 3, "saco items por una lista vacia");
    assert!(
        pasos
            .iter()
            .any(|p| matches!(p, worklist_provider::absorb::Membresia::VacioSospechoso { .. })),
        "no lo reporto: {pasos:?}"
    );
}

/// **El bug que una tarea nueva habría pagado.**
///
/// Un ítem con `@` no tiene clave, así que el board **nunca** puede tenerlo en
/// su sprint. Comparar sin filtrarlo lo trataba como una baja y lo sacaba del
/// `items`: crear una tarea y correr `pull` la hacía desaparecer del sprint.
///
/// Es la misma regla que el resto: *no se preguntó* no es *no está*.
#[test]
fn una_tarea_nueva_sin_clave_no_se_saca_del_items() {
    let (_d, r) = con_composicion(&["ACC-1", "@la-tarea-nueva"]);

    let (pasos, commit) = worklist_provider::absorb::membresia(
        &r,
        "refs/heads/secure/sprint/1",
        &board_con("6525", &["ACC-1"]),
        "701",
        &SinLector,
        None,
        false,
    )
    .unwrap();

    assert!(commit.is_none(), "escribio: {pasos:?}");
    assert!(
        items_del_21(&r).iter().any(|i| i == "@la-tarea-nueva"),
        "saco la tarea nueva del sprint: {:?}",
        items_del_21(&r)
    );
    assert!(
        pasos.iter().any(|p| matches!(p, worklist_provider::absorb::Membresia::SinClave { .. })),
        "callo que habia una esperando cruzar: {pasos:?}"
    );
}

/// **El alcance es lo que hace que un `pull` no cueste el proyecto entero.**
///
/// `sprint_items` es una llamada por sprint —1.364s medidos el 2026-09-07— así
/// que recorrer los 22 son 31s para contestar una pregunta sobre uno. Y
/// `pull --all` lo multiplicaba: veinte vistas por veintidós sprints son 440
/// requests.
#[test]
fn acotado_a_un_sprint_no_pregunta_por_los_demas() {
    let (_d, r) = ventana();
    // Dos sprints en la composición, y el board contesta por los dos.
    let mut p = worklist_core::product::Product::default();
    for (id, key, items) in
        [("21", "6525", vec!["ACC-1"]), ("22", "6526", vec!["ACC-2"])]
    {
        p.sprints.push(worklist_core::product::Sprint {
            id: id.into(),
            name: format!("{id}-el-sprint"),
            status: "open".into(),
            key: Some(key.into()),
            items: items.iter().map(|s| s.to_string()).collect(),
        });
    }
    std::fs::create_dir_all(r.join(".metadata")).unwrap();
    std::fs::write(r.join(worklist_core::product::ARCHIVO), p.to_yaml().unwrap()).unwrap();
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-qm", "dos sprints"]);

    // El board tiene los dos vacíos de lo que la composición nombra, así que
    // sin acotar los dos darían un paso. Acotado, sólo el 21.
    let spy = common::Spy::default();
    spy.inside.borrow_mut().push(("6525".into(), vec!["ACC-1".into(), "ACC-9".into()]));
    spy.inside.borrow_mut().push(("6526".into(), vec!["ACC-2".into(), "ACC-8".into()]));

    let (pasos, _) = worklist_provider::absorb::membresia(
        &r,
        "refs/heads/secure/sprint/1",
        &spy,
        "701",
        &SinLector,
        Some("21"),
        false,
    )
    .unwrap();

    assert!(!pasos.is_empty(), "no miro ni el suyo");
    assert!(
        !format!("{pasos:?}").contains("\"22\""),
        "pregunto por un sprint que no es el de la vista: {pasos:?}"
    );
}

/// Un lector que no informa nada: los tests de membresía no prueban adopción.
struct SinLector;

impl Provider for SinLector {
    fn snapshot(&self, _keys: &[String]) -> anyhow::Result<HashMap<String, Snapshot>> {
        Ok(HashMap::new())
    }
}

/// Un lector que contesta lo que el test le pone.
struct Lector(BTreeMap<String, Snapshot>);

impl Provider for Lector {
    fn snapshot(&self, keys: &[String]) -> anyhow::Result<HashMap<String, Snapshot>> {
        Ok(keys.iter().filter_map(|k| self.0.get(k).map(|s| (k.clone(), s.clone()))).collect())
    }
}
