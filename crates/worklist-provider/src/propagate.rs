//! La propagacion hacia arriba: lo que una ventana resolvio llega al panorama.
//!
//! Corre al final del `post-receive`, cuando el servidor ya escribio lo suyo:
//! lo que mas falta arriba son las claves, y las escribe el. Ver
//! `concepts/propagation.md`.
//!
//! **Lo que depende del arbol que lo vio no se copia: se rehace.** Son dos, y
//! por el mismo motivo. El renombre lleva el `git mv` y la reescritura de todo
//! lo que nombraba al slug, y esa reescritura recorre el arbol donde corre: 19
//! archivos en la ventana, 243 en el panorama. Y `normalize:` escribe la forma
//! canonica **traduciendo los links a otros items**, que es leer esos mismos
//! nombres. Copiados tal cual dejarian las referencias de afuera del recorte
//! apuntando a un slug que ya no existe.

use anyhow::{bail, Result};
use std::path::Path;
use worklist_core::git::{
    normalize_subject, redone_above, rename_subject, sprint_subject,
    cherry_pick_one, cut_commit, git_output, rev_parse, try_git, Picked, ALL_ZEROS, PANORAMA,
};

/// Donde el servidor anota hasta donde subio una ventana.
///
/// Vive en `core` porque el recorte tambien la lee: una ventana propagada
/// entera no tiene nada que replantar, y eso lo contesta esta ref y no el
/// patch-id. Se re-exporta desde aca porque es vocabulario de la propagacion.
pub use worklist_core::git::propagated_ref;

/// Si este repo tiene panorama.
///
/// Se pregunta **una vez, antes del lote**: que el panorama falte es una
/// propiedad del repo y no de cada ventana, y repetir el mismo aviso dieciseis
/// veces convierte una linea util en ruido que se scrollea.
pub fn has_panorama(repo: &Path) -> bool {
    rev_parse(repo, PANORAMA).is_some()
}


/// Que se hizo con cada commit de la ventana.
#[derive(Debug)]
pub enum Step {
    /// Se cherry-pickeo tal cual.
    Picked { sha: String, subject: String },
    /// El commit no aportaba nada nuevo: el panorama ya lo tenia.
    Empty { sha: String, subject: String },
    /// El panorama ya lo dice, **escrito de otra manera**: choca en bytes y
    /// coincide en forma canonica, porque arriba guarda la vuelta del
    /// round-trip y el commit guarda lo que se tipeo.
    Superseded { sha: String, subject: String },
    /// Un renombre, **rehecho** sobre el arbol del panorama.
    Renamed { slug: String, key: String, rewritten: usize },
    /// Un renombre que el panorama ya tenia hecho: no queda nada que rehacer.
    AlreadyRenamed { slug: String, key: String },
    /// Una normalizacion, **rehecha** sobre el arbol del panorama.
    Normalized { key: String },
    /// La forma canonica del panorama ya coincidia: no quedo nada que escribir.
    AlreadyNormalized { key: String },
    /// La clave del sprint, **anotada** en la composicion del panorama.
    SprintKeyed { id: String, key: String },
    /// La composicion del panorama ya tenia esa clave.
    AlreadySprintKeyed { id: String, key: String },
}

/// Si lo que la ventana traeria entra al panorama.
#[derive(Debug)]
pub enum Verdict {
    /// El repo no tiene panorama: **no se pudo probar**, que no es lo mismo
    /// que haber probado y estar bien.
    NoPanorama,
    Applies,
    Conflict { sha: String, subject: String, files: Vec<String> },
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
/// **No poder probar no es que entre**, y por eso hay tres respuestas y no
/// dos: un repo sin panorama —que hoy es el de la instalacion— haria que
/// devolver "entra" fuera aceptar en silencio algo que nadie miro. Ver la task
/// `77`.
pub fn would_conflict(repo: &Path, refname: &str, tip: &str) -> Result<Verdict> {
    if tip == ALL_ZEROS {
        return Ok(Verdict::Applies);
    }
    let Some(panorama) = rev_parse(repo, PANORAMA) else { return Ok(Verdict::NoPanorama) };
    let (_, commits) = pending(repo, refname, tip)?;

    let mut sobre = panorama;
    for (sha, subject) in &commits {
        // Los que se rehacen no se prueban: no hay parche que pueda no aplicar.
        if redone_above(subject) {
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
            return Ok(Verdict::Conflict { sha: sha.clone(), subject: subject.clone(), files });
        }
        // El resultado se envuelve en un commit para que el siguiente parche se
        // pruebe sobre el arbol que el anterior dejo, y no sobre el original.
        sobre = git_output(repo, &["commit-tree", &tree, "-p", &sobre, "-m", "probando"])?
            .trim()
            .to_string();
    }
    Ok(Verdict::Applies)
}

/// Sube al panorama lo que la ventana resolvio y el panorama todavia no tiene.
///
/// `None` cuando no hay nada que subir. **Si algo no aplica, el panorama no
/// avanza**: de `all` se corta todo, asi que un marcador de conflicto escrito
/// ahi entra en el proximo recorte de cada ventana. La ventana queda
/// adelantada, que es un estado del que se sale reintentando.
pub fn propagate(repo: &Path, refname: &str, tip: &str, base: &str, dry_run: bool) -> Result<Option<Propagated>> {
    if tip == ALL_ZEROS {
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

    // Donde este el panorama decide donde se escribe. En un bare no esta en
    // ningun lado y va un worktree temporal; en un clon puede estar abierto,
    // y ahi mover la ref por debajo deja ese indice apuntando al arbol
    // anterior — el defecto de `5o`.
    //
    // **Y un bare puede tener el `HEAD` apuntando al panorama sin tener donde
    // escribir**, que es lo que deja `git clone --bare`. Preguntarlo por el
    // `HEAD` solo manda a `git status` a fallar con *"esta operacion debe ser
    // realizada en un arbol de trabajo"*. Es el mismo defecto que `assign` ya
    // tenia arreglado y que aca nunca se aplico: en la instalacion de hoy no
    // se ve porque su bare quedo con el `HEAD` sin nacer, que es una casualidad
    // de como se creo y no una propiedad del diseño.
    let con_arbol = git_output(repo, &["rev-parse", "--is-bare-repository"])
        .map(|s| s.trim() != "true")
        .unwrap_or(true);
    let aca = con_arbol
        && git_output(repo, &["rev-parse", "--symbolic-full-name", "HEAD"])
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
            == PANORAMA;
    if !aca {
        if let Some(wt) = worklist_core::checked_out_at(repo, PANORAMA) {
            bail!(
                "{PANORAMA} esta checkouteada en otro worktree y no se puede mover:\n\
                 \x20 {wt}\n\
                 \n\
                 moverla dejaria ese worktree con el indice del arbol anterior. Corre\n\
                 esto parado ahi, o saca el worktree primero."
            );
        }
    }
    if aca {
        let sucio = git_output(repo, &["status", "--porcelain"])?;
        if !sucio.trim().is_empty() {
            bail!(
                "el panorama tiene cambios sin commitear, y propagar cherry-pickea encima:\n\
                 \x20 {}\n\
                 \n\
                 comitealos o descartalos antes.",
                sucio.lines().take(3).collect::<Vec<_>>().join("\n  ")
            );
        }
    }

    let tmp = if aca {
        repo.to_path_buf()
    } else {
        let tmp = repo.join(format!("../.worklist-propagate-{}", refname.replace('/', "-")));
        let _ = std::fs::remove_dir_all(&tmp);
        git_output(
            repo,
            &["worktree", "add", "--detach", "-q", tmp.to_str().unwrap(), &panorama_old],
        )?;
        tmp
    };

    let result = (|| -> Result<Vec<Step>> {
        let mut steps = Vec::new();
        for (sha, subject) in &commits {
            // El renombre se rehace: su reescritura es del arbol donde corre, y
            // el panorama tiene un arbol mas grande.
            if let Some((slug, key)) = rename_subject(subject) {
                match worklist_core::find_file(&tmp, &slug) {
                    Ok(_) => {
                        let touched = worklist_core::rename_one(&tmp, &slug, &key)?;
                        steps.push(Step::Renamed { slug, key, rewritten: touched.len() });
                    }
                    Err(_) => steps.push(Step::AlreadyRenamed { slug, key }),
                }
                continue;
            }
            // Y la normalizacion tambien, por el mismo motivo: su contenido es
            // la traduccion de los links leida en el arbol de la ventana. Va
            // despues del renombre rehecho, que es el que deja los nombres
            // finales sobre los que esta se recalcula.
            if let Some(key) = normalize_subject(subject) {
                match worklist_core::find_file(&tmp, &key) {
                    Ok((path, _)) => {
                        let text = std::fs::read_to_string(&path)?;
                        let (_, canonical) = worklist_core::body::round_trip(&text, base, &tmp)?;
                        if canonical == text {
                            steps.push(Step::AlreadyNormalized { key });
                        } else {
                            std::fs::write(&path, &canonical)?;
                            worklist_core::commit_all(&tmp, &format!("normalize: {key}"))?;
                            steps.push(Step::Normalized { key });
                        }
                    }
                    Err(_) => steps.push(Step::AlreadyNormalized { key }),
                }
                continue;
            }
            // Y la clave del sprint: un campo, no un parche. La ventana dejo
            // un marcador sin archivo adentro — no tiene `product.yaml` donde
            // escribirlo — y es aca, sobre el arbol del panorama, donde se
            // anota de verdad.
            if let Some((id, key)) = sprint_subject(subject) {
                if worklist_core::product::anotar_key(&tmp, &id, &key)? {
                    worklist_core::commit_all(&tmp, &format!("sprint: {id} -> {key}"))?;
                    steps.push(Step::SprintKeyed { id, key });
                } else {
                    steps.push(Step::AlreadySprintKeyed { id, key });
                }
                continue;
            }
            match cherry_pick_one(&tmp, sha)? {
                Picked::Applied => {
                    steps.push(Step::Picked { sha: sha.clone(), subject: subject.clone() })
                }
                Picked::Empty => {
                    steps.push(Step::Empty { sha: sha.clone(), subject: subject.clone() })
                }
                Picked::Superseded => {
                    steps.push(Step::Superseded { sha: sha.clone(), subject: subject.clone() })
                }
                Picked::Conflict { files, output } => bail!(
                    "el panorama no recibe {} {subject}\n\
                     \x20 choca en: {}\n\
                     \x20 contra: {refname}\n\
                     \n\
                     el panorama no avanzo y la ventana queda adelantada — se reintenta.\n\
                     Lo que el proveedor arbitra no llega hasta aca: si esto choco, dos\n\
                     ventanas escribieron algo que el no ve. Ver `concepts/propagation.md`.\n\
                     \n{}",
                    &sha[..7.min(sha.len())],
                    files.join(", "),
                    output.trim()
                ),
            }
        }
        Ok(steps)
    })();

    let panorama_new = git_output(&tmp, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string());
    if !aca {
        let _ = git_output(repo, &["worktree", "remove", "--force", tmp.to_str().unwrap()]);
    }
    let steps = result?;
    let panorama_new = panorama_new?;

    // El panorama primero, la marca despues: si algo se cae en el medio, la
    // marca sin mover hace que el reintento vuelva a pasar por lo mismo, y no
    // que se saltee lo que no subio.
    //
    // Parado en la rama, los cherry-picks ya la movieron.
    if !aca && panorama_new != panorama_old {
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
