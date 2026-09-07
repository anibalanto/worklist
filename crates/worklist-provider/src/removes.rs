//! Lo que el proveedor perdio sale del arbol, y **no se borra**.
//!
//! Un item puede tener clave aca y no existir del otro lado. Medido el
//! 2026-09-07: `ACC-268` tenia su `rename 6m -> ACC-268` en el log y Jira
//! contestaba 404. Y eso **traba el item para siempre**: `is_unassigned` es
//! falso, asi que ninguna pasada vuelve a mirarlo, y su clave muerta se lleva
//! el lote del sprint entero en cada push.
//!
//! Ver `concepts/composition.md` seccion "Y lo que el proveedor perdio se saca,
//! pero no se borra".

use crate::provider::{Existencia, Provider};
use anyhow::{bail, Result};
use std::path::Path;
use worklist_core::git::git_output;

/// Donde van. **Se mueven, no se destruyen**: el item deja de ser un item y su
/// contenido queda a la vista, sin arqueologia de git.
pub const DIR: &str = ".metadata/removes";

/// Que se decidio sobre una clave.
#[derive(Debug)]
pub enum Paso {
    /// 404: el proveedor no la tiene. Sale del arbol.
    Sacado { key: String, de: String, a: String },
    /// 403: **no se toca.** Una credencial sin permiso no es un item borrado, y
    /// el mensaje del proveedor no los distingue — el codigo si.
    SinPermiso { key: String },
    /// El proveedor no lo puede contestar: el de prueba no tiene con que. **No
    /// es que exista**, es que no se pregunto.
    NoSePudoPreguntar { key: String },
}

#[derive(Debug, Default)]
pub struct Resultado {
    pub claves: usize,
    pub pasos: Vec<Paso>,
    pub commit: Option<String>,
}

impl Resultado {
    pub fn sacados(&self) -> usize {
        self.pasos.iter().filter(|p| matches!(p, Paso::Sacado { .. })).count()
    }
}

/// Saca del arbol los items cuya clave el proveedor ya no tiene.
///
/// **Corre sobre el panorama**, que es donde estan las dos cosas que hay que
/// tocar: el archivo del item y el `items` que lo nombra. Las ventanas no se
/// tocan — se regeneran, y el recorte ya no lo incluye. Es la misma forma que
/// la composicion habilito para cualquier cambio de membresia.
///
/// **Pregunta clave por clave y no en lote**, al contrario de `snapshot`: lo
/// que se decide con la respuesta es sacar un archivo, y una respuesta de lote
/// que no distingue *no existe* de *no se pudo ver* no alcanza para eso.
pub fn recolectar(
    repo: &Path,
    refname: &str,
    provider: &dyn Provider,
    dry_run: bool,
) -> Result<Resultado> {
    let beliefs = crate::check_push::tip_beliefs(repo, refname)?;
    let mut claves: Vec<String> = beliefs.keys().cloned().collect();
    claves.sort();

    let mut out = Resultado { claves: claves.len(), ..Default::default() };
    let mut mover: Vec<(String, String)> = Vec::new();

    for key in &claves {
        match provider.existe(key)? {
            None => out.pasos.push(Paso::NoSePudoPreguntar { key: key.clone() }),
            Some(Existencia::Si) => {}
            Some(Existencia::SinPermiso) => {
                out.pasos.push(Paso::SinPermiso { key: key.clone() })
            }
            Some(Existencia::Borrada) => {
                let Ok(de) = archivo_de(repo, refname, key) else { continue };
                let a = format!("{DIR}/{}", de.rsplit('/').next().unwrap_or(&de));
                out.pasos.push(Paso::Sacado { key: key.clone(), de: de.clone(), a: a.clone() });
                mover.push((de, a));
            }
        }
    }

    if mover.is_empty() || dry_run {
        return Ok(out);
    }
    out.commit = Some(mover_en_rama(repo, refname, &mover)?);
    Ok(out)
}

fn archivo_de(repo: &Path, rev: &str, key: &str) -> Result<String> {
    let listing = git_output(repo, &["ls-tree", "-r", "--name-only", rev])?;
    listing
        .lines()
        .find(|n| crate::provider::key_of_filename(n).as_deref() == Some(key))
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("{key} no esta en {rev}"))
}

/// El `git mv` sobre la rama, en un worktree temporal.
///
/// Es un movimiento y no una escritura: git lo registra como rename, asi que
/// devolverlo es moverlo de nuevo. **Si el 404 fue un error, restaurar es un
/// `git mv` y no un rescate.**
fn mover_en_rama(repo: &Path, refname: &str, mover: &[(String, String)]) -> Result<String> {
    let tmp = repo.join("../.worklist-removes");
    let _ = std::fs::remove_dir_all(&tmp);
    git_output(repo, &["worktree", "add", "--detach", "-q", tmp.to_str().unwrap(), refname])?;

    let hecho = (|| -> Result<String> {
        std::fs::create_dir_all(tmp.join(DIR))?;
        let mut claves = Vec::new();
        for (de, a) in mover {
            git_output(&tmp, &["mv", de, a])?;
            if let Some(k) = crate::provider::key_of_filename(de) {
                // **Y sale del `items`, o el proximo recorte falla** con "la
                // composicion nombra a X, y no esta". No es una mejora: sacar
                // el archivo sin sacar la referencia deja el sprint roto.
                worklist_core::product::sacar_de(&tmp, &k)?;
                claves.push(k);
            }
        }
        if claves.is_empty() {
            bail!("no habia nada que mover");
        }
        worklist_core::commit_all(
            &tmp,
            &format!("removes: {} — el proveedor no las tiene", claves.join(", ")),
        )?;
        Ok(git_output(&tmp, &["rev-parse", "HEAD"])?.trim().to_string())
    })();

    let _ = git_output(repo, &["worktree", "remove", "--force", tmp.to_str().unwrap()]);
    let head = hecho?;
    git_output(repo, &["update-ref", refname, &head])?;
    Ok(head)
}
