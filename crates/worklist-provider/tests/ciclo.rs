//! El ciclo con el servidor: empujar, lo que el servidor escribe encima, y
//! volver a bajar.
//!
//! **Es el unico lugar donde el cliente y el servidor se cruzan**, y hasta acá
//! no tenía pruebas: las de recorte y replante corren de un solo lado. Los tres
//! defectos que aparecieron el 2026-09-07 estaban todos en el cruce, y ninguno
//! se veía en una vuelta sola — hacían falta **dos** pushes, porque el primero
//! es el que deja commits del servidor encima.
//!
//! El hook no se instala: es un `sh` de una línea que llama a `assign_window` y
//! a `propagate`, y acá se los llama directo. Lo que se prueba es el cruce, no
//! el shell.

mod common;

use common::{item, run, sprint, Spy};
use std::path::{Path, PathBuf};
use std::process::Command;

const BASE: &str = "https://ejemplo.atlassian.net";

fn rev(repo: &Path, refname: &str) -> String {
    let out = Command::new("git").arg("-C").arg(repo).args(["rev-parse", refname]).output().unwrap();
    assert!(out.status.success(), "git rev-parse {refname} en {}", repo.display());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn existe(repo: &Path, archivo: &str) -> bool {
    repo.join(archivo).exists()
}

/// El bare del servidor con su panorama, y un clon con la ventana del sprint 1
/// checkouteada — que es la forma que tiene la instalación de verdad.
fn instalacion() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("srv.git");
    let semilla = dir.path().join("semilla");

    std::fs::create_dir(&semilla).unwrap();
    run(&semilla, &["init", "-q", "-b", "insecure/all"]);
    run(&semilla, &["config", "user.email", "t@t"]);
    run(&semilla, &["config", "user.name", "t"]);
    item(&semilla, "@1.epic.md", None);
    item(&semilla, "@o.task.md", Some("@1"));
    item(&semilla, "@q.task.md", None);
    sprint(&semilla, "1", "el primero", &["@o"], None);
    run(&semilla, &["add", "-A"]);
    run(&semilla, &["commit", "-qm", "arbol"]);

    run(&semilla, &["clone", "--bare", "-q", ".", bare.to_str().unwrap()]);
    run(&bare, &["config", "user.email", "srv@srv"]);
    run(&bare, &["config", "user.name", "srv"]);

    // El servidor recorta; el cliente sólo trae.
    worklist_core::window::open(&bare, "1", "insecure/all", false, false).unwrap();

    let clon = dir.path().join("clon");
    run(dir.path(), &["clone", "-q", bare.to_str().unwrap(), clon.to_str().unwrap()]);
    run(&clon, &["config", "user.email", "yo@yo"]);
    run(&clon, &["config", "user.name", "yo"]);
    run(&clon, &["remote", "rename", "origin", "srv"]);
    let vista = clon.join("w");
    run(&clon, &["worktree", "add", "-q", vista.to_str().unwrap(), "secure/sprint/1"]);

    (dir, bare, vista)
}

/// Lo que hace el `post-receive`: resolver los pedidos y subir al panorama.
fn el_servidor_resuelve(bare: &Path) {
    let tip = rev(bare, "refs/heads/secure/sprint/1");
    let spy = Spy::default();
    worklist_provider::assign::assign_window(
        bare,
        "refs/heads/secure/sprint/1",
        worklist_provider::check_push::ALL_ZEROS,
        &tip,
        BASE,
        &spy,
        "701",
        false,
    )
    .unwrap();
    let tip = rev(bare, "refs/heads/secure/sprint/1");
    worklist_provider::propagate::propagate(bare, "refs/heads/secure/sprint/1", &tip, BASE, false)
        .unwrap();
}

/// Lo que hace `worklist pull`: recortar de nuevo, bajar, y replantar lo que no
/// se empujó.
fn el_cliente_baja(bare: &Path, vista: &Path) {
    let head = rev(vista, "HEAD");
    // **Antes de recortar**, que es lo que hace `pull`: recortar mueve la marca,
    // así que preguntarle después contesta sobre la rama nueva y no sobre lo que
    // yo tenía. El orden acá no es de estilo — es la pregunta.
    let todo_subido =
        worklist_core::window::propagado_entero(bare, "refs/heads/secure/sprint/1", &head);

    worklist_core::window::open(bare, "1", "insecure/all", false, false).unwrap();
    run(vista, &["fetch", "srv", "--quiet"]);
    let tip = rev(vista, "refs/remotes/srv/secure/sprint/1");

    if todo_subido {
        run(vista, &["reset", "--hard", "--quiet", &tip]);
        return;
    }
    let ancestro = Command::new("git")
        .arg("-C")
        .arg(vista)
        .args(["merge-base", "--is-ancestor", &tip, &head])
        .status()
        .unwrap()
        .success();
    if ancestro {
        return;
    }
    worklist_core::window::replantar(vista, &tip, &head).unwrap();
}

/// **El defecto que costó un ítem duplicado.**
///
/// El commit que crea un ítem ya subió —por eso el servidor pudo renombrarlo— y
/// el replante lo re-aplicaba sobre un corte que ya tenía la clave. Como el
/// archivo con el slug viejo no estaba, la creación aplicaba limpia y la vista
/// quedaba con el ítem **dos veces, con dos nombres**.
#[test]
fn el_item_que_el_servidor_renombro_no_vuelve_con_su_slug() {
    let (_dir, bare, vista) = instalacion();

    // El cliente crea un ítem adentro de su ventana y empuja.
    item(&vista, "@nuevo.task.md", Some("@o"));
    run(&vista, &["add", "-A"]);
    run(&vista, &["commit", "-qm", "un item nuevo"]);
    run(&vista, &["push", "-q", "srv", "HEAD:refs/heads/secure/sprint/1"]);

    el_servidor_resuelve(&bare);
    el_cliente_baja(&bare, &vista);

    let renombrado: Vec<String> = std::fs::read_dir(&vista)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
        .filter(|n| n.ends_with(".task.md") && n.starts_with("ACC-"))
        .collect();
    assert!(!renombrado.is_empty(), "el servidor no le puso clave a nada: {renombrado:?}");
    assert!(
        !existe(&vista, "@nuevo.task.md"),
        "volvió el slug viejo: la vista tiene el ítem dos veces, como @nuevo y como {renombrado:?}"
    );
}

/// Y el segundo push es el que descubre todo, porque el primero deja commits
/// del servidor encima. Una vuelta sola no alcanza para probar nada de esto.
#[test]
fn dos_vueltas_seguidas_dejan_la_vista_al_dia() {
    let (_dir, bare, vista) = instalacion();

    for i in 1..=2 {
        // **El archivo cambia de nombre entre una vuelta y la otra**, porque el
        // servidor le pone clave. Buscarlo cada vez es parte de lo que se
        // prueba: escribir sobre el nombre viejo dejaría un archivo suelto.
        let archivo = la_task(&vista);
        let cuerpo = std::fs::read_to_string(&archivo).unwrap();
        let (frontmatter, _) = worklist_core::body::split_frontmatter(&cuerpo);
        std::fs::write(&archivo, format!("{frontmatter}\nvuelta {i}\n")).unwrap();
        run(&vista, &["commit", "-aqm", &format!("edito la task, vuelta {i}")]);
        run(&vista, &["push", "-q", "srv", "HEAD:refs/heads/secure/sprint/1"]);
        el_servidor_resuelve(&bare);
        el_cliente_baja(&bare, &vista);

        let sucio = Command::new("git")
            .arg("-C")
            .arg(&vista)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&sucio.stdout).trim().is_empty(),
            "la vuelta {i} dejó la vista sucia"
        );
    }

    // Y lo último que se escribió es lo que quedó: dar dos vueltas no revive
    // una versión vieja.
    let texto = std::fs::read_to_string(la_task(&vista)).unwrap();
    assert!(texto.contains("vuelta 2"), "quedó una versión vieja: {texto}");

    // Y no quedó una segunda copia con otro nombre.
    let tasks: Vec<String> = std::fs::read_dir(&vista)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
        .filter(|n| n.ends_with(".task.md"))
        .collect();
    assert_eq!(tasks.len(), 1, "la vista quedó con la task duplicada: {tasks:?}");
}

/// La única task de la ventana, se llame como se llame hoy.
fn la_task(vista: &Path) -> PathBuf {
    let mut encontradas: Vec<PathBuf> = std::fs::read_dir(vista)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".task.md"))
        .collect();
    encontradas.sort();
    assert_eq!(encontradas.len(), 1, "se esperaba una sola task: {encontradas:?}");
    encontradas.pop().unwrap()
}

/// Bajar despues de un push no puede dejar trabajo pendiente inventado.
///
/// Es la pregunta que `worklist status` contesta, y la que estaba mal: la marca
/// del servidor queda **adelante** del HEAD del cliente, asi que compararlas
/// por igualdad decia "tengo algo sin empujar" cuando no habia nada.
#[test]
fn despues_de_bajar_no_queda_nada_sin_empujar() {
    let (_dir, bare, vista) = instalacion();

    item(&vista, "@otro.task.md", Some("@o"));
    run(&vista, &["add", "-A"]);
    run(&vista, &["commit", "-qm", "otro item"]);
    run(&vista, &["push", "-q", "srv", "HEAD:refs/heads/secure/sprint/1"]);
    el_servidor_resuelve(&bare);
    el_cliente_baja(&bare, &vista);

    let head = rev(&vista, "HEAD");
    assert!(
        worklist_core::window::propagado_entero(&bare, "refs/heads/secure/sprint/1", &head),
        "despues de empujar y bajar, el servidor tendria que tener todo lo mio"
    );
}
