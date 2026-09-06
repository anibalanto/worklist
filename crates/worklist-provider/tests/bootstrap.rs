//! El bootstrap: clave del proveedor para lo que no la tiene, sobre el
//! panorama y sin prometer que la rama se verifique.
//!
//! Es la separacion que pide `5n`: *tener clave* es del item, *verificarse
//! entera* es de la rama. Ver `concepts/sync.md`.

mod common;

use common::{arbol, Spy};
use worklist_provider::assign::bootstrap;

const REF: &str = "refs/heads/insecure/all";

fn correr(dir: &std::path::Path, spy: &Spy) -> Option<worklist_provider::assign::BootstrapResult> {
    bootstrap(dir, REF, "https://x", spy, "701", None, false).unwrap()
}

#[test]
fn le_da_clave_al_panorama_entero_sin_pedirle_un_sprint() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    let res = correr(&r, &spy).expect("hay cuatro pedidos");

    assert_eq!(res.assigned.len(), 4, "la epica, la US y las dos tasks");
    let listado = common::show_tree(&r, REF);
    assert!(listado.contains("ACC-"), "los archivos quedaron renombrados: {listado}");
    assert!(!listado.contains("\n1.epic.md"), "el slug ya no esta: {listado}");
    // Y nada de lo que promete sincronizacion: ni sprint, ni vinculos.
    assert!(spy.sprinted.borrow().is_empty(), "el bootstrap no toca sprints");
    assert!(spy.blocks.borrow().is_empty(), "ni vinculos");
}

#[test]
fn la_epica_va_primero_y_los_hijos_le_cuelgan() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    let res = correr(&r, &spy).unwrap();

    assert_eq!(res.order[0], "@1", "el orden topologico pone la epica adelante");
    let epica = &res.assigned.iter().find(|a| a.slug == "@1").unwrap().key;
    for slug in ["@n", "@o", "@q"] {
        let a = res.assigned.iter().find(|a| a.slug == slug).unwrap();
        assert_eq!(a.parent.as_deref(), Some(epica.as_str()), "{slug} cuelga de la epica");
    }
}

#[test]
fn correrlo_de_nuevo_no_encuentra_nada_que_hacer() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    correr(&r, &spy).unwrap();
    let creados = spy.created.borrow().len();

    assert!(bootstrap(&r, REF, "https://x", &spy, "701", None, false).unwrap().is_none());
    assert_eq!(spy.created.borrow().len(), creados, "no le pidio nada mas al proveedor");
}

/// **Encontrado no es creado.** Sobre un issue que ya existia no hay
/// compare-and-swap detras que pruebe que partimos del estado actual, asi que
/// el cuerpo no viaja: escribirlo seria pisar lo que alguien edito en el board.
#[test]
fn el_cuerpo_viaja_solo_donde_el_issue_se_creo() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy { existing: vec![("@o.task.md".into(), "ACC-99".into())], ..Spy::default() };

    let res = correr(&r, &spy).unwrap();

    let encontrado = res.assigned.iter().find(|a| a.slug == "@o").unwrap();
    assert_eq!(encontrado.key, "ACC-99");
    assert!(!encontrado.created, "lo encontro, no lo creo");

    let descripciones = spy.descriptions.borrow();
    assert!(
        !descripciones.iter().any(|(k, _)| k == "ACC-99"),
        "sobre lo que ya existia el cuerpo no se toca: {descripciones:?}"
    );
    assert_eq!(descripciones.len(), 3, "y sobre los tres creados si: {descripciones:?}");
}

/// Y el panorama sigue avanzando solo por escritura del servidor: el bootstrap
/// mueve la ref, no empuja.
#[test]
fn deja_la_ref_del_panorama_movida() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    let res = correr(&r, &spy).unwrap();
    assert_ne!(res.new_head, res.old_head);

    let head = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["rev-parse", REF])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(head.trim(), res.new_head);
}

/// **Parado en la rama se trabaja en el arbol**, como cualquier commit. Es el
/// caso real de hoy: el panorama es un worktree del clon, no una rama del bare.
#[test]
fn parado_en_el_panorama_escribe_en_el_arbol() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    correr(&r, &spy).unwrap();

    // El worktree quedo sano: los archivos renombrados en disco, y nada
    // "modificado" que nadie toco — que es el defecto de `5o`.
    assert!(r.join("ACC-1.epic.md").exists(), "el renombre esta en el arbol");
    assert!(!r.join("@1.epic.md").exists());
    let status = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["status", "--short"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(status.trim().is_empty(), "sin cambios fantasma: {status:?}");
}

/// Y no mueve una rama que otro worktree tiene abierta: serian ciento y pico
/// de renombres dejando ese indice apuntando al arbol anterior.
#[test]
fn no_mueve_el_panorama_si_lo_tiene_otro_worktree() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let otro = dir.path().join("otro");
    // Primero nos salimos de la rama —para caer en el camino del worktree
    // temporal— y recien ahi otro la toma.
    common::run(&r, &["checkout", "-q", "--detach"]);
    common::run(&r, &["worktree", "add", "-q", otro.to_str().unwrap(), "insecure/all"]);

    let spy = Spy::default();
    let err = bootstrap(&r, REF, "https://x", &spy, "701", None, false).unwrap_err().to_string();
    assert!(err.contains("otro worktree"), "tiene que nombrar el problema: {err}");
    assert!(err.contains("otro"), "y donde esta: {err}");
    assert!(spy.created.borrow().is_empty(), "y no le pidio nada al proveedor");
}

/// El arbol sucio tampoco: `rename_one` commitea con `add -A`, asi que lo que
/// hubiera sin commitear se colaria adentro del renombre.
#[test]
fn no_corre_sobre_un_arbol_sucio() {
    let dir = arbol();
    let r = dir.path().join("repo");
    std::fs::write(r.join("sin-commitear.txt"), "algo").unwrap();

    let spy = Spy::default();
    let err = bootstrap(&r, REF, "https://x", &spy, "701", None, false).unwrap_err().to_string();
    assert!(err.contains("sin commitear"), "{err}");
    assert!(spy.created.borrow().is_empty());
}

/// Un proveedor que crea bien las primeras `n` veces y despues falla, como se
/// cayo la corrida real: 23 issues creados y el ítem 24 rompiendo la JQL.
struct SeCaeEn {
    n: std::cell::Cell<usize>,
    hasta: usize,
}

impl worklist_provider::board::Board for SeCaeEn {
    fn create_or_find(
        &self,
        title: &str,
        _t: &str,
        _d: &str,
        _p: Option<&str>,
    ) -> anyhow::Result<worklist_provider::board::Assignment> {
        let i = self.n.get();
        if i >= self.hasta {
            anyhow::bail!("buscar por titulo fallo por acli: {title}");
        }
        self.n.set(i + 1);
        Ok(worklist_provider::board::Assignment::Created(format!("ACC-{}", 100 + i)))
    }
    fn find(&self, _t: &str) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
    fn link_relates(&self, _a: &str, _b: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
    fn link_blocks(&self, _a: &str, _b: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
    fn set_summary(&self, _k: &str, _t: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn set_description(&self, _k: &str, _a: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn parent_of(&self, _k: &str) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
    fn set_parent(&self, _k: &str, _e: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    fn transition(
        &self,
        _k: &str,
        _d: &worklist_provider::states::Destino,
    ) -> anyhow::Result<worklist_provider::board::Transicion> {
        Ok(worklist_provider::board::Transicion::Hecha)
    }
    fn create_or_find_sprint(&self, _b: &str, _n: &str) -> anyhow::Result<(String, bool)> {
        Ok(("1".into(), false))
    }
    fn add_to_sprint(&self, _s: &str, _k: &[&str]) -> anyhow::Result<usize> {
        Ok(0)
    }
    fn sprint_items(&self, _b: &str, _s: &str) -> anyhow::Result<Vec<String>> {
        Ok(vec![])
    }
}

/// La propiedad de `7k`: crear un issue es un efecto afuera, irreversible y
/// pago. Una corrida que se cae no puede descartar las claves que ya consiguió
/// — quedarían **sólo del otro lado**.
#[test]
fn una_corrida_que_se_cae_deja_en_el_panorama_lo_que_alcanzo_a_conseguir() {
    let dir = arbol();
    let r = dir.path().join("repo");
    // Corrido desde un repo donde la rama **no** está checkouteada, que es el
    // caso del servidor y el único donde se perdía.
    let bare = dir.path().join("bare.git");
    common::run(&r, &["clone", "--bare", "-q", ".", bare.to_str().unwrap()]);
    let antes = common::show_tree(&bare, REF);

    let board = SeCaeEn { n: std::cell::Cell::new(0), hasta: 2 };
    let e = bootstrap(&bare, REF, "https://x", &board, "701", None, false).unwrap_err();

    assert!(e.to_string().contains("fallo por acli"), "se cayó como se esperaba: {e}");
    let despues = common::show_tree(&bare, REF);
    assert_ne!(antes, despues, "**el panorama avanzó**: las dos claves conseguidas están");
    assert!(despues.contains("ACC-100"), "la primera quedó: {despues}");
    assert!(despues.contains("ACC-101"), "y la segunda: {despues}");
}

/// El corte es para mirar, no para poder deshacer: lo que queda sigue sin
/// clave, así que la corrida siguiente lo toma sin contabilidad extra.
#[test]
fn limit_crea_los_primeros_y_para_y_la_siguiente_sigue() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    let uno = bootstrap(&r, REF, "https://x", &spy, "701", Some(2), false).unwrap().unwrap();
    assert_eq!(uno.assigned.len(), 2, "dos y para, de los cuatro");
    assert_eq!(spy.created.borrow().len(), 2, "y sólo dos issues creados");

    // Sin marca de progreso: lo que falta es lo que no tiene clave.
    let dos = bootstrap(&r, REF, "https://x", &spy, "701", None, false).unwrap().unwrap();
    assert_eq!(dos.assigned.len(), 2, "los dos que quedaban");
    assert_eq!(spy.created.borrow().len(), 4, "cuatro en total, ninguno dos veces");

    assert!(bootstrap(&r, REF, "https://x", &spy, "701", None, false).unwrap().is_none());
}

/// El corte va después del orden topológico: una épica creada con sus tasks
/// sin crear es válido, y al revés el orden lo impide.
#[test]
fn el_corte_respeta_el_orden_y_la_epica_va_en_el_primer_lote() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    let uno = bootstrap(&r, REF, "https://x", &spy, "701", Some(1), false).unwrap().unwrap();

    assert_eq!(uno.assigned[0].slug, "@1", "la épica es la primera del orden");
    // Y la task que le cuelga, creada en el lote siguiente, la encuentra puesta.
    let dos = bootstrap(&r, REF, "https://x", &spy, "701", Some(1), false).unwrap().unwrap();
    assert!(dos.assigned[0].parent.is_some(), "el --parent sale de la épica ya creada");
}

/// El defecto de `7p`: la épica ya cruzó, así que no está entre los pedidos —
/// y buscar el ancestro sólo ahí la volvía invisible. Once ítems del panorama
/// real se habrían creado sueltos.
#[test]
fn una_epica_que_ya_tiene_clave_sigue_siendo_el_parent_de_sus_tasks() {
    let dir = arbol();
    let r = dir.path().join("repo");
    let spy = Spy::default();

    // Primer lote: sólo la épica. Queda con clave y fuera de los pedidos.
    let uno = bootstrap(&r, REF, "https://x", &spy, "701", Some(1), false).unwrap().unwrap();
    let clave_epica = uno.assigned[0].key.clone();
    assert!(common::show_tree(&r, REF).contains(&format!("{clave_epica}.epic.md")));

    // Y lo que sigue la nombra igual.
    let dos = bootstrap(&r, REF, "https://x", &spy, "701", None, false).unwrap().unwrap();
    for a in &dos.assigned {
        assert_eq!(
            a.parent.as_deref(),
            Some(clave_epica.as_str()),
            "{} se creó sin la épica que ya tenía clave",
            a.slug
        );
    }
    // Y le llegó al proveedor en la creación, que es el único momento en que viaja.
    for (_, _, parent) in spy.created.borrow().iter().skip(1) {
        assert_eq!(parent.as_deref(), Some(clave_epica.as_str()));
    }
}

/// `ACC-299`: un sprint sin ventana no tenía camino al proveedor. El board
/// mostró 22 sprints en el worklist y 17 en Jira, con el que estaba en curso
/// entre los que faltaban.
#[test]
fn los_sprints_sin_key_tambien_cruzan_y_van_al_final() {
    let dir = arbol();
    let r = dir.path().join("repo");
    // El árbol trae `@1.epic`, `@n.user-story`, `@o.task` y `@q.task`; le pongo
    // un sprint que nombra a la user story, que entra con su subárbol. El
    // sprint no lleva marca: su nombre no lo reemplaza el proveedor.
    common::sprint(&r, "1", "el primero", &["@n"], None);
    common::run(&r, &["add", "-A"]);
    common::run(&r, &["commit", "-qm", "un sprint sin key"]);
    let spy = Spy::default();

    let res = bootstrap(&r, REF, "https://x", &spy, "701", None, false).unwrap().unwrap();

    assert_eq!(res.sprints.len(), 1, "el sprint sin key también es un pedido");
    let sp = &res.sprints[0];
    assert!(sp.created, "no existía del otro lado");
    assert!(!sp.added.is_empty(), "y le entraron los issues de sus ítems");
    // Va al final: lo que le mete adentro son claves, que la pasada 1 acaba de
    // escribir. Si corriera antes, no habría qué meter.
    for k in &sp.added {
        assert!(k.starts_with("ACC-"), "entró una clave, no un slug: {k}");
    }
    // Y el `key` quedó anotado, así que la segunda corrida no lo vuelve a crear.
    let sprint_md = common::show(&r, REF, "_sprints/1.sprint.md");
    assert!(sprint_md.contains(&format!("key: {}", sp.key)), "falta el key:\n{sprint_md}");
    // La segunda corrida ya no devuelve `None` —los sprints se reconcilian
    // siempre— pero no mueve nada: la idempotencia la da la pasada, no el
    // filtro. Ver `ACC-301`.
    let otra = bootstrap(&r, REF, "https://x", &spy, "701", None, false).unwrap().unwrap();
    assert!(otra.assigned.is_empty(), "no queda ningun item que pedir");
    assert!(otra.sprints.iter().all(|s| !s.created && s.added.is_empty()), "y el sprint quieto");
    assert_eq!(spy.created_sprints.borrow().len(), 1, "creado una sola vez");
}

/// `ACC-301`: el sprint 20 tenía cinco ítems en el worklist y cuatro en Jira,
/// porque el quinto entró después de que el sprint cruzara. La clave se pide
/// una vez; los miembros se reconcilian siempre.
#[test]
fn un_sprint_que_ya_cruzo_y_gana_un_item_lo_sube_igual() {
    let dir = arbol();
    let r = dir.path().join("repo");
    common::sprint(&r, "1", "el primero", &["@n"], None);
    common::run(&r, &["add", "-A"]);
    common::run(&r, &["commit", "-qm", "un sprint"]);
    let spy = Spy::default();

    // Primera corrida: el sprint cruza con lo que tenía.
    let uno = bootstrap(&r, REF, "https://x", &spy, "701", None, false).unwrap().unwrap();
    let key = uno.sprints[0].key.clone();
    let antes = uno.sprints[0].added.len();

    // Y después gana un ítem, como pasó de verdad.
    let sp = r.join("_sprints/1.sprint.md");
    let texto = std::fs::read_to_string(&sp).unwrap();
    // `@q` es la única que el sprint no tenía: `@n` entró con su subárbol.
    let q = uno
        .assigned
        .iter()
        .find(|a| a.slug == "@q")
        .map(|a| a.key.clone())
        .expect("q cruzó en la primera corrida");
    std::fs::write(&sp, texto.replace("items: [", &format!("items: [{q}, "))).unwrap();
    common::run(&r, &["commit", "-aqm", "entra uno mas al sprint"]);

    let dos = bootstrap(&r, REF, "https://x", &spy, "701", None, false).unwrap().unwrap();

    let sprint = &dos.sprints[0];
    assert_eq!(sprint.key, key, "el mismo sprint, no uno nuevo");
    assert!(!sprint.created, "no se volvió a crear");
    assert!(
        sprint.added.contains(&q),
        "el ítem nuevo tiene que entrar; entraron {:?} (antes {antes})",
        sprint.added
    );
    assert_eq!(spy.created_sprints.borrow().len(), 1, "creado una sola vez");
}
