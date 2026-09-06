//! El sprint de la ventana, del otro lado.
//!
//! Un sprint del proveedor **no es un issue**: es un objeto propio, con su id
//! y su lista de miembros, y por eso tiene su propia pasada. Ver
//! `concepts/sync.md` seccion "El sprint viaja como sprint, no como issue".

mod common;

use common::{arbol, resolve, show, sprint, Spy};
use std::cell::RefCell;

/// Las claves que el sprint recibio, traducidas de vuelta a los slugs del
/// worklist: las pruebas hablan de `n` y `o`, no de `ACC-3`.
fn slugs(res: &worklist_provider::assign::WindowResult, keys: &[String]) -> Vec<String> {
    let mut out: Vec<String> = keys
        .iter()
        .map(|k| {
            res.assigned
                .iter()
                .find(|a| &a.key == k)
                .map(|a| a.slug.clone())
                .unwrap_or_else(|| k.clone())
        })
        .collect();
    out.sort();
    out
}

fn repo(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("repo")
}

fn con_sprint(items: &[&str], key: Option<&str>) -> tempfile::TempDir {
    let dir = arbol();
    let r = repo(&dir);
    sprint(&r, "3", "Los sprints en el board", items, key);
    common::run(&r, &["add", "-A"]);
    common::run(&r, &["commit", "-qm", "el sprint"]);
    dir
}

/// El sprint se crea, su id queda anotado en el `.sprint.md`, y adentro entra
/// el subarbol de lo que `items` nombra.
#[test]
fn el_sprint_se_crea_y_lleva_su_subarbol() {
    let dir = con_sprint(&["n"], None);
    let r = repo(&dir);
    let spy = Spy::default();
    let res = resolve(&r, &spy);

    let s = res.sprint.as_ref().expect("la ventana lleva un sprint");
    assert!(s.created, "no existia del otro lado");
    assert_eq!(s.id, "3");
    assert_eq!(slugs(&res, &s.added), vec!["n", "o"], "la user story con su task");
    assert_eq!(s.already, Some(0));

    // El id del proveedor queda en git, que es lo que hace que la proxima
    // corrida no vuelva a buscarlo.
    let texto = show(&r, &res.new_head, "_sprints/3.sprint.md");
    assert!(texto.contains(&format!("key: {}", s.key)), "{texto}");
}

/// **`items` nombra los topes, no los miembros.** La epica viaja en la ventana
/// de solo lectura, para que la cadena `parent` cierre adentro del recorte;
/// que este en el arbol no la pone en la iteracion.
#[test]
fn lo_que_viaja_de_solo_lectura_no_es_miembro() {
    let dir = con_sprint(&["n"], None);
    let spy = Spy::default();
    let res = resolve(&repo(&dir), &spy);
    let s = res.sprint.as_ref().unwrap();
    let dentro = slugs(&res, &s.added);
    assert!(!dentro.contains(&"1".to_string()), "la epica ancestro no entra: {dentro:?}");
    assert!(!dentro.contains(&"q".to_string()), "una task de otra rama tampoco: {dentro:?}");
}

/// El nombre del otro lado lo escribe el worklist, con la regla de siempre:
/// **nunca el id solo**, porque el que lee es el que menos contexto tiene.
#[test]
fn el_nombre_lleva_el_numero_y_el_titulo() {
    let dir = con_sprint(&["n"], None);
    let spy = Spy::default();
    resolve(&repo(&dir), &spy);
    assert_eq!(spy.board.borrow()[0].0, "3 Los sprints en el board");
}

/// Volver a correrlo **no duplica ni el sprint ni la membresia**, y de paso no
/// escribe nada: la lectura de antes deja el `sprint add` sin nada que mandar.
#[test]
fn volver_a_correrlo_no_duplica_ni_escribe() {
    let dir = con_sprint(&["n"], None);
    let r = repo(&dir);
    let spy = Spy::default();
    let primera = resolve(&r, &spy);
    let segunda = resolve(&r, &spy);

    let s = segunda.sprint.as_ref().expect("la segunda corrida tambien lo mira");
    assert_eq!(s.key, primera.sprint.as_ref().unwrap().key, "el mismo sprint");
    assert!(!s.created);
    assert!(s.added.is_empty(), "no habia nada que mandar: {:?}", s.added);
    assert_eq!(s.already, Some(2));
    assert_eq!(spy.created_sprints.borrow().len(), 1, "se creo una sola vez");
    assert_eq!(spy.sprinted.borrow().len(), 1, "y se escribio una sola vez");
}

/// **Que no haya nada que asignar no es que no haya nada que hacer.** Es el
/// caso que importa: las ventanas ya empujadas tienen sus issues creados y su
/// sprint sin existir, y las otras cuatro pasadas no tienen nada que hacer con
/// ellas.
#[test]
fn una_ventana_sin_pedidos_igual_reconcilia_el_sprint() {
    let dir = con_sprint(&["n"], None);
    let r = repo(&dir);
    let spy = Spy::default();
    let primera = resolve(&r, &spy);
    assert!(!primera.assigned.is_empty(), "la primera si tenia pedidos");

    let segunda = resolve(&r, &spy);
    assert!(segunda.assigned.is_empty(), "ya no queda ninguno");
    assert!(segunda.sprint.is_some(), "y aun asi la ventana se abrio, por el sprint");
}

/// Con `key` puesto no se busca nada: se usa. Es el mismo trato que la clave
/// de un item, que nadie vuelve a buscar por titulo.
#[test]
fn un_sprint_con_key_no_se_crea_ni_se_busca() {
    let dir = con_sprint(&["n"], Some("4127"));
    let spy = Spy::default();
    let res = resolve(&repo(&dir), &spy);
    let s = res.sprint.as_ref().unwrap();
    assert_eq!(s.key, "4127");
    assert!(!s.created);
    assert!(spy.created_sprints.borrow().is_empty(), "no se creo nada");
    assert!(spy.board.borrow().is_empty(), "ni se miro el board");
}

/// Sin `key` **creemos** que no existe, y creerlo no alcanza: una corrida que
/// crea el sprint y se cae antes de anotar su id lo dejo creado y sin clave.
/// Por eso se busca por nombre antes de crear — el mismo agujero que
/// `create-or-find` tapa para un issue.
#[test]
fn una_corrida_caida_no_deja_el_sprint_duplicado() {
    let dir = con_sprint(&["n"], None);
    let spy = Spy {
        // Lo que dejo la corrida anterior: el sprint creado, y el `key` que
        // nunca llego a git porque la ref no se movio.
        board: RefCell::new(vec![("3 Los sprints en el board".into(), "6501".into())]),
        ..Default::default()
    };
    let res = resolve(&repo(&dir), &spy);
    let s = res.sprint.as_ref().unwrap();
    assert_eq!(s.key, "6501", "es el que ya estaba");
    assert!(!s.created);
    assert!(spy.created_sprints.borrow().is_empty(), "y no se creo uno segundo");
    assert_eq!(spy.board.borrow().len(), 1);
}

/// Una rama que no lleva `.sprint.md` no tiene sprint que reconciliar, y eso
/// no es un error: es una rama que no es una ventana.
#[test]
fn una_rama_sin_sprint_no_inventa_uno() {
    let dir = arbol();
    let spy = Spy::default();
    let res = resolve(&repo(&dir), &spy);
    assert!(res.sprint.is_none());
    assert!(spy.created_sprints.borrow().is_empty());
}

/// El `key` se anota **despues** de `items`, y el frontmatter sigue cerrando:
/// pegar un campo contra el delimitador convierte todo el archivo en cuerpo.
/// Ver la task `5i`, que ya lo pago una vez con `parent`.
#[test]
fn anotar_el_key_no_rompe_el_frontmatter() {
    let dir = con_sprint(&["n"], None);
    let r = repo(&dir);
    let spy = Spy::default();
    let res = resolve(&r, &spy);
    let texto = show(&r, &res.new_head, "_sprints/3.sprint.md");
    let fin = texto.find("\n---\n").expect("el frontmatter cierra: {texto}");
    let fm = &texto[..fin];
    assert!(fm.contains("key: "), "y la clave esta adentro: {fm}");
    assert!(fm.contains("updated_at:"), "sin comerse lo que venia despues: {fm}");
    assert!(texto[fin..].contains("cuerpo del sprint"), "{texto}");
}

/// El id del board es argumento de la operacion y no un campo del cliente: un
/// proyecto puede tener varios boards, asi que guardar uno adentro seria
/// afirmar algo que no es cierto.
#[test]
fn el_id_del_board_llega_a_la_operacion() {
    let dir = con_sprint(&["n"], None);
    let spy = ConBoard::default();
    let rev = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo(&dir))
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    worklist_provider::assign::assign_window(
        &repo(&dir),
        "refs/heads/secure/sprint/3",
        worklist_provider::check_push::ALL_ZEROS,
        &rev,
        "https://x",
        &spy,
        "701",
        false,
    )
    .unwrap();
    assert_eq!(spy.boards.borrow().as_slice(), ["701", "701"], "crear y listar lo piden");
}

/// Un `Board` que solo anota con que id de board se lo llamo.
#[derive(Default)]
struct ConBoard {
    inner: Spy,
    boards: RefCell<Vec<String>>,
}

impl worklist_provider::board::Board for ConBoard {
    fn create_or_find(
        &self,
        title: &str,
        item_type: &str,
        description: &str,
        parent: Option<&str>,
    ) -> anyhow::Result<worklist_provider::board::Assignment> {
        self.inner.create_or_find(title, item_type, description, parent)
    }
    fn find(&self, title: &str) -> anyhow::Result<Option<String>> {
        self.inner.find(title)
    }
    fn link_relates(&self, a: &str, b: &str) -> anyhow::Result<bool> {
        self.inner.link_relates(a, b)
    }
    fn set_summary(&self, key: &str, title: &str) -> anyhow::Result<()> {
        self.inner.set_summary(key, title)
    }
    fn set_description(&self, key: &str, adf: &str) -> anyhow::Result<()> {
        self.inner.set_description(key, adf)
    }
    fn link_blocks(&self, a: &str, b: &str) -> anyhow::Result<bool> {
        self.inner.link_blocks(a, b)
    }
    fn set_parent(&self, key: &str, epic: &str) -> anyhow::Result<bool> {
        self.inner.set_parent(key, epic)
    }
    fn parent_of(&self, key: &str) -> anyhow::Result<Option<String>> {
        self.inner.parent_of(key)
    }
    fn add_to_sprint(&self, sprint: &str, keys: &[&str]) -> anyhow::Result<usize> {
        self.inner.add_to_sprint(sprint, keys)
    }
    fn create_or_find_sprint(&self, board: &str, name: &str) -> anyhow::Result<(String, bool)> {
        self.boards.borrow_mut().push(board.into());
        self.inner.create_or_find_sprint(board, name)
    }
    fn sprint_items(&self, board: &str, sprint: &str) -> anyhow::Result<Vec<String>> {
        self.boards.borrow_mut().push(board.into());
        self.inner.sprint_items(board, sprint)
    }
}

/// Y el `--dry-run` **no dice cuantos ya estaban**, porque no le pregunto a
/// nadie. Decir `0` seria afirmar sobre el board sin haberlo mirado, que es el
/// defecto que `67` corrigio del otro lado.
#[test]
fn el_dry_run_no_afirma_cuantos_ya_estaban() {
    let dir = common::arbol();
    let r = dir.path().join("repo");
    common::sprint(&r, "1", "El sprint", &["n"], None);
    common::run(&r, &["add", "-A"]);
    common::run(&r, &["commit", "-qm", "sprint"]);

    let spy = common::Spy::default();
    let head = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let res = worklist_provider::assign::assign_window(
        &r,
        "refs/heads/insecure/all",
        worklist_provider::check_push::ALL_ZEROS,
        &head,
        "https://x",
        &spy,
        "701",
        true,
    )
    .unwrap()
    .unwrap();

    assert_eq!(res.sprint.unwrap().already, None, "no se pregunto");
    assert!(spy.inside.borrow().is_empty(), "y de verdad no se pregunto");
}

/// Jira corta en 30, y diez de los veintidos sprints de este repo se pasaban.
#[test]
fn el_nombre_del_sprint_entra_en_el_limite_de_jira() {
    use worklist_provider::assign::{sprint_name, SPRINT_NAME_MAX};

    let corto = sprint_name("17", Some("Los sprints en el board"));
    assert_eq!(corto, "17 Los sprints en el board", "lo que entra no se toca");

    let largo = sprint_name("12", Some("El formato: `accepted` como lista y el vecindario con captures"));
    assert!(largo.chars().count() <= SPRINT_NAME_MAX, "{largo:?} mide {}", largo.chars().count());
    assert!(largo.starts_with("12 "), "el numero nunca se recorta: {largo}");
    assert!(largo.ends_with('…'), "y el corte se marca: {largo}");
}

/// Y sobre todo **es deterministico**: mientras el `.sprint.md` no tenga `key`,
/// este nombre es con lo que se busca antes de crear. Dos corridas que
/// produjeran nombres distintos duplicarian el sprint.
#[test]
fn el_nombre_recortado_es_el_mismo_todas_las_veces() {
    use worklist_provider::assign::sprint_name;
    let t = Some("La migración, hasta el corte de formato");
    assert_eq!(sprint_name("4", t), sprint_name("4", t));
}

/// Un sprint sin titulo es su numero, y el numero solo nunca se recorta.
#[test]
fn sin_titulo_el_nombre_es_el_numero() {
    assert_eq!(worklist_provider::assign::sprint_name("7", None), "7");
}
