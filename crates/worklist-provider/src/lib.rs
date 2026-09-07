//! Todo lo que enlaza el puerto del proveedor, y lo que escribe en las ramas
//! del servidor.
//!
//! `port` declara las operaciones y a que transporte le toca cada una; `board`
//! y `provider` las implementan contra los dos CLIs y `api` contra REST, que
//! es donde nace lo que se agregue; `check_push` es el
//! compare-and-swap de una ventana; `assign` resuelve sus pedidos; `propagate`
//! sube al panorama lo que la ventana resolvio; `push_states` cierra las
//! divergencias de estado que un push ya no puede alcanzar.
//!
//! **Este crate lo enlaza `worklist-server` y no `worklist-cli`.** Es el corte
//! que hace que el cliente no tenga con que usar una credencial aunque la
//! tenga — ver `concepts/distribution.md`.

pub mod api;
pub mod absorb;
pub mod assign;
pub mod board;
pub mod check_push;
pub mod port;
pub mod propagate;
pub mod provider;
pub mod push_states;
pub mod removes;
pub mod states;
