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
pub enum Step {
    /// Nacio como item, con su clave por nombre.
    Adopted { key: String, file: String, kind: &'static str },
    /// El proveedor tiene un tipo que el worklist no modela —un `Bug`, una
    /// `Sub-tarea`—. **No es un error**: adoptarlo pediria decidir a que se
    /// parece, y eso lo decide una persona.
    UnknownType { key: String, kind: String },
    /// El proveedor no informo lo suficiente para escribir el item.
    NoData { key: String },
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub steps: Vec<Step>,
    pub commit: Option<String>,
}

impl Outcome {
    pub fn adopted(&self) -> usize {
        self.steps.iter().filter(|p| matches!(p, Step::Adopted { .. })).count()
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
pub fn adopt(
    repo: &Path,
    refname: &str,
    keys: &[String],
    provider: &dyn Provider,
    dry_run: bool,
) -> Result<Outcome> {
    let mut out = Outcome::default();
    if keys.is_empty() {
        return Ok(out);
    }
    let live = provider.snapshot(keys)?;
    let mut to_write: Vec<(String, String)> = Vec::new();

    for key in keys {
        let Some(snap) = live.get(key) else {
            out.steps.push(Step::NoData { key: key.clone() });
            continue;
        };
        let (Some(title), Some(jira_kind)) = (&snap.summary, &snap.issue_type) else {
            out.steps.push(Step::NoData { key: key.clone() });
            continue;
        };
        let Some(kind) = crate::board::worklist_type(jira_kind) else {
            out.steps.push(Step::UnknownType {
                key: key.clone(),
                kind: jira_kind.clone(),
            });
            continue;
        };
        // **El nombre del archivo es la clave, que ya la tiene.** Es la unica
        // clase de item que nace con clave: no hay `@<slug>` que asignar ni
        // renombre que rehacer.
        let file = format!("{key}.{kind}.md");
        out.steps.push(Step::Adopted { key: key.clone(), file: file.clone(), kind });
        // Sin padre no es un error: nace suelto, que el formato admite. Un
        // fallo puntual de esta lectura tampoco frena la adopcion — el titulo
        // y el tipo ya se tienen, y perder el padre es menos costoso que
        // perder el item entero.
        let parent = provider.parent(key).ok().flatten();
        to_write.push((file, contents(title, kind, parent.as_deref())));
    }

    if to_write.is_empty() || dry_run {
        return Ok(out);
    }
    out.commit = Some(write_on_branch(repo, refname, &to_write)?);
    Ok(out)
}

/// El item que se escribe. **Sin cuerpo del proveedor**, con una linea que dice
/// de donde vino.
///
/// Sin `parent`, el item **nace suelto** — que el formato admite —, ya sea
/// porque el issue no tiene padre alla o porque esta lectura fallo: las dos
/// se corrigen igual, poniendo `parent` a mano.
fn contents(title: &str, kind: &str, parent: Option<&str>) -> String {
    let now = now();
    let escaped = title.replace('\'', "''");
    let parent_line = parent.map(|p| format!("parent: {p}\n")).unwrap_or_default();
    format!(
        "---\ntitle: '{escaped}'\nstatus: open\ncreated_at: {now}\nupdated_at: {now}\n{parent_line}---\n\
         \n# {title}\n\
         \n**Nacio en el proveedor** y se adopto: es un issue que existia del otro lado y no \
         de este. El cuerpo no bajo todavia —el round-trip no cierra— asi que lo que dice el \
         board sobre este item esta alla y no aca.\n\
         \nTipo: `{kind}`.\n"
    )
}

fn write_on_branch(repo: &Path, refname: &str, items: &[(String, String)]) -> Result<String> {
    let tmp = repo.join("../.worklist-adopt");
    let _ = std::fs::remove_dir_all(&tmp);
    git_output(repo, &["worktree", "add", "--detach", "-q", tmp.to_str().unwrap(), refname])?;

    let done = (|| -> Result<String> {
        let mut keys = Vec::new();
        for (file, text) in items {
            std::fs::write(tmp.join(file), text)?;
            keys.push(file.split('.').next().unwrap_or(file).to_string());
        }
        worklist_core::commit_all(
            &tmp,
            &format!("adopt: {} — nacieron en el proveedor", keys.join(", ")),
        )?;
        Ok(git_output(&tmp, &["rev-parse", "HEAD"])?.trim().to_string())
    })();

    let _ = git_output(repo, &["worktree", "remove", "--force", tmp.to_str().unwrap()]);
    let head = done?;
    git_output(repo, &["update-ref", refname, &head])?;
    Ok(head)
}

fn now() -> String {
    std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
