//! La jerarquia que viaja al proveedor, que no es la del worklist.
//!
//! Jira no admite `parent` entre tipos del mismo nivel, asi que de los tres
//! escalones del worklist entra uno como jerarquia y el otro como link. Ver
//! `concepts/sync.md` seccion "La jerarquia entra hasta donde el proveedor la
//! tiene".

use std::cell::RefCell;
use std::path::Path;
use std::process::Command;
use worklist::creator::{Assignment, Creator};

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

/// Un `Creator` que anota lo que se le pidio y nunca sale a la red. Reparte
/// claves en orden de creacion, que es lo que hace el proveedor real.
#[derive(Default)]
struct Spy {
    created: RefCell<Vec<(String, String, Option<String>)>>,
    relates: RefCell<Vec<(String, String)>>,
    blocks: RefCell<Vec<(String, String)>>,
    /// Titulos que el proveedor "ya tiene": `create_or_find` los encuentra.
    existing: Vec<(String, String)>,
}

impl Creator for Spy {
    fn create_or_find(
        &self,
        title: &str,
        item_type: &str,
        _description: &str,
        parent: Option<&str>,
    ) -> anyhow::Result<Assignment> {
        if let Some((_, k)) = self.existing.iter().find(|(t, _)| t == title) {
            return Ok(Assignment::Found(k.clone()));
        }
        let key = format!("ACC-{}", self.created.borrow().len() + 1);
        self.created.borrow_mut().push((
            title.to_string(),
            item_type.to_string(),
            parent.map(|s| s.to_string()),
        ));
        Ok(Assignment::Created(key))
    }
    fn set_description(&self, _key: &str, _adf: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn link_blocks(&self, blocker: &str, blocked: &str) -> anyhow::Result<bool> {
        self.blocks.borrow_mut().push((blocker.into(), blocked.into()));
        Ok(true)
    }
    fn link_relates(&self, a: &str, b: &str) -> anyhow::Result<bool> {
        self.relates.borrow_mut().push((a.into(), b.into()));
        Ok(true)
    }
}

/// `1.epic` -> `n.user-story` -> `o.task`, y una task suelta bajo la epica.
fn arbol() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = &dir.path().join("repo");
    std::fs::create_dir(r).unwrap();
    run(r, &["init", "-q", "-b", "insecure/all"]);
    run(r, &["config", "user.email", "t@t"]);
    run(r, &["config", "user.name", "t"]);
    item(r, "1.epic.md", None);
    item(r, "n.user-story.md", Some("1"));
    item(r, "o.task.md", Some("n"));
    item(r, "q.task.md", Some("1"));
    run(r, &["add", "-A"]);
    run(r, &["commit", "-qm", "arbol"]);
    dir
}

fn resolve(dir: &Path, spy: &Spy) -> worklist::assign::WindowResult {
    let rev = String::from_utf8(
        Command::new("git").arg("-C").arg(dir).args(["rev-parse", "HEAD"]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    worklist::assign::assign_window(dir, "refs/heads/insecure/all", &rev, "https://x", spy, false)
        .unwrap()
        .unwrap()
}

/// El `--parent` de una task **no** es su user story: es la epica, por lejos
/// que quede. Jira rechaza `Tarea` bajo `Historia`.
#[test]
fn the_parent_sent_is_the_epic_not_the_direct_one() {
    let dir = arbol();
    let spy = Spy::default();
    resolve(&dir.path().join("repo"), &spy);
    let created = spy.created.borrow();
    let epic_key = "ACC-1";
    for (title, _, parent) in created.iter() {
        match title.as_str() {
            "1.epic.md" => assert_eq!(*parent, None, "la epica no cuelga de nada"),
            _ => assert_eq!(
                parent.as_deref(),
                Some(epic_key),
                "{title} tendria que colgar de la epica"
            ),
        }
    }
}

/// El escalon del medio viaja como `Relates`, y **solo** cuando el padre
/// directo no es la epica: una task suelta bajo la epica ya quedo anidada.
#[test]
fn the_middle_step_travels_as_a_relates_link() {
    let dir = arbol();
    let spy = Spy::default();
    let res = resolve(&dir.path().join("repo"), &spy);
    let key_of = |slug: &str| {
        res.assigned.iter().find(|a| a.slug == slug).map(|a| a.key.clone()).unwrap()
    };
    let rel = spy.relates.borrow();
    assert_eq!(
        *rel,
        vec![(key_of("n"), key_of("o"))],
        "solo la task que cuelga de la user story"
    );
}

/// Un item que el proveedor ya tenia no recibe el padre —`acli` acepta
/// `--parent` al crear y no al editar—, y eso se reporta en vez de callarse.
#[test]
fn a_found_item_reports_that_its_parent_was_not_applied() {
    let dir = arbol();
    let spy = Spy {
        existing: vec![("o.task.md".into(), "ACC-99".into())],
        ..Default::default()
    };
    let res = resolve(&dir.path().join("repo"), &spy);
    let o = res.assigned.iter().find(|a| a.slug == "o").unwrap();
    assert_eq!(o.key, "ACC-99");
    assert!(o.parent.is_some(), "se le pidio un padre");
    assert!(o.parent_missed, "y hay que decir que no se aplico");

    let epic = res.assigned.iter().find(|a| a.slug == "1").unwrap();
    assert!(!epic.parent_missed, "la epica no pedia padre");
}

/// Un ciclo en los `parent` es un error del worklist, y se reporta **antes**
/// de tocar el proveedor: lo caza el orden topologico, que es el primer paso.
/// Nada se crea a medias, y el mensaje nombra el item.
#[test]
fn a_cycle_in_the_parents_is_reported_before_touching_the_provider() {
    let dir = tempfile::tempdir().unwrap();
    let r = &dir.path().join("repo");
    std::fs::create_dir(r).unwrap();
    run(r, &["init", "-q", "-b", "insecure/all"]);
    run(r, &["config", "user.email", "t@t"]);
    run(r, &["config", "user.name", "t"]);
    item(r, "a.task.md", Some("b"));
    item(r, "b.task.md", Some("a"));
    run(r, &["add", "-A"]);
    run(r, &["commit", "-qm", "ciclo"]);
    let spy = Spy::default();
    let rev = String::from_utf8(
        Command::new("git").arg("-C").arg(r).args(["rev-parse", "HEAD"]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let err = worklist::assign::assign_window(
        r, "refs/heads/insecure/all", &rev, "https://x", &spy, false,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("ciclo"), "tiene que decir que hay un ciclo: {err}");
    assert!(err.contains('a') || err.contains('b'), "y cual: {err}");
    assert!(
        spy.created.borrow().is_empty(),
        "no se creo nada en el proveedor: {:?}",
        spy.created.borrow()
    );
}

/// El defecto de `5k`: una dependencia que apunta fuera de la ventana no tiene
/// clave que mandar. Se informa y **no aborta**: exigir que toda dependencia
/// caiga adentro seria pedirle al backlog que se ordene por el recorte.
#[test]
fn a_dependency_outside_the_window_is_reported_and_does_not_abort() {
    let dir = tempfile::tempdir().unwrap();
    let r = &dir.path().join("repo");
    std::fs::create_dir(r).unwrap();
    run(r, &["init", "-q", "-b", "insecure/all"]);
    run(r, &["config", "user.email", "t@t"]);
    run(r, &["config", "user.name", "t"]);
    item(r, "1.epic.md", None);
    // `c` depende de `9`, que es de otra ventana, y de `d`, que esta en esta.
    std::fs::write(
        r.join("c.user-story.md"),
        "---\ntitle: c\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nparent: 1\nrelation.depends: [9, d]\n---\n\ncuerpo\n",
    )
    .unwrap();
    item(r, "d.task.md", Some("1"));
    run(r, &["add", "-A"]);
    run(r, &["commit", "-qm", "arbol"]);

    let spy = Spy::default();
    let res = resolve(r, &spy);

    let key_of = |slug: &str| {
        res.assigned.iter().find(|a| a.slug == slug).map(|a| a.key.clone()).unwrap()
    };
    assert_eq!(
        res.untranslated,
        vec![("9".to_string(), key_of("c"))],
        "la que apunta afuera se informa"
    );
    assert_eq!(
        *spy.blocks.borrow(),
        vec![(key_of("d"), key_of("c"))],
        "y la de adentro se crea igual, ya traducida"
    );
    assert_eq!(res.assigned.len(), 3, "la ventana se resolvio entera");
}
