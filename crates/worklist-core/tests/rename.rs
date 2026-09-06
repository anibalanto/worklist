//! Los cuatro casos de aceptacion de `52`, contra un repo real de git en un
//! directorio temporal. Ningun test toca el repo del proyecto ni ningun
//! proveedor.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

fn git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    run(dir.path(), &["init", "-q"]);
    run(dir.path(), &["config", "user.email", "test@test"]);
    run(dir.path(), &["config", "user.name", "test"]);
    dir
}

fn run(repo: &Path, args: &[&str]) {
    let status = Command::new("git").arg("-C").arg(repo).args(args).status().unwrap();
    assert!(status.success(), "git {:?} failed", args);
}

fn write(repo: &Path, name: &str, content: &str) {
    std::fs::write(repo.join(name), content).unwrap();
}

fn head(repo: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn rename_sin_referencias_entrantes() {
    let dir = git_repo();
    let repo = dir.path();
    write(
        repo,
        "slug-solo.task.md",
        "---\ntitle: Solo\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\nNada la referencia.\n",
    );
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);

    let touched = worklist_core::rename_one(repo, "slug-solo", "ACC-1").unwrap();
    assert!(touched.is_empty());
    assert!(repo.join("ACC-1.task.md").exists());
    assert!(!repo.join("slug-solo.task.md").exists());
}

#[test]
fn rename_reescribe_referencias_delimitadas_y_no_subcadenas() {
    let dir = git_repo();
    let repo = dir.path();
    write(
        repo,
        "slug-a.task.md",
        "---\ntitle: A\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n# A\n",
    );
    write(
        repo,
        "slug-b.task.md",
        "---\ntitle: B\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nrelation.depends: [slug-a]\n---\nDepende de [`slug-a`](slug-a.task.md).\n",
    );
    write(
        repo,
        "unrelated.task.md",
        "---\ntitle: No tocar\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\nMenciona slug-a10 y slug-a sueltos, sin backticks.\n",
    );
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);

    let touched = worklist_core::rename_one(repo, "slug-a", "ACC-101").unwrap();
    assert_eq!(touched, vec!["slug-b.task.md".to_string()]);

    let b = std::fs::read_to_string(repo.join("slug-b.task.md")).unwrap();
    assert!(b.contains("relation.depends: [ACC-101]"));
    assert!(b.contains("[`ACC-101`](ACC-101.task.md)"));

    let unrelated = std::fs::read_to_string(repo.join("unrelated.task.md")).unwrap();
    assert!(unrelated.contains("slug-a10"), "no debia tocar la subcadena");
    assert!(unrelated.contains("slug-a sueltos"), "no debia tocar la mencion suelta sin backticks");
}

#[test]
fn resolve_batch_respeta_el_orden_topologico() {
    let dir = git_repo();
    let repo = dir.path();
    write(
        repo,
        "slug-c.task.md",
        "---\ntitle: C\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nrelation.depends: [slug-d]\n---\nDepende de [`slug-d`](slug-d.task.md).\n",
    );
    write(
        repo,
        "slug-d.task.md",
        "---\ntitle: D\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\nSin dependencias.\n",
    );
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);

    let mut map = HashMap::new();
    map.insert("slug-c".to_string(), "ACC-201".to_string());
    map.insert("slug-d".to_string(), "ACC-200".to_string());
    worklist_core::resolve_batch(repo, &map).unwrap();

    let c = std::fs::read_to_string(repo.join("ACC-201.task.md")).unwrap();
    assert!(c.contains("relation.depends: [ACC-200]"));
    assert!(c.contains("[`ACC-200`](ACC-200.task.md)"));
}

#[test]
fn un_ciclo_se_rechaza_sin_escribir_nada() {
    let dir = git_repo();
    let repo = dir.path();
    write(
        repo,
        "slug-e.task.md",
        "---\ntitle: E\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nrelation.depends: [slug-f]\n---\nCiclo.\n",
    );
    write(
        repo,
        "slug-f.task.md",
        "---\ntitle: F\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nrelation.depends: [slug-e]\n---\nCiclo.\n",
    );
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);
    let before = head(repo);

    let mut map = HashMap::new();
    map.insert("slug-e".to_string(), "ACC-301".to_string());
    map.insert("slug-f".to_string(), "ACC-302".to_string());
    let result = worklist_core::resolve_batch(repo, &map);

    assert!(result.is_err());
    assert_eq!(before, head(repo), "el repo no debia cambiar");
    assert!(repo.join("slug-e.task.md").exists());
    assert!(repo.join("slug-f.task.md").exists());
}

/// El defecto de `5i`: el regex del `parent` capturaba el delimitador y no lo
/// restituia. Con `parent:` como **ultima** linea del frontmatter, el `\n` que
/// se perdia era el del cierre.
#[test]
fn renaming_the_parent_keeps_the_newline() {
    let text = "---\ntitle: X\nparent: 1\n---\n\n# X\n\ncuerpo\n";
    let (out, changed) = worklist_core::rewrite_references(text, "1", "epic", "ACC-14");
    assert!(changed);
    assert_eq!(out, "---\ntitle: X\nparent: ACC-14\n---\n\n# X\n\ncuerpo\n");
}

/// Y la propiedad que de verdad se rompio, que no se ve mirando el `parent`:
/// despues de renombrar, el frontmatter tiene que seguir separandose. Si no,
/// el archivo entero viaja al proveedor como cuerpo.
#[test]
fn the_frontmatter_still_splits_after_renaming_the_parent() {
    let text = "---\ntitle: X\nstatus: done\nparent: 1\n---\n\n# X\n\ncuerpo\n";
    let (out, _) = worklist_core::rewrite_references(text, "1", "epic", "ACC-14");
    let (fm, body) = worklist_core::body::split_frontmatter(&out);
    assert!(fm.contains("parent: ACC-14"), "el frontmatter es el frontmatter: {fm:?}");
    assert!(!fm.is_empty(), "no se separo nada: todo el archivo seria cuerpo");
    assert_eq!(body, "\n# X\n\ncuerpo\n", "y el cuerpo es solo el cuerpo");
}

/// Con `parent:` en el medio el sintoma era otro —dos lineas de YAML pegadas—
/// y por eso el defecto se escondio: el bloque seguia cerrando igual.
#[test]
fn renaming_a_parent_in_the_middle_does_not_glue_the_next_line() {
    let text = "---\ntitle: X\nparent: 1\nstatus: done\n---\n\ncuerpo\n";
    let (out, _) = worklist_core::rewrite_references(text, "1", "epic", "ACC-14");
    assert!(out.contains("parent: ACC-14\nstatus: done"), "{out:?}");
}

/// El defecto de `5m`: el limite de palabra solo miraba a la derecha, asi que
/// renombrar `j` entraba adentro de `2j` y escribia `2ACC-77`.
#[test]
fn renaming_a_slug_does_not_touch_one_that_ends_with_it() {
    let text = "---\ntitle: k\nrelation.depends: [2j]\n---\n\ncuerpo\n";
    let (out, changed) = worklist_core::rewrite_references(text, "j", "user-story", "ACC-77");
    assert!(!changed, "no habia ninguna referencia a `j`");
    assert!(out.contains("[2j]"), "quedo intacto: {out:?}");
}

/// Y el que si es, se reescribe: los dos en el mismo campo, que es el caso
/// real del sprint 7.
#[test]
fn the_right_slug_is_rewritten_with_a_lookalike_next_to_it() {
    let text = "---\ntitle: k\nrelation.depends: [2j, j]\n---\n\ncuerpo\n";
    let (out, changed) = worklist_core::rewrite_references(text, "j", "user-story", "ACC-77");
    assert!(changed);
    assert!(out.contains("[2j, ACC-77]"), "{out:?}");
}

/// Dos referencias adyacentes **sin espacio** comparten el caracter que las
/// separa. Es lo que romperia meter el limite izquierdo en el patron: el
/// primer match se lo lleva y el segundo se queda sin.
#[test]
fn two_adjacent_references_are_both_rewritten() {
    let text = "---\ntitle: x\nrelation.depends: [j,j]\n---\n\ncuerpo\n";
    let (out, _) = worklist_core::rewrite_references(text, "j", "task", "ACC-77");
    assert_eq!(out.matches("ACC-77").count(), 2, "las dos: {out:?}");
    assert!(!out.contains(",j]"), "no quedo ninguna sin reescribir: {out:?}");
}

/// El slug al principio del campo tambien tiene limite: el `[` que lo abre, y
/// el inicio del texto cuando el valor va suelto.
#[test]
fn a_slug_at_the_start_of_the_field_is_rewritten() {
    let text = "---\ntitle: x\nrelation.depends: j\n---\n\ncuerpo\n";
    let (out, changed) = worklist_core::rewrite_references(text, "j", "task", "ACC-77");
    assert!(changed);
    assert!(out.contains("relation.depends: ACC-77"), "{out:?}");
}

// ─── el renombre llega al sprint ───────────────────────────────────────────
//
// Task `63`. Tres puertas y cada una alcanzaba sola: el subdirectorio no se
// recorria, el patron de link no admitia el `../`, y `items` no estaba en la
// lista de campos.

/// El campo `items` del sprint, que la task `4x` agrego y el renombre nunca
/// miraba.
#[test]
fn renaming_rewrites_the_sprint_items_field() {
    let text = "---\ntitle: El sprint\nstatus: open\nitems: [4h, 4j, 4i]\n---\n\nplan\n";
    let (out, changed) = worklist_core::rewrite_references(text, "4h", "task", "ACC-94");
    assert!(changed);
    assert!(out.contains("items: [ACC-94, 4j, 4i]"), "{out:?}");
}

/// Y no toca al que solo se le parece: `4h` no es `4hh`.
#[test]
fn the_items_field_respects_word_boundaries() {
    let text = "---\ntitle: x\nitems: [4hh, 4h]\n---\n\nplan\n";
    let (out, _) = worklist_core::rewrite_references(text, "4h", "task", "ACC-94");
    assert!(out.contains("items: [4hh, ACC-94]"), "{out:?}");
}

/// Un link desde `_sprints/` lleva `../`, y el prefijo se conserva: lo que
/// cambia es el nombre del archivo, no donde esta.
#[test]
fn a_link_with_dotdot_is_rewritten_keeping_the_prefix() {
    let text = "- [`4h` El titulo](../4h.task.md)\n";
    let (out, changed) = worklist_core::rewrite_references(text, "4h", "task", "ACC-94");
    assert!(changed);
    assert_eq!(out, "- [`ACC-94` El titulo](../ACC-94.task.md)\n");
}

/// Y con mas de un nivel tambien.
#[test]
fn a_link_with_two_levels_keeps_them_both() {
    let text = "ver [`4h`](../../4h.task.md)\n";
    let (out, _) = worklist_core::rewrite_references(text, "4h", "task", "ACC-94");
    assert_eq!(out, "ver [`ACC-94`](../../ACC-94.task.md)\n");
}

/// Un link sin prefijo sigue saliendo sin prefijo.
#[test]
fn a_link_without_prefix_stays_without_it() {
    let text = "ver [`4h`](4h.task.md)\n";
    let (out, _) = worklist_core::rewrite_references(text, "4h", "task", "ACC-94");
    assert_eq!(out, "ver [`ACC-94`](ACC-94.task.md)\n");
}

/// La tercera puerta: `rename_one` tiene que **abrir el subdirectorio**.
#[test]
fn renaming_reaches_files_in_subdirectories() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    let g = |args: &[&str]| {
        let st = std::process::Command::new("git").arg("-C").arg(r).args(args).status().unwrap();
        assert!(st.success(), "git {args:?}");
    };
    g(&["init", "-q"]);
    g(&["config", "user.email", "t@t"]);
    g(&["config", "user.name", "t"]);
    std::fs::write(
        r.join("4h.task.md"),
        "---\ntitle: La task\nstatus: open\n---\n\ncuerpo\n",
    )
    .unwrap();
    std::fs::create_dir(r.join("_sprints")).unwrap();
    std::fs::write(
        r.join("_sprints/10.sprint.md"),
        "---\ntitle: El sprint\nstatus: open\nitems: [4h]\n---\n\n- [`4h` La task](../4h.task.md)\n",
    )
    .unwrap();
    g(&["add", "-A"]);
    g(&["commit", "-qm", "seed"]);

    let touched = worklist_core::rename_one(r, "4h", "ACC-94").unwrap();
    assert!(
        touched.iter().any(|t| t.contains("10.sprint.md")),
        "el sprint tiene que estar entre los tocados: {touched:?}"
    );
    let sprint = std::fs::read_to_string(r.join("_sprints/10.sprint.md")).unwrap();
    assert!(sprint.contains("items: [ACC-94]"), "{sprint}");
    assert!(sprint.contains("](../ACC-94.task.md)"), "{sprint}");
    assert!(sprint.contains("[`ACC-94`"), "{sprint}");
    assert!(r.join("ACC-94.task.md").exists());
}

/// El renombre de un pedido de verdad: el nombre viejo lleva la marca, y el
/// nuevo es la clave del proveedor. La marca no es un delimitador —es parte
/// del id—, asi que `@a` no matchea adentro de `@a1` ni de `x@a`.
#[test]
fn renombrar_un_id_marcado_no_come_la_marca_ni_toca_a_sus_vecinos() {
    let dir = git_repo();
    let repo = dir.path();
    write(
        repo,
        "@a.task.md",
        "---\ntitle: A\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n# A\n",
    );
    write(
        repo,
        "@a1.task.md",
        "---\ntitle: A1\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\n---\n# A1\n",
    );
    write(
        repo,
        "@b.task.md",
        "---\ntitle: B\nstatus: open\ncreated_at: 2026-09-04T00:00:00Z\nupdated_at: 2026-09-04T00:00:00Z\nparent: @a\nrelation.depends: [@a1]\n---\nSale de [`@a`](@a.task.md), y menciona @a1 al pasar.\n",
    );
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", "seed"]);

    let touched = worklist_core::rename_one(repo, "@a", "ACC-347").unwrap();
    assert_eq!(touched, vec!["@b.task.md".to_string()]);
    assert!(repo.join("ACC-347.task.md").exists());
    assert!(!repo.join("@a.task.md").exists());

    let b = std::fs::read_to_string(repo.join("@b.task.md")).unwrap();
    assert!(b.contains("parent: ACC-347"), "{b}");
    assert!(b.contains("[`ACC-347`](ACC-347.task.md)"), "{b}");
    assert!(b.contains("relation.depends: [@a1]"), "el vecino mas largo no se toco: {b}");
    assert!(b.contains("menciona @a1 al pasar"), "ni su mencion suelta: {b}");
    assert!(repo.join("@a1.task.md").exists());
}
