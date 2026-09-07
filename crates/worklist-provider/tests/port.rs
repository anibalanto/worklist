//! El puerto: el reparto de transporte, la forma unica de un fallo, y que la
//! credencial que falta se nombre por su nombre.

use worklist_provider::port::{missing, Failure, Op, Transport, TOKEN_ENV};

/// El reparto de `concepts/sync.md` seccion "El reparto, escrito una vez",
/// fila por fila. Si la tabla cambia, este test es el que lo dice.
#[test]
fn el_reparto_es_el_de_la_spec() {
    for op in [Op::Search, Op::Create, Op::SetSummary, Op::SetDescription, Op::Link, Op::Snapshot] {
        assert_eq!(op.transport(), Transport::Acli, "{op:?} deberia ir por acli");
    }
    // Las que `acli` no puede: `edit` no acepta ni `parent` ni el sprint, y
    // ninguno de sus comandos de sprint lista los del board.
    for op in [Op::SetParent, Op::AddToSprint, Op::SprintList] {
        assert_eq!(op.transport(), Transport::JiraCli, "{op:?} deberia ir por jira-cli");
    }
    // Leer la epica si es de `acli`: lo que no puede es escribirla.
    assert_eq!(Op::ParentOf.transport(), Transport::Acli);
    // Y el sprint se crea y se lee con `acli`: lo unico que no sabe hacer con
    // uno es meterle un issue y enumerar los del board.
    assert_eq!(Op::CreateSprint.transport(), Transport::Acli);
    assert_eq!(Op::SprintItems.transport(), Transport::Acli);
    // Las dos de estado, por REST: **ningun CLI puede listar las transiciones
    // de un issue**, y sin listar no hay id con que pedir la transicion.
    for op in [Op::Transition, Op::TransitionsOf] {
        assert_eq!(op.transport(), Transport::Api, "{op:?} deberia ir por la API");
    }
}


/// Un fallo se lee igual venga de donde venga: la clave, el transporte que se
/// quejo y el motivo. Es lo que evita que agregar un transporte multiplique
/// los modos de falla que hay que conocer rio arriba.
#[test]
fn un_fallo_lleva_la_clave_el_transporte_y_el_motivo() {
    let f = Failure::new(Op::AddToSprint, Some("ACC-100"), "sprint 7 no existe");
    let s = f.to_string();
    assert!(s.contains("ACC-100"), "{s}");
    assert!(s.contains("jira"), "tiene que decir con cual se quejo: {s}");
    assert!(s.contains("sprint 7 no existe"), "{s}");
}

/// Al crear todavia no hay clave, y el fallo no inventa una.
#[test]
fn un_fallo_sin_clave_no_inventa_una() {
    let s = Failure::new(Op::Create, None, "nope").to_string();
    assert!(!s.contains("sobre"), "no hay clave que nombrar: {s}");
    assert!(s.contains("nope"), "{s}");
}

/// Hay **tres** formas de estar mal configurado, y decir "falta la credencial"
/// manda a mirar las dos que ya estaban bien.
#[test]
fn el_mensaje_dice_cual_de_las_tres_falta() {
    let solo_acli = missing(false, "un-token", "yo@ejemplo.com");
    assert_eq!(solo_acli.len(), 1);
    assert!(solo_acli[0].starts_with("acli:"), "{solo_acli:?}");

    let solo_token = missing(true, "", "yo@ejemplo.com");
    assert_eq!(solo_token.len(), 1);
    assert!(solo_token[0].contains(TOKEN_ENV), "y dice como se llama: {solo_token:?}");

    let solo_cuenta = missing(true, "un-token", "");
    assert_eq!(solo_cuenta.len(), 1);
    assert!(solo_cuenta[0].contains("--account"), "y dice de donde sale: {solo_cuenta:?}");

    assert_eq!(missing(false, "", "").len(), 3, "las tres juntas se dicen las tres");
    assert!(missing(true, "un-token", "yo@ejemplo.com").is_empty());
}

/// El token y el email faltan por motivos distintos y se arreglan en lugares
/// distintos —uno es del entorno de quien empuja, el otro de como se
/// instalaron los hooks—, asi que se dicen por separado aunque los dos sean de
/// REST.
#[test]
fn el_token_y_el_email_no_se_dicen_juntos() {
    let faltan = missing(true, "", "");
    assert_eq!(faltan.len(), 2, "{faltan:?}");
}

/// Un token en blanco es un token que falta. Un `export JIRA_API_TOKEN=` deja
/// la variable puesta y vacia, y eso no es estar configurado.
#[test]
fn un_token_en_blanco_no_es_un_token() {
    assert_eq!(missing(true, "   ", "yo@ejemplo.com").len(), 1);
    assert_eq!(missing(true, "\n", "yo@ejemplo.com").len(), 1);
}
