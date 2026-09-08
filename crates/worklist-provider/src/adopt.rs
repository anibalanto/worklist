//! Un issue que nace en el board **baja como item**.
//!
//! **El proveedor es la autoridad**, asi que un issue que existe alla y no aca
//! es un item que falta, no un intruso. Ver `commands/absorb.md` seccion "Un
//! issue que nace en el board baja como item".
//!
//! Medido el 2026-09-08: se creo `ACC-330` directamente en el sprint 21 y
//! `worklist pull` no traia nada. El comando lo veia —`no es un item de este
//! lado`— y el resumen no lo contaba, asi que se callaba.
//!
//! **No es `reconcile`.** Aquel adopta la *correspondencia* —le pone la clave a
//! un item que ya esta de este lado— y esto crea el item. Comparten la palabra
//! y no la operacion.

use crate::provider::Provider;
use anyhow::Result;
use std::path::Path;
use worklist_core::git::git_output;

/// Que se hizo con un issue que el board tiene y el arbol no.
#[derive(Debug)]
pub enum Paso {
    /// Nacio como item, con su clave por nombre.
    Adoptado { key: String, file: String, tipo: &'static str },
    /// El proveedor tiene un tipo que el worklist no modela —un `Bug`, una
    /// `Sub-tarea`—. **No es un error**: adoptarlo pediria decidir a que se
    /// parece, y eso lo decide una persona.
    TipoDesconocido { key: String, tipo: String },
    /// El proveedor no informo lo suficiente para escribir el item.
    SinDatos { key: String },
}

#[derive(Debug, Default)]
pub struct Resultado {
    pub pasos: Vec<Paso>,
    pub commit: Option<String>,
}

impl Resultado {
    pub fn adoptados(&self) -> usize {
        self.pasos.iter().filter(|p| matches!(p, Paso::Adoptado { .. })).count()
    }
}

/// Trae al arbol los issues que el board tiene y el arbol no.
///
/// **El cuerpo no viene**, y no por autoria: hoy 113 de 149 cuerpos que
/// difieren son del conversor, asi que traerlo escribiria uno que el propio
/// round-trip no reproduce. El item nace con una linea que dice de donde vino,
/// y el cuerpo se absorbe cuando el conversor cierre.
///
/// **Un item sin cuerpo es valido**: el formato pide frontmatter, y la prosa es
/// opcional.
pub fn adoptar(
    repo: &Path,
    refname: &str,
    claves: &[String],
    provider: &dyn Provider,
    dry_run: bool,
) -> Result<Resultado> {
    let mut out = Resultado::default();
    if claves.is_empty() {
        return Ok(out);
    }
    let vivo = provider.snapshot(claves)?;
    let mut escribir: Vec<(String, String)> = Vec::new();

    for key in claves {
        let Some(snap) = vivo.get(key) else {
            out.pasos.push(Paso::SinDatos { key: key.clone() });
            continue;
        };
        let (Some(titulo), Some(jira_tipo)) = (&snap.summary, &snap.issue_type) else {
            out.pasos.push(Paso::SinDatos { key: key.clone() });
            continue;
        };
        let Some(tipo) = crate::board::worklist_type(jira_tipo) else {
            out.pasos.push(Paso::TipoDesconocido {
                key: key.clone(),
                tipo: jira_tipo.clone(),
            });
            continue;
        };
        // **El nombre del archivo es la clave, que ya la tiene.** Es la unica
        // clase de item que nace con clave: no hay `@<slug>` que asignar ni
        // renombre que rehacer.
        let file = format!("{key}.{tipo}.md");
        out.pasos.push(Paso::Adoptado { key: key.clone(), file: file.clone(), tipo });
        escribir.push((file, contenido(titulo, tipo)));
    }

    if escribir.is_empty() || dry_run {
        return Ok(out);
    }
    out.commit = Some(escribir_en_rama(repo, refname, &escribir)?);
    Ok(out)
}

/// El item que se escribe. **Sin cuerpo del proveedor**, con una linea que dice
/// de donde vino.
fn contenido(titulo: &str, tipo: &str) -> String {
    let ahora = ahora();
    let escapado = titulo.replace('\'', "''");
    format!(
        "---\ntitle: '{escapado}'\nstatus: open\ncreated_at: {ahora}\nupdated_at: {ahora}\n---\n\
         \n# {titulo}\n\
         \n**Nacio en el proveedor** y se adopto: es un issue que existia del otro lado y no \
         de este. El cuerpo no bajo todavia —el round-trip no cierra— asi que lo que dice el \
         board sobre este item esta alla y no aca.\n\
         \nTipo: `{tipo}`.\n"
    )
}

fn escribir_en_rama(repo: &Path, refname: &str, items: &[(String, String)]) -> Result<String> {
    let tmp = repo.join("../.worklist-adopt");
    let _ = std::fs::remove_dir_all(&tmp);
    git_output(repo, &["worktree", "add", "--detach", "-q", tmp.to_str().unwrap(), refname])?;

    let hecho = (|| -> Result<String> {
        let mut claves = Vec::new();
        for (file, texto) in items {
            std::fs::write(tmp.join(file), texto)?;
            claves.push(file.split('.').next().unwrap_or(file).to_string());
        }
        worklist_core::commit_all(
            &tmp,
            &format!("adopt: {} — nacieron en el proveedor", claves.join(", ")),
        )?;
        Ok(git_output(&tmp, &["rev-parse", "HEAD"])?.trim().to_string())
    })();

    let _ = git_output(repo, &["worktree", "remove", "--force", tmp.to_str().unwrap()]);
    let head = hecho?;
    git_output(repo, &["update-ref", refname, &head])?;
    Ok(head)
}

fn ahora() -> String {
    std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
