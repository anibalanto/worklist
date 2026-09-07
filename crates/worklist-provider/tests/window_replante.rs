//! El cruce entre recortar y propagar: vive aca y no en `worklist-core`
//! porque usa los dos crates — recortar es del cliente, propagar es del
//! servidor. Ver `concepts/distribution.md`.

use std::path::Path;
use std::process::Command;

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

fn arbol_aislado() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path().join("repo");
    std::fs::create_dir(&r).unwrap();
    run(&r, &["init", "-q"]);
    run(&r, &["config", "user.email", "t@t"]);
    run(&r, &["config", "user.name", "t"]);
    item(&r, "ACC-1.epic.md", None);
    item(&r, "ACC-2.user-story.md", Some("ACC-1"));
    item(&r, "ACC-3.task.md", Some("ACC-2"));
    // Estos quedan afuera de la ventana: sin ellos el recorte no borra nada y
    // no hay qué commitear.
    item(&r, "ACC-8.user-story.md", Some("ACC-1"));
    item(&r, "ACC-9.task.md", Some("ACC-8"));
    item(&r, "ACC-7.task.md", None);
    std::fs::create_dir(r.join("_sprints")).unwrap();
    std::fs::write(
        r.join("_sprints/10.sprint.md"),
        "---\ntitle: El sprint\nstatus: in-progress\nitems: [ACC-2]\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n\nplan\n",
    )
    .unwrap();
    run(&r, &["add", "-A"]);
    run(&r, &["commit", "-q", "-m", "arbol"]);
    run(&r, &["branch", "-q", "insecure/all"]);
    (dir, r)
}

fn head_de(r: &Path, refname: &str) -> String {
    String::from_utf8(
        Command::new("git").arg("-C").arg(r).args(["rev-parse", refname]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string()
}

/// Y el descarte deja de ser una promesa: si el trabajo ya subio al panorama,
/// el replante queda vacio **solo**, por patch-id. Nadie tiene que acordarse
/// de propagar antes de regenerar.
#[test]
fn el_replante_queda_vacio_si_el_trabajo_ya_subio() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();

    let wt = r.join("w");
    run(&r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/10"]);
    item(&wt, "ACC-3.task.md", Some("ACC-2"));
    std::fs::write(wt.join("ACC-3.task.md"), "---\ntitle: x\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nparent: ACC-2\n---\n\neditado\n").unwrap();
    run(&wt, &["commit", "-aqm", "edito ACC-3"]);
    run(&r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    let tip = head_de(&r, "refs/heads/secure/sprint/10");
    worklist_provider::propagate::propagate(&r, "refs/heads/secure/sprint/10", &tip, "https://ejemplo.atlassian.net", false)
        .unwrap()
        .unwrap();

    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();

    let log = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["log", "--format=%s", "refs/heads/secure/sprint/10"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    // El commit sigue nombrado en el log, y **por el panorama**: la
    // propagacion lo cherry-pickeo alla, y el corte nuevo sale de ahi. Lo que
    // no queda es una copia **encima** del corte.
    assert_eq!(
        log.lines().next(),
        Some("window: sprint/10 recortado desde insecure/all"),
        "el replante quedo vacio solo: {log}"
    );
    let contenido = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["show", "refs/heads/secure/sprint/10:ACC-3.task.md"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(contenido.contains("editado"), "y sin embargo el trabajo esta: {contenido}");
}

/// Y el otro caso en que el replante no aporta nada, que **no** lo resuelve el
/// patch-id: el panorama guarda la vuelta del round-trip y el commit de la
/// ventana guarda lo que se tipeo, asi que chocan en bytes y dicen lo mismo.
///
/// Antes de esto el error culpaba al `items`, que no tenia nada que ver — un
/// diagnostico falso es peor que ninguno. Ver `commands/pull.md` seccion "El
/// paso 3 deja caer lo que ya fue superado".
#[test]
fn un_commit_ya_superado_por_la_normalizacion_se_deja_caer() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();

    let frontmatter = "---\ntitle: x\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nparent: ACC-2\n---\n";
    let tipeado = format!("{frontmatter}\nEsta task pedia *\"un nombre bajo 20\"* elegido a mano.\n");
    let normalizado = worklist_core::body::canonical(&tipeado).unwrap();
    // Si el conversor no cambiara nada, el test pasaria sin ejercitar nada.
    assert_ne!(tipeado, normalizado, "el caso que esto prueba necesita que difieran en bytes");

    // La ventana escribe lo que se tipeo.
    let wt = r.join("w");
    run(&r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/10"]);
    std::fs::write(wt.join("ACC-3.task.md"), &tipeado).unwrap();
    run(&wt, &["commit", "-aqm", "edito ACC-3"]);
    run(&r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    // Y el panorama termina con la vuelta, que es lo que `normalize:` rehace
    // arriba. Se escribe a mano para aislar el caso del resto de la propagacion.
    let pan = r.join("p");
    run(&r, &["worktree", "add", "-q", pan.to_str().unwrap(), "insecure/all"]);
    std::fs::write(pan.join("ACC-3.task.md"), &normalizado).unwrap();
    run(&pan, &["commit", "-aqm", "normalize: ACC-3"]);
    run(&r, &["worktree", "remove", "--force", pan.to_str().unwrap()]);

    // Sin descontar la normalizacion, esto choca y pide `--force`.
    worklist_core::window::open(&r, "10", "insecure/all", false, false)
        .expect("un commit superado no es un conflicto");

    let contenido = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["show", "refs/heads/secure/sprint/10:ACC-3.task.md"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    // Gana la vigente, que es la del panorama: no hay dos versiones que
    // reconciliar, hay una superada y una vigente.
    assert_eq!(contenido, normalizado, "quedo la vuelta del round-trip");
}

/// Y la respuesta que hace innecesarias a las tres: **si la ventana esta
/// propagada entera, no hay nada que replantar**, y el servidor lo tiene
/// anotado en una ref.
///
/// El caso que lo obliga es una ventana larga: arriba el cuerpo quedo guardado
/// en su forma canonica, asi que el patch-id no reconoce ni uno solo de sus
/// commits y el replante los va chocando de a uno. Preguntarle a la ref
/// contesta por construccion lo que git tendria que redescubrir.
#[test]
fn una_ventana_propagada_entera_no_replanta_nada() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();

    let wt = r.join("w");
    run(&r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/10"]);
    std::fs::write(wt.join("ACC-3.task.md"), "---\ntitle: x\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nparent: ACC-2\n---\n\neditado\n").unwrap();
    run(&wt, &["commit", "-aqm", "edito ACC-3"]);
    run(&r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    let tip = head_de(&r, "refs/heads/secure/sprint/10");
    worklist_provider::propagate::propagate(&r, "refs/heads/secure/sprint/10", &tip, "https://ejemplo.atlassian.net", false)
        .unwrap()
        .unwrap();

    // Y encima el panorama sigue: el archivo queda distinto en bytes de lo que
    // la ventana commiteo, que es lo que rompe el patch-id.
    let pan = r.join("p");
    run(&r, &["worktree", "add", "-q", pan.to_str().unwrap(), "insecure/all"]);
    std::fs::write(pan.join("ACC-3.task.md"), "---\ntitle: x\nstatus: done\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-08T00:00:00Z\nparent: ACC-2\n---\n\neditado, y algo mas\n").unwrap();
    run(&pan, &["commit", "-aqm", "sigue ACC-3"]);
    run(&r, &["worktree", "remove", "--force", pan.to_str().unwrap()]);

    worklist_core::window::open(&r, "10", "insecure/all", false, false)
        .expect("propagada entera, no hay nada que replantar");

    let contenido = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["show", "refs/heads/secure/sprint/10:ACC-3.task.md"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(contenido.contains("editado, y algo mas"), "quedo lo ultimo del panorama");
}

/// Recortar es idempotente: si no hay nada nuevo que recortar, la rama no se
/// mueve.
///
/// No es una optimizacion. Un commit lleva la hora adentro del hash, asi que
/// recortar igual reescribe la rama **para decir lo que ya decia**, y todo el
/// que la tenga clonada queda sin poder fast-forwardear por nada. Desde que
/// `pull` recorta en cada invocacion, eso pasaria todo el tiempo.
#[test]
fn recortar_sin_nada_nuevo_no_mueve_la_rama() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();
    let primero = head_de(&r, "refs/heads/secure/sprint/10");

    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();
    assert_eq!(head_de(&r, "refs/heads/secure/sprint/10"), primero, "recorto de nuevo por nada");
}

/// Y sigue siendo idempotente cuando el panorama se movio **para otra
/// ventana**: cualquier push a cualquiera de las otras lo adelanta, y esta no
/// tiene por que enterarse.
#[test]
fn el_panorama_que_avanza_por_otra_ventana_no_mueve_esta() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();
    let primero = head_de(&r, "refs/heads/secure/sprint/10");

    // Se toca un item que esta ventana no lleva: `ACC-7` quedo afuera.
    let pan = r.join("p");
    run(&r, &["worktree", "add", "-q", pan.to_str().unwrap(), "insecure/all"]);
    std::fs::write(pan.join("ACC-7.task.md"), "---\ntitle: otro\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-08T00:00:00Z\n---\n\nde otra ventana\n").unwrap();
    run(&pan, &["commit", "-aqm", "edito ACC-7, que no es de la 10"]);
    run(&r, &["worktree", "remove", "--force", pan.to_str().unwrap()]);

    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();
    assert_eq!(
        head_de(&r, "refs/heads/secure/sprint/10"),
        primero,
        "el panorama avanzo, pero no para esta ventana"
    );
}

/// Y cuando si cambia para esta ventana, recorta.
#[test]
fn un_cambio_en_un_item_de_la_ventana_si_recorta() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();
    let primero = head_de(&r, "refs/heads/secure/sprint/10");

    let pan = r.join("p");
    run(&r, &["worktree", "add", "-q", pan.to_str().unwrap(), "insecure/all"]);
    std::fs::write(pan.join("ACC-3.task.md"), "---\ntitle: x\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-08T00:00:00Z\nparent: ACC-2\n---\n\nesto si es de la 10\n").unwrap();
    run(&pan, &["commit", "-aqm", "edito ACC-3, que si es de la 10"]);
    run(&r, &["worktree", "remove", "--force", pan.to_str().unwrap()]);

    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();
    let segundo = head_de(&r, "refs/heads/secure/sprint/10");
    assert_ne!(segundo, primero, "cambio algo suyo y no lo trajo");
    let contenido = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["show", "refs/heads/secure/sprint/10:ACC-3.task.md"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(contenido.contains("esto si es de la 10"));
}

/// Y el replante tiene la misma propiedad por su lado: un commit que ya esta
/// encima de la punta no se cherry-pickea sobre su propio padre.
///
/// Estar **adelantado** no es estar divergido. Cherry-pickearlo lo dejaria
/// igual con otro sha, que es la misma reescritura por nada que el recorte
/// dejo de hacer.
#[test]
fn el_trabajo_que_ya_esta_encima_del_corte_no_se_replanta() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();

    let wt = r.join("w");
    run(&r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/10"]);
    std::fs::write(wt.join("ACC-3.task.md"), "---\ntitle: x\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nparent: ACC-2\n---\n\nsin empujar\n").unwrap();
    run(&wt, &["commit", "-aqm", "edito ACC-3"]);
    run(&r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    let con_trabajo = head_de(&r, "refs/heads/secure/sprint/10");
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();
    assert_eq!(
        head_de(&r, "refs/heads/secure/sprint/10"),
        con_trabajo,
        "el trabajo sin propagar se replanto sobre su propio padre"
    );
}

/// Y la contabilidad se mueve con la rama.
///
/// Regenerar reescribe la historia, asi que la marca se quedaria apuntando a un
/// commit que ya no es ancestro de nada. `pending` la usa como piso, y con el
/// piso afuera de la rama ese rango es la ventana entera: el proximo push
/// re-propagaria todo lo que ya subio.
#[test]
fn regenerar_deja_la_marca_adentro_de_la_rama() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();

    let wt = r.join("w");
    run(&r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/10"]);
    std::fs::write(wt.join("ACC-3.task.md"), "---\ntitle: x\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nparent: ACC-2\n---\n\neditado\n").unwrap();
    run(&wt, &["commit", "-aqm", "edito ACC-3"]);
    run(&r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    let tip = head_de(&r, "refs/heads/secure/sprint/10");
    worklist_provider::propagate::propagate(&r, "refs/heads/secure/sprint/10", &tip, "https://ejemplo.atlassian.net", false)
        .unwrap()
        .unwrap();

    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();

    let nuevo = head_de(&r, "refs/heads/secure/sprint/10");
    let marca = head_de(&r, &worklist_provider::propagate::propagated_ref("refs/heads/secure/sprint/10"));
    let es_ancestro = Command::new("git")
        .arg("-C")
        .arg(&r)
        .args(["merge-base", "--is-ancestor", &marca, &nuevo])
        .status()
        .unwrap()
        .success();
    assert!(es_ancestro, "la marca quedo afuera de la rama regenerada: {marca} / {nuevo}");

    // Y no queda nada por subir: lo que la ventana hizo ya esta arriba.
    let (_, pendientes) =
        worklist_provider::propagate::pending(&r, "refs/heads/secure/sprint/10", &nuevo).unwrap();
    assert!(pendientes.is_empty(), "re-propagaria {} commit(s) ya subidos", pendientes.len());
}

/// El tercer motivo por el que un replante no aporta nada: el `.sprint.md`.
///
/// `status` e `items` se editan **arriba** y bajan regenerando, asi que el
/// commit de la ventana arrastra como contexto un `items` que ya quedo viejo y
/// choca sobre algo que no estaba tratando de cambiar. Gana el corte.
#[test]
fn un_conflicto_solo_en_el_sprint_md_lo_gana_el_corte() {
    let (_dir, r) = arbol_aislado();
    worklist_core::window::open(&r, "10", "insecure/all", false, false).unwrap();

    // La ventana arranca el sprint: `open -> in-progress`, y nada mas.
    let wt = r.join("w");
    run(&r, &["worktree", "add", "-q", wt.to_str().unwrap(), "secure/sprint/10"]);
    let en_curso = std::fs::read_to_string(wt.join("_sprints/10.sprint.md"))
        .unwrap()
        .replace("status: in-progress", "status: open")
        .replace("updated_at: 2026-09-04", "updated_at: 2026-09-05");
    std::fs::write(wt.join("_sprints/10.sprint.md"), &en_curso).unwrap();
    run(&wt, &["commit", "-aqm", "el sprint 10 arranca"]);
    run(&r, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    // Y arriba se replanifica: el `items` gana un item. Es el contexto que
    // deriva, y es trabajo legitimo.
    let pan = r.join("p");
    run(&r, &["worktree", "add", "-q", pan.to_str().unwrap(), "insecure/all"]);
    let replanificado = std::fs::read_to_string(pan.join("_sprints/10.sprint.md"))
        .unwrap()
        .replace("items: [ACC-2]", "items: [ACC-2, ACC-7]");
    std::fs::write(pan.join("_sprints/10.sprint.md"), &replanificado).unwrap();
    run(&pan, &["commit", "-aqm", "el sprint 10 gana un item"]);
    run(&r, &["worktree", "remove", "--force", pan.to_str().unwrap()]);

    worklist_core::window::open(&r, "10", "insecure/all", false, false)
        .expect("un choque solo en el `.sprint.md` no es un conflicto que frene el recorte");

    let contenido = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["show", "refs/heads/secure/sprint/10:_sprints/10.sprint.md"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(contenido.contains("items: [ACC-2, ACC-7]"), "gano la planificacion de arriba");
}
