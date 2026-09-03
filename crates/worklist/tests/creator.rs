//! Lo que se puede probar sin tocar Jira: el escapado de JQL, y que
//! `--dry-run` no llama a `acli`. El create-or-find real contra un board
//! necesita permiso explícito — no corre en CI.

use worklist::creator::{dry_run_plan, escape_jql, search_text};

#[test]
fn escapa_comillas_y_backslash() {
    assert_eq!(escape_jql(r#"El card "Acceso al Portal""#), r#"El card \"Acceso al Portal\""#);
    assert_eq!(escape_jql(r"ruta\archivo"), r"ruta\\archivo");
    assert_eq!(escape_jql("sin nada especial"), "sin nada especial");
}

#[test]
fn un_titulo_con_comillas_no_rompe_el_jql_generado() {
    let plan = dry_run_plan("ACC", "Task", r#"El card "raro""#, "Fuente: x.task.md");
    assert!(plan.contains(r#"summary ~ "El card \"raro\"""#));
}

#[test]
fn corchetes_se_neutralizan_para_la_busqueda() {
    // Confirmado contra Jira real: "summary ~ \"[prueba] algo\"" no parsea,
    // aunque las comillas esten bien escapadas. `[`/`]` no tienen forma
    // valida de escaparse en JQL (`\[` es una secuencia ilegal).
    assert_eq!(search_text("[prueba] algo"), " prueba  algo");
    assert!(!search_text("[prueba] algo").contains('['));
    assert!(!search_text("[prueba] algo").contains(']'));
}

#[test]
fn dry_run_no_llama_a_acli() {
    // Si esto llamara a un binario real, un entorno sin `acli` en PATH
    // fallaria. `dry_run_plan` es una funcion pura de formateo.
    let plan = dry_run_plan("ACC", "Task", "Titulo normal", "Fuente: y.task.md");
    assert!(plan.starts_with("would search:"));
    assert!(plan.contains("would create:"));
    assert!(plan.contains("--description \"Fuente: y.task.md\""));
}
