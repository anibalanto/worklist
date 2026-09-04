//! Lo que se puede probar sin tocar Jira: el texto con el que se busca, la
//! lectura del resultado del proveedor, y que `--dry-run` no llama a `acli`.
//! El create-or-find real contra un board necesita permiso explícito — no
//! corre en CI.

use worklist::creator::{dry_run_plan, jira_type, search_text};

/// Ya no hay nada que escapar: el texto de busqueda no puede contener una
/// comilla, porque no es alfanumerica. La query sale sin metacaracteres en vez
/// de con metacaracteres escapados.
#[test]
fn un_titulo_con_comillas_no_rompe_el_jql_generado() {
    let plan = dry_run_plan("ACC", "task", r#"El card "raro""#, "Fuente: x.task.md").unwrap();
    let search_line = plan.lines().next().unwrap();
    assert!(search_line.contains(r#"summary ~ "El card raro""#), "{search_line}");
    assert!(!search_line.contains('\\'), "no hace falta escapar nada: {search_line}");
    // El titulo real, en cambio, viaja entero en `--summary`.
    assert!(plan.contains(r#"--summary "El card \"raro\"""#), "{plan}");
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
    assert_eq!(search_text("[prueba] algo"), "prueba  algo");
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

/// El defecto de `5l`: `*` es un comodin del full-text y `refs/bilink/*`
/// devolvia cero sobre un issue que existia, asi que el reintento duplicaba.
/// No se escapa: se busca por lo alfanumerico, que es seguro por construccion.
#[test]
fn the_search_text_keeps_only_what_no_parser_can_choke_on() {
    let t = "Índice git propio y refspecs de `refs/bilink/*`";
    for c in ['*', '[', ']', '(', ')', '{', '}', '^', '"'] {
        assert!(!search_text(t).contains(c), "quedo un {c:?} que rompe la JQL");
    }
    // La barra **se conserva**: no rompe, y sacarla rompe la tokenizacion.
    assert!(search_text(t).contains("refs/bilink"), "{}", search_text(t));
}

/// Los acentos se conservan: el full-text **no** los normaliza, asi que
/// buscar "Indice" no encuentra un issue titulado "Índice".
#[test]
fn the_search_text_keeps_the_accents() {
    assert!(search_text("Índice de migración").contains('Í'));
    assert!(search_text("Índice de migración").contains('ó'));
}

/// Un titulo hecho solo de simbolos no deja con que buscar. Eso no puede
/// mandarse como query, y crear a ciegas seria duplicar por otro camino.
/// Un titulo hecho solo de metacaracteres no deja con que buscar.
#[test]
fn a_title_with_nothing_but_metacharacters_leaves_no_query() {
    assert_eq!(search_text("*** [] {}"), "");
}

/// Y no abre ni cierra con espacio: dos separadores seguidos son uno.
/// No abre ni cierra con espacio, y un metacaracter entre palabras las separa
/// en vez de pegarlas.
#[test]
fn the_search_text_does_not_pad_with_spaces() {
    let t = search_text("[hola]algo[chau]");
    assert!(!t.starts_with(' ') && !t.ends_with(' '), "{t:?}");
    assert_eq!(t, "hola algo chau");
}

/// El defecto de `65`: el guion es parte del token, y sacarlo hace que la
/// busqueda no encuentre un issue que existe — y el reintento lo duplica.
#[test]
fn the_hyphen_survives_because_it_is_part_of_the_token() {
    let t = "Escribir `bilinker-002-file-partition`";
    assert!(search_text(t).contains("bilinker-002-file-partition"), "{}", search_text(t));
}

/// Y los otros que se midieron como seguros contra Jira.
#[test]
fn the_characters_measured_as_safe_survive() {
    for c in ['-', '.', '/', '_', ':', '#', '?', '+', '&', '|', '!', '~', '\''] {
        let t = format!("proba{c}proba");
        assert!(search_text(&t).contains(c), "se fue un {c:?}, que no rompe la JQL");
    }
}
