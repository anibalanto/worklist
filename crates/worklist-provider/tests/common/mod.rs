//! El proveedor de mentira que comparten las pruebas de `assign_window`, y el
//! arbol chico sobre el que corren.
//!
//! Vive aca y no en un archivo de pruebas porque **lo usan dos**: la jerarquia
//! que viaja al proveedor y el sprint que se arma del otro lado. Una copia por
//! archivo se habria desincronizado con el puerto en el primer cambio.

// Cada archivo de pruebas compila su propia copia de este modulo, asi que lo
// que usa uno le sobra al otro. No es codigo muerto: es codigo que no todos
// los que lo compilan necesitan.
#![allow(dead_code)]

use std::cell::RefCell;
use std::path::Path;
use std::process::Command;
use worklist_provider::board::{Assignment, Board, Transicion};
use worklist_provider::states::Destino;

pub fn run(repo: &Path, args: &[&str]) {
    let st = Command::new("git").arg("-C").arg(repo).args(args).status().unwrap();
    assert!(st.success(), "git {args:?}");
}

pub fn item(repo: &Path, name: &str, parent: Option<&str>) {
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

/// Un `Board` que anota lo que se le pidio y nunca sale a la red. Reparte
/// claves en orden de creacion, que es lo que hace el proveedor real.
#[derive(Default)]
pub struct Spy {
    pub created: RefCell<Vec<(String, String, Option<String>)>>,
    pub relates: RefCell<Vec<(String, String)>>,
    pub blocks: RefCell<Vec<(String, String)>>,
    /// `(clave, titulo)` de los `set_summary`: lo que se actualizo en el proveedor.
    pub summaries: RefCell<Vec<(String, String)>>,
    /// `(clave, adf)` de los `set_description`.
    pub descriptions: RefCell<Vec<(String, String)>>,
    /// Titulos que el proveedor "ya tiene": `create_or_find` los encuentra.
    pub existing: Vec<(String, String)>,
    /// `clave -> epica` que el proveedor **tiene puesta**, que no es lo mismo
    /// que la que se pidio al crear.
    pub parents: RefCell<Vec<(String, String)>>,
    /// `(sprint, claves)` de los `add_to_sprint`.
    pub sprinted: RefCell<Vec<(String, Vec<String>)>>,
    /// El board del proveedor: `(nombre, id)` de cada sprint que ya tiene.
    pub board: RefCell<Vec<(String, String)>>,
    /// Los nombres con los que se llamo a `create_or_find_sprint` y no se
    /// encontraron: **cuantas veces se creo de verdad.**
    pub created_sprints: RefCell<Vec<String>>,
    /// Que issues tiene cada sprint del board, por id.
    pub inside: RefCell<Vec<(String, Vec<String>)>>,
    /// `(clave, status, resolucion)` de las transiciones pedidas.
    pub transitions: RefCell<Vec<(String, String, Option<String>)>>,
    /// Las claves cuya transicion el "workflow" rechaza, para poder ejercer
    /// el rechazo por regla — que es una respuesta y no una falla.
    pub rechaza: Vec<String>,
}

impl Board for Spy {
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
    fn find(&self, title: &str) -> anyhow::Result<Option<String>> {
        Ok(self.existing.iter().find(|(t, _)| t == title).map(|(_, k)| k.clone()))
    }
    fn set_description(&self, key: &str, adf: &str) -> anyhow::Result<()> {
        self.descriptions.borrow_mut().push((key.into(), adf.into()));
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
    fn set_summary(&self, key: &str, title: &str) -> anyhow::Result<()> {
        self.summaries.borrow_mut().push((key.into(), title.into()));
        Ok(())
    }
    fn parent_of(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(self.parents.borrow().iter().find(|(k, _)| k == key).map(|(_, e)| e.clone()))
    }
    fn set_parent(&self, key: &str, epic: &str) -> anyhow::Result<bool> {
        let mut ps = self.parents.borrow_mut();
        if ps.iter().any(|(k, e)| k == key && e == epic) {
            return Ok(false);
        }
        ps.retain(|(k, _)| k != key);
        ps.push((key.into(), epic.into()));
        Ok(true)
    }
    fn add_to_sprint(&self, sprint: &str, keys: &[&str]) -> anyhow::Result<usize> {
        self.sprinted
            .borrow_mut()
            .push((sprint.into(), keys.iter().map(|k| k.to_string()).collect()));
        // La membresia es un campo del issue: entrar a un sprint es salir del
        // anterior, y volver a entrar al mismo no duplica nada.
        let mut inside = self.inside.borrow_mut();
        for (_, ks) in inside.iter_mut() {
            ks.retain(|k| !keys.contains(&k.as_str()));
        }
        match inside.iter_mut().find(|(s, _)| s == sprint) {
            Some((_, ks)) => ks.extend(keys.iter().map(|k| k.to_string())),
            None => inside.push((sprint.into(), keys.iter().map(|k| k.to_string()).collect())),
        }
        Ok(keys.len())
    }
    fn transition(&self, key: &str, destino: &Destino) -> anyhow::Result<Transicion> {
        if self.rechaza.iter().any(|k| k == key) {
            return Ok(Transicion::Rechazada {
                motivo: format!("\"{}\" no es una transicion de este workflow", destino.status()),
                disponibles: vec!["Ready for Review".into()],
            });
        }
        self.transitions.borrow_mut().push((
            key.into(),
            destino.status().into(),
            destino.resolution().map(|r| r.to_string()),
        ));
        Ok(Transicion::Hecha)
    }
    fn create_or_find_sprint(&self, _board: &str, name: &str) -> anyhow::Result<(String, bool)> {
        if let Some((_, id)) = self.board.borrow().iter().find(|(n, _)| n == name) {
            return Ok((id.clone(), false));
        }
        let id = format!("65{:02}", self.board.borrow().len());
        self.board.borrow_mut().push((name.into(), id.clone()));
        self.created_sprints.borrow_mut().push(name.into());
        Ok((id, true))
    }
    fn sprint_items(&self, _board: &str, sprint: &str) -> anyhow::Result<Vec<String>> {
        Ok(self
            .inside
            .borrow()
            .iter()
            .find(|(s, _)| s == sprint)
            .map(|(_, ks)| ks.clone())
            .unwrap_or_default())
    }
}

/// `@1.epic` -> `@n.user-story` -> `@o.task`, y una task suelta bajo la epica.
///
/// Los cuatro son **pedidos**, asi que llevan la marca: lo que no la lleva es
/// del proveedor. Ver `concepts/item.md` seccion "La marca `@`".
pub fn arbol() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = &dir.path().join("repo");
    std::fs::create_dir(r).unwrap();
    run(r, &["init", "-q", "-b", "insecure/all"]);
    run(r, &["config", "user.email", "t@t"]);
    run(r, &["config", "user.name", "t"]);
    item(r, "@1.epic.md", None);
    item(r, "@n.user-story.md", Some("@1"));
    item(r, "@o.task.md", Some("@n"));
    item(r, "@q.task.md", Some("@1"));
    run(r, &["add", "-A"]);
    run(r, &["commit", "-qm", "arbol"]);
    dir
}

pub fn resolve(dir: &Path, spy: &Spy) -> worklist_provider::assign::WindowResult {
    let rev = String::from_utf8(
        Command::new("git").arg("-C").arg(dir).args(["rev-parse", "HEAD"]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    worklist_provider::assign::assign_window(
        dir,
        "refs/heads/insecure/all",
        worklist_provider::check_push::ALL_ZEROS,
        &rev,
        "https://x",
        spy,
        "701",
        false,
    )
        .unwrap()
        .unwrap()
}
/// Escribe el `.sprint.md` de una ventana. `key` es el id del proveedor, que
/// **ausente significa que el sprint no existe del otro lado**.
/// Escribe el sprint **en las dos formas**, que es donde esta la migracion.
///
/// La composicion —`.metadata/product.yaml`— es de donde el recorte lee desde
/// `ACC-305`. El `.sprint.md` sigue porque las pasadas que sincronizan sprints
/// todavia lo leen, y se va con ellas. Escribir las dos es lo que deja el arbol
/// verde **durante** la mudanza en vez de al final.
pub fn sprint(repo: &Path, id: &str, title: &str, items: &[&str], key: Option<&str>) {
    componer(repo, id, title, items, key);
    std::fs::create_dir_all(repo.join("_sprints")).unwrap();
    let k = match key {
        Some(k) => format!("key: {k}\n"),
        None => String::new(),
    };
    std::fs::write(
        repo.join(format!("_sprints/{id}.sprint.md")),
        format!(
            "---\ntitle: {title}\nstatus: in-progress\nitems: [{}]\n{k}created_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\ncuerpo del sprint\n",
            items.join(", ")
        ),
    )
    .unwrap();
}

/// El contenido de un archivo en un commit, sin working tree de por medio.
pub fn show(repo: &Path, rev: &str, file: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &format!("{rev}:{file}")])
        .output()
        .unwrap();
    assert!(out.status.success(), "git show {rev}:{file}");
    String::from_utf8(out.stdout).unwrap()
}

/// Los archivos que un rev tiene, uno por linea y con un `\n` adelante para
/// poder preguntar por un nombre completo sin que un sufijo lo confunda.
pub fn show_tree(repo: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-tree", "-r", "--name-only", rev])
        .output()
        .unwrap();
    assert!(out.status.success(), "git ls-tree {rev}");
    format!("\n{}", String::from_utf8(out.stdout).unwrap())
}

/// La entrada de este sprint en la composicion, agregada a lo que ya haya.
pub fn componer(repo: &Path, id: &str, title: &str, items: &[&str], key: Option<&str>) {
    let path = repo.join(worklist_core::product::ARCHIVO);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut producto = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| worklist_core::product::de_yaml(&t).ok())
        .unwrap_or_default();
    producto.sprints.retain(|s| s.id != id);
    producto.sprints.push(worklist_core::product::Sprint {
        id: id.to_string(),
        name: title.chars().take(19).collect(),
        status: "in-progress".into(),
        key: key.map(|k| k.to_string()),
        items: items.iter().map(|s| s.to_string()).collect(),
    });
    producto.sprints.sort_by(|a, b| a.id.cmp(&b.id));
    std::fs::write(&path, producto.to_yaml().unwrap()).unwrap();
}
