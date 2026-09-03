//! Lo que se puede probar sin tocar Jira: el escapado de JQL, y que
//! `--dry-run` no llama a `acli`. El create-or-find real contra un board
//! necesita permiso explícito — no corre en CI.

use worklist::creator::{dry_run_plan, escape_jql, jira_type, search_text};

#[test]
fn escapa_comillas_y_backslash() {
    assert_eq!(escape_jql(r#"El card "Acceso al Portal""#), r#"El card \"Acceso al Portal\""#);
    assert_eq!(escape_jql(r"ruta\archivo"), r"ruta\\archivo");
    assert_eq!(escape_jql("sin nada especial"), "sin nada especial");
}

#[test]
fn un_titulo_con_comillas_no_rompe_el_jql_generado() {
    let plan = dry_run_plan("ACC", "task", r#"El card "raro""#, "Fuente: x.task.md").unwrap();
    assert!(plan.contains(r#"summary ~ "El card \"raro\"""#));
}

#[test]
fn el_tipo_es_del_worklist_y_se_traduce_a_jira() {
    assert_eq!(jira_type("task").unwrap(), "Tarea");
    assert_eq!(jira_type("user-story").unwrap(), "Historia");
    assert_eq!(jira_type("epic").unwrap(), "Epic");
    assert!(jira_type("Task").is_err(), "el vocabulario es el del worklist, no el de Jira");
    assert!(jira_type("sprint").is_err(), "un sprint no se crea como issue");
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
    let plan = dry_run_plan("ACC", "task", "Titulo normal", "Fuente: y.task.md").unwrap();
    assert!(plan.starts_with("would search:"));
    assert!(plan.contains("would create:"));
    assert!(plan.contains("--type Tarea"));
    assert!(plan.contains("--description \"Fuente: y.task.md\""));
}
