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

/// El defecto de fondo: `acli` sale con 0 y el fracaso viene en el cuerpo. Un
/// lote con un `FAILURE` es un fracaso, aunque el proceso haya salido bien.
#[test]
fn a_failure_in_the_batch_is_a_failure() {
    let v: serde_json::Value = serde_json::from_str(
        r#"{"results":[{"status":"FAILURE","message":"InvalidPayloadException: INVALID_INPUT","id":"ACC-16"}],
            "totalCount":1,"successCount":0}"#,
    )
    .unwrap();
    let err = worklist::creator::check_batch(&v, "edit").unwrap_err().to_string();
    assert!(err.contains("ACC-16"), "la clave tiene que estar: {err}");
    assert!(err.contains("INVALID_INPUT"), "el motivo tambien: {err}");
}

/// Y un lote que salio bien no molesta.
#[test]
fn a_successful_batch_passes() {
    let v: serde_json::Value = serde_json::from_str(
        r#"{"results":[{"status":"SUCCESS","message":"ok","id":"ACC-16"}],
            "totalCount":1,"successCount":1}"#,
    )
    .unwrap();
    assert!(worklist::creator::check_batch(&v, "edit").is_ok());
}

/// Un lote mixto falla, y nombra solo al que fallo: el que anduvo no se
/// reporta como problema y el que no, no se pierde entre los que si.
#[test]
fn a_mixed_batch_names_only_the_one_that_failed() {
    let v: serde_json::Value = serde_json::from_str(
        r#"{"results":[{"status":"SUCCESS","message":"ok","id":"ACC-1"},
                       {"status":"FAILURE","message":"nope","id":"ACC-2"}],
            "totalCount":2,"successCount":1}"#,
    )
    .unwrap();
    let err = worklist::creator::check_batch(&v, "edit").unwrap_err().to_string();
    assert!(err.contains("ACC-2"), "{err}");
    assert!(!err.contains("ACC-1"), "el que anduvo no es un problema: {err}");
}

/// `create` no responde con forma de lote —devuelve la clave sola—, y eso no
/// es un fracaso: no hay nada que chequear ahi.
#[test]
fn an_output_without_a_batch_shape_is_not_a_failure() {
    let v: serde_json::Value = serde_json::from_str(r#"{"key":"ACC-14"}"#).unwrap();
    assert!(worklist::creator::check_batch(&v, "create").is_ok());
}

/// Un lote vacio no falla: nada que reportar no es un fracaso.
#[test]
fn an_empty_batch_passes() {
    let v: serde_json::Value = serde_json::from_str(r#"{"results":[],"successCount":0}"#).unwrap();
    assert!(worklist::creator::check_batch(&v, "edit").is_ok());
}
