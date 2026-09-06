//! El alfabeto de un id y la marca de lo local.
//!
//! Ver `concepts/item.md` secciones "El alfabeto de un id" y "La marca `@`".

/// Lo que hundio al regex anterior: `^[A-Z]+-\d+$` decia "local" sobre
/// cualquier cosa que no tuviera forma de clave de Jira, y un entero pelado
/// —GitHub, GitLab, Taiga— la tiene tan poco como un id base-36.
#[test]
fn un_entero_pelado_ya_no_se_lee_como_pedido() {
    assert!(!worklist_core::is_unassigned("10"), "es la clave 10 del proveedor");
    assert!(worklist_core::is_unassigned("@10"), "y esta es la marcada");
}

#[test]
fn la_marca_es_lo_unico_que_distingue_un_pedido() {
    assert!(worklist_core::is_unassigned("@arreglar-el-hook"));
    assert!(!worklist_core::is_unassigned("ACC-347"));
    // Y no conoce a ningun proveedor: la forma de la clave no entra en la
    // decision, ni para acertar ni para errar.
    assert!(!worklist_core::is_unassigned("PROJ_42/beta"));
}

#[test]
fn el_alfabeto_toma_la_marca_una_vez_y_adelante() {
    for id in ["@a", "@arreglar-el-hook", "ACC-347", "5p", "con_guion_bajo"] {
        assert!(worklist_core::is_valid_id(id), "{id} tendria que ser valido");
    }
    for id in ["", "@", "@@a", "a@b", "a.b", "_sprints/20", "con espacio", "acento-á"] {
        assert!(!worklist_core::is_valid_id(id), "{id} no tendria que serlo");
    }
}

/// El `.` es el separador de tipo y el `/` dice que el item no vive en la raiz:
/// los dos rompen el nombre del archivo, no el gusto de nadie.
#[test]
fn el_punto_y_la_barra_quedan_afuera_por_lo_que_rompen() {
    assert!(!worklist_core::is_valid_id("@a.sprint"), "seria `@a` el sprint o `@a.sprint` el item");
    assert!(!worklist_core::is_valid_id("_sprints/20"), "los items viven en la raiz");
}
