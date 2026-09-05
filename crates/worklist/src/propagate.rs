//! La propagacion hacia arriba: lo que una ventana resolvio llega al panorama.
//!
//! Corre al final del `post-receive`, cuando el servidor ya escribio lo suyo:
//! lo que mas falta arriba son las claves, y las escribe el. Ver
//! `concepts/propagation.md`.
//!
//! **El renombre es el unico que no se copia.** Lleva el `git mv` y la
//! reescritura de todo lo que nombraba al slug, y esa reescritura recorre el
//! arbol donde corre: 19 archivos en la ventana, 243 en el panorama. Copiado
//! tal cual dejaria el archivo movido y las referencias de afuera del recorte
//! apuntando a un slug que ya no existe. Asi que se rehace.

use anyhow::{bail, Context, Result};
use std::path::Path;

/// El panorama: la rama de la que se corta todo y a la que nadie empuja.
pub const PANORAMA: &str = "refs/heads/insecure/all";

/// Donde el servidor anota hasta donde subio una ventana.
///
/// Va en `refs/worklist/**` y no en una rama: es contabilidad del servidor, no
/// contenido. Deducirlo comparando arboles seria preguntar *"este cambio ya
/// esta?"* sobre el panorama entero en cada push, y contestarlo mal en cuanto
/// dos ventanas tocaran lo mismo.
pub fn propagated_ref(refname: &str) -> String {
    let short = refname.strip_prefix("refs/heads/").unwrap_or(refname);
    format!("refs/worklist/propagated/{short}")
}

/// Que se hizo con cada commit de la ventana.
#[derive(Debug)]
pub enum Step {
    /// Se cherry-pickeo tal cual.
    Picked { sha: String, subject: String },
    /// El commit no aportaba nada nuevo: el panorama ya lo tenia.
    Empty { sha: String, subject: String },
    /// Un renombre, **rehecho** sobre el arbol del panorama.
    Renamed { slug: String, key: String, rewritten: usize },
    /// Un renombre que el panorama ya tenia hecho: no queda nada que rehacer.
    AlreadyRenamed { slug: String, key: String },
}

#[derive(Debug)]
pub struct Propagated {
    pub refname: String,
    /// Hasta donde estaba propagada antes de esta corrida.
    pub from: String,
    /// El tip de la ventana que quedo propagado.
    pub to: String,
    pub steps: Vec<Step>,
    pub panorama_old: String,
    pub panorama_new: String,
}

fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
    let out = crate::git_command(repo)
        .args(args)
        .output()
        .with_context(|| format!("corriendo git {args:?}"))?;
    if !out.status.success() {
        bail!("git {args:?} fallo: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8(out.stdout)?)
}

fn try_git(repo: &Path, args: &[&str]) -> Result<(bool, String)> {
    let out = crate::git_command(repo)
        .args(args)
        .output()
        .with_context(|| format!("corriendo git {args:?}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

fn rev_parse(repo: &Path, refname: &str) -> Option<String> {
    let out = crate::git_command(repo).args(["rev-parse", "--verify", "-q", refname]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// `rename <slug> -> <clave>` o `rename <slug> -> <clave> (N refs)`.
fn rename_subject(subject: &str) -> Option<(String, String)> {
    let rest = subject.strip_prefix("rename ")?;
    let (slug, rest) = rest.split_once(" -> ")?;
    let key = rest.split(' ').next()?;
    if slug.is_empty() || key.is_empty() {
        return None;
    }
    Some((slug.to_string(), key.to_string()))
}

/// El commit del corte: el primero que la ventana tiene sobre el panorama.
///
/// Todo lo que esta **encima** es trabajo de la ventana; el corte no, porque es
/// un commit que borra los items que el recorte dejo afuera. Lo usan las dos
/// direcciones: subir arranca despues del corte, y regenerar lo reemplaza por
/// uno nuevo y replanta lo que estaba encima.
pub fn cut_commit(repo: &Path, tip: &str, base_ref: &str) -> Result<String> {
    let base = git_output(repo, &["merge-base", base_ref, tip])?.trim().to_string();
    let log = git_output(repo, &["log", "--reverse", "--format=%H%x09%s", &format!("{base}..{tip}")])?;
    let first = log.lines().next().unwrap_or_default();
    let (sha, subject) = first.split_once('\t').unwrap_or((first, ""));
    if !subject.starts_with("window: ") {
        bail!(
            "no encuentro el corte de esta ventana: el primer commit sobre el panorama es\n\
             \x20 {} {subject}\n\
             \n\
             sin el corte no se sabe donde empieza el trabajo de la ventana, y propagarlo\n\
             entero le borraria al panorama todo lo que el recorte dejo afuera.",
            &sha[..7.min(sha.len())]
        );
    }
    Ok(sha.to_string())
}

/// Que paso al re-aplicar un commit sobre el HEAD de un worktree.
pub(crate) enum Picked {
    Applied,
    /// El arbol ya lo tenia: la re-aplicacion no aporta nada.
    Empty,
    Conflict { files: Vec<String>, output: String },
}

/// Cherry-pick de un commit, distinguiendo *no aporta nada* de *choca*.
///
/// La diferencia se lee del indice y no del codigo de salida: git falla en los
/// dos casos. Si no quedo nada en conflicto, el commit ya estaba aplicado.
pub(crate) fn cherry_pick_one(tmp: &Path, sha: &str) -> Result<Picked> {
    let (ok, output) = try_git(tmp, &["cherry-pick", "--allow-empty", sha])?;
    if ok {
        return Ok(Picked::Applied);
    }
    let unmerged = git_output(tmp, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    if unmerged.trim().is_empty() {
        let _ = try_git(tmp, &["cherry-pick", "--skip"]);
        return Ok(Picked::Empty);
    }
    let files = unmerged.lines().map(|s| s.to_string()).collect();
    let _ = try_git(tmp, &["cherry-pick", "--abort"]);
    Ok(Picked::Conflict { files, output })
}

/// Los commits que faltan subir, en orden, con su asunto.
pub fn pending(repo: &Path, refname: &str, tip: &str) -> Result<(String, Vec<(String, String)>)> {
    let from = match rev_parse(repo, &propagated_ref(refname)) {
        Some(sha) => sha,
        None => cut_commit(repo, tip, PANORAMA)?,
    };
    let log = git_output(repo, &["log", "--reverse", "--format=%H%x09%s", &format!("{from}..{tip}")])?;
    let commits = log
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(a, b)| (a.to_string(), b.to_string())))
        .collect();
    Ok((from, commits))
}

/// Si lo que este push deja en la ventana entraria al panorama.
///
/// Es el paso del `pre-receive`: **prueba** el cherry-pick, no lo aplica. La
/// misma forma que el compare-and-swap, y por la misma razon — una escritura
/// tiene que probar que parte del estado actual, y de `all` se corta todo.
///
/// Se hace con `merge-tree`, que resuelve en memoria: un `pre-receive` no
/// puede dejar un worktree ni objetos atras si despues rechaza.
///
/// **Los renombres no se prueban**, porque no se copian: se rehacen sobre el
/// arbol del panorama, y ahi no hay parche que pueda no aplicar.
///
/// `None` si no hay nada que probar o si todo entra. `Some` nombra el primer
/// commit que no.
pub fn would_conflict(
    repo: &Path,
    refname: &str,
    tip: &str,
) -> Result<Option<(String, String, Vec<String>)>> {
    if tip == crate::check_push::ALL_ZEROS {
        return Ok(None);
    }
    let Some(panorama) = rev_parse(repo, PANORAMA) else { return Ok(None) };
    let (_, commits) = pending(repo, refname, tip)?;

    let mut sobre = panorama;
    for (sha, subject) in &commits {
        if rename_subject(subject).is_some() {
            continue;
        }
        let base = format!("{sha}^");
        let (ok, out) = try_git(
            repo,
            &["merge-tree", "--write-tree", &format!("--merge-base={base}"), &sobre, sha],
        )?;
        let tree = out.lines().next().unwrap_or_default().trim().to_string();
        if !ok {
            // Sin `-z`, la salida lleva el arbol, una linea en blanco, y los
            // conflictos en `<modo> <oid> <etapa>\t<archivo>`.
            let mut files: Vec<String> = out
                .lines()
                .skip(1)
                .filter_map(|l| l.split_once('\t').map(|(_, f)| f.to_string()))
                .collect();
            files.sort();
            files.dedup();
            return Ok(Some((sha.clone(), subject.clone(), files)));
        }
        // El resultado se envuelve en un commit para que el siguiente parche se
        // pruebe sobre el arbol que el anterior dejo, y no sobre el original.
        sobre = git_output(repo, &["commit-tree", &tree, "-p", &sobre, "-m", "probando"])?
            .trim()
            .to_string();
    }
    Ok(None)
}

/// Sube al panorama lo que la ventana resolvio y el panorama todavia no tiene.
///
/// `None` cuando no hay nada que subir. **Si algo no aplica, el panorama no
/// avanza**: de `all` se corta todo, asi que un marcador de conflicto escrito
/// ahi entra en el proximo recorte de cada ventana. La ventana queda
/// adelantada, que es un estado del que se sale reintentando.
pub fn propagate(repo: &Path, refname: &str, tip: &str, dry_run: bool) -> Result<Option<Propagated>> {
    if tip == crate::check_push::ALL_ZEROS {
        return Ok(None);
    }
    let panorama_old = match rev_parse(repo, PANORAMA) {
        Some(sha) => sha,
        None => bail!("no hay panorama en este repo: {PANORAMA} no existe"),
    };
    let (from, commits) = pending(repo, refname, tip)?;
    if commits.is_empty() {
        return Ok(None);
    }
    if dry_run {
        let steps = commits
            .iter()
            .map(|(sha, subject)| match rename_subject(subject) {
                Some((slug, key)) => Step::Renamed { slug, key, rewritten: 0 },
                None => Step::Picked { sha: sha.clone(), subject: subject.clone() },
            })
            .collect();
        return Ok(Some(Propagated {
            refname: refname.to_string(),
            from,
            to: tip.to_string(),
            steps,
            panorama_old: panorama_old.clone(),
            panorama_new: panorama_old,
        }));
    }

    // Un worktree temporal en --detach: el panorama no tiene worktree en un
    // repo bare, y darle uno lo dejaria sin poder recibir el proximo corte.
    let tmp = repo.join(format!("../.worklist-propagate-{}", refname.replace('/', "-")));
    let _ = std::fs::remove_dir_all(&tmp);
    git_output(repo, &["worktree", "add", "--detach", "-q", tmp.to_str().unwrap(), &panorama_old])?;

    let result = (|| -> Result<Vec<Step>> {
        let mut steps = Vec::new();
        for (sha, subject) in &commits {
            match rename_subject(subject) {
                // El renombre se rehace: su reescritura es del arbol donde
                // corre, y el panorama tiene un arbol mas grande.
                Some((slug, key)) => match crate::find_file(&tmp, &slug) {
                    Ok(_) => {
                        let touched = crate::rename_one(&tmp, &slug, &key)?;
                        steps.push(Step::Renamed { slug, key, rewritten: touched.len() });
                    }
                    Err(_) => steps.push(Step::AlreadyRenamed { slug, key }),
                },
                None => match cherry_pick_one(&tmp, sha)? {
                    Picked::Applied => {
                        steps.push(Step::Picked { sha: sha.clone(), subject: subject.clone() })
                    }
                    Picked::Empty => {
                        steps.push(Step::Empty { sha: sha.clone(), subject: subject.clone() })
                    }
                    Picked::Conflict { files, output } => bail!(
                        "el panorama no recibe {} {subject}\n\
                         \x20 choca en: {}\n\
                         \n\
                         el panorama no avanzo y la ventana queda adelantada — se reintenta.\n\
                         Lo que el proveedor arbitra no llega hasta aca: si esto choco, dos\n\
                         ventanas escribieron algo que el no ve. Ver `concepts/propagation.md`.\n\
                         \n{}",
                        &sha[..7.min(sha.len())],
                        files.join(", "),
                        output.trim()
                    ),
                },
            }
        }
        Ok(steps)
    })();

    let panorama_new = git_output(&tmp, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string());
    let _ = git_output(repo, &["worktree", "remove", "--force", tmp.to_str().unwrap()]);
    let steps = result?;
    let panorama_new = panorama_new?;

    // El panorama primero, la marca despues: si algo se cae en el medio, la
    // marca sin mover hace que el reintento vuelva a pasar por lo mismo, y no
    // que se saltee lo que no subio.
    if panorama_new != panorama_old {
        git_output(repo, &["update-ref", PANORAMA, &panorama_new, &panorama_old])?;
    }
    git_output(repo, &["update-ref", &propagated_ref(refname), tip])?;

    Ok(Some(Propagated {
        refname: refname.to_string(),
        from,
        to: tip.to_string(),
        steps,
        panorama_old,
        panorama_new,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_asunto_del_renombre_se_lee_con_y_sin_refs() {
        assert_eq!(
            rename_subject("rename m -> ACC-93"),
            Some(("m".into(), "ACC-93".into()))
        );
        assert_eq!(
            rename_subject("rename m -> ACC-93 (2 refs)"),
            Some(("m".into(), "ACC-93".into()))
        );
        assert_eq!(rename_subject("normalize: ACC-93"), None);
        assert_eq!(rename_subject("window: sprint/1 recortado desde insecure/all"), None);
    }

    #[test]
    fn la_marca_vive_afuera_de_las_ramas() {
        assert_eq!(
            propagated_ref("refs/heads/secure/sprint/17"),
            "refs/worklist/propagated/secure/sprint/17"
        );
    }
}
