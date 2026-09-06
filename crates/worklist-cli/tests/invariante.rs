//! La invariante del corte, verificada y no confiada.
//!
//! Que el cliente no pueda escribir en el proveedor es **una linea de un
//! `Cargo.toml`**: el dia que alguien agregue `worklist-provider` a las
//! dependencias, el corte se deshace sin que nada lo diga y sin que ningun
//! otro test se ponga rojo — el codigo compilaria igual.
//!
//! Ver `concepts/distribution.md`.

#[test]
fn el_cliente_no_enlaza_el_proveedor() {
    let manifest = include_str!("../Cargo.toml");
    assert!(
        !manifest.contains("worklist-provider"),
        "worklist-cli enlazo worklist-provider: el binario del cliente pasa a tener \
         el codigo que habla con el proveedor, y la invariante de \
         concepts/distribution.md deja de sostenerse.\n\n\
         Lo que el cliente necesita del proveedor se lo pide al servidor."
    );
}
