//! `worklist pull`: traer el corte de hoy y replantar encima lo que no se
//! empujo. Ver `commands/pull.md`.
//!
//! Son tres pasos y ningun mecanismo nuevo —el corte, el `fetch`, el replante—
//! y lo que no existia era que fueran uno. **El paso 1 es el que mas se
//! olvida**: sin el se baja el corte de la ultima vez que alguien recorto, y la
//! ventana queda al dia contra una foto vieja del panorama, que se ve identica
//! a estarlo de verdad.
//!
//! Los dos primeros pasos tienen dueños distintos: calcular el corte es del
//! servidor —un recorte nuevo puede traer items que el cliente no tiene, asi
//! que se calcula donde esta el tronco— y replantar lo mio encima es del
//! cliente, porque son los commits que el servidor no tiene.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use worklist_core::git::{cherry_pick_one, git_output, rev_parse, try_git, Picked};

/// Una vista del clon: su worktree y la rama que tiene checkouteada.
pub struct View {
    pub path: PathBuf,
    pub branch: String,
}

/// Como quedo una vista. **`Amedias` no es un error del comando**: con `--all`
/// las demas siguen, porque son operaciones independientes.
enum Outcome {
    AlDia { replantados: usize, caidos: Vec<String> },
    /// `--dry-run`: se dijo que pasaria, y no se toco nada. **No es "al dia"**
    /// — decirlo seria afirmar algo que esta corrida no hizo.
    Informado,
    Amedias { linea: String, files: Vec<String> },
}

pub fn run(vista: Option<String>, all: bool, dry_run: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let todas = views(&cwd)?;

    let objetivo: Vec<View> = if all {
        todas
    } else {
        let quiero = match vista {
            Some(v) => normalizar(&v),
            None => rama_actual(&cwd)?,
        };
        match todas.into_iter().find(|v| v.branch == quiero) {
            Some(v) => vec![v],
            None => bail!("no encuentro un worktree con la rama `{quiero}` en este clon"),
        }
    };

    // Parado afuera del clon del worklist no hay ninguna vista, y eso es lo
    // que mas pasa: la raiz del proyecto y las capas impl son otros repos.
    // Decirlo es mejor que buscar en cero.
    let Some(primera) = objetivo.first() else {
        bail!(
            "no hay ninguna vista del worklist desde aca: `{}` no es el clon del worklist.\n\
             Las vistas cuelgan de `.worklist/secure/**`.",
            cwd.display()
        );
    };
    let bare = servidor(&primera.path)?;
    let mut amedias: Vec<(String, String, Vec<String>)> = Vec::new();

    for v in &objetivo {
        match una(v, &bare, dry_run) {
            Ok(Outcome::AlDia { replantados, caidos }) => {
                if all {
                    println!("{}: al dia", v.branch);
                } else {
                    reportar(v, replantados, &caidos);
                }
            }
            Ok(Outcome::Informado) => {}
            Ok(Outcome::Amedias { linea, files }) => {
                amedias.push((v.branch.clone(), linea, files));
            }
            // Que una vista no se pueda poner al dia no dice nada de las
            // demas. Se anota y se sigue.
            Err(e) => amedias.push((v.branch.clone(), format!("{e}"), Vec::new())),
        }
    }

    if amedias.is_empty() {
        if all {
            // Un `--dry-run` no dejo ninguna al dia: dijo lo que pasaria. La
            // linea final es la que se lee, asi que es la que menos puede
            // afirmar de mas.
            let cierre = if dry_run { "sin tocar nada" } else { "todas al dia" };
            println!("{} vistas, {cierre}", objetivo.len());
        }
        return Ok(());
    }

    // El modo de falla de esto es que no se note, asi que la ultima linea
    // cuenta cuantas quedaron a medias y cada una dice por que.
    println!(
        "\n{} vista(s), {} al dia, {} a medias:",
        objetivo.len(),
        objetivo.len() - amedias.len(),
        amedias.len()
    );
    for (branch, linea, files) in &amedias {
        if files.is_empty() {
            println!("  {branch}: {linea}");
        } else {
            println!("  {branch}: conflicto al replantar {linea} — {}", files.join(", "));
            println!("    el trabajo toca un item que el `items` de hoy ya no lleva.");
        }
    }
    std::process::exit(1);
}

fn reportar(v: &View, replantados: usize, caidos: &[String]) {
    if replantados > 0 {
        println!("  {replantados} commit(s) replantado(s)");
    }
    // Cada uno dice por que se dejo caer: son dos motivos distintos, y la
    // diferencia entre un descarte y una perdida esta justo ahi.
    for linea in caidos {
        println!("  dejado caer: {linea}");
    }
    println!("{}: al dia", v.branch);
}

/// Los tres pasos, sobre una vista.
fn una(v: &View, bare: &Path, dry_run: bool) -> Result<Outcome> {
    let srv_ref = format!("refs/remotes/srv/{}", v.branch);
    let branch_ref = format!("refs/heads/{}", v.branch);
    let antes = rev_parse(&v.path, "HEAD").context("la vista no tiene HEAD")?;
    // Hasta donde subio esta vista, **preguntado en el servidor**, que es quien
    // lo anota. Si mi HEAD es exactamente eso, no hay ni un commit sin
    // empujar, y el corte nuevo trae todo mi trabajo por construccion.
    //
    // No se usa `refs/remotes/srv/**` para contestarlo, aunque este a mano: esa
    // ref dice *lo ultimo que traje*, que es otra cosa, y **la mueve cualquier
    // `fetch`** — incluido el de un `--dry-run`. Una fuente que la propia
    // consulta desplaza no puede contestar esta pregunta.
    let subido = rev_parse(bare, &worklist_core::git::propagated_ref(&branch_ref));

    if dry_run {
        // No se le pide el corte al servidor y **no se hace `fetch`**: la
        // punta de hoy se lee del bare, que la tiene. Un dry-run que mueve
        // refs no es un dry-run.
        let Some(tip) = rev_parse(bare, &branch_ref) else {
            bail!("`{}` no existe en el servidor", v.branch);
        };
        let sin_empujar = if subido.as_deref() == Some(antes.as_str()) {
            0
        } else {
            git_output(&v.path, &["rev-list", "--count", &format!("{}..{antes}", subido.unwrap_or_else(|| antes.clone()))])?
                .trim()
                .parse()
                .unwrap_or(0)
        };
        println!(
            "{}: {sin_empujar} commit(s) sin empujar; la punta de hoy es {} y el corte se recalcularia",
            v.branch,
            &tip[..7.min(tip.len())]
        );
        return Ok(Outcome::Informado);
    }

    // Paso 1 — el corte de hoy, donde esta el panorama. Es del servidor, y no
    // hace falta ningun canal porque el bare esta en la misma maquina. El dia
    // que sea un GitLab, esto es lo primero que se rompe.
    if let Some(id) = v.branch.strip_prefix("secure/sprint/") {
        recortar(bare, id)?;
    }

    // Paso 2 — baja.
    let (ok, salida) = try_git(&v.path, &["fetch", "srv", "--quiet"])?;
    if !ok {
        bail!("no se pudo traer del servidor:\n{}", salida.trim());
    }
    let Some(tip) = rev_parse(&v.path, &srv_ref) else {
        bail!("`{srv_ref}` no existe: esta vista no esta en el servidor");
    };

    // Paso 3 — replantar lo que no se empujo. Reemplaza el arbol de la vista,
    // asi que lo que no esta commiteado se pierde: se niega en vez de decidir
    // por el que lo corre.
    let sucio = git_output(&v.path, &["status", "--porcelain"])?;
    if !sucio.trim().is_empty() {
        bail!(
            "la vista tiene cambios sin commitear y replantar los pisaria:\n{}",
            sucio.trim()
        );
    }
    // Estar **adelantado** no es estar divergido. Si mi HEAD ya contiene la
    // punta del servidor, mi trabajo ya esta donde tiene que estar y no hay
    // nada que replantar — cherry-pickearlo sobre su propio padre lo dejaria
    // igual con otro sha, que es la misma reescritura por nada que el recorte
    // dejo de hacer.
    let (contiene, _) =
        try_git(&v.path, &["merge-base", "--is-ancestor", &tip, &antes])?;
    if contiene {
        return Ok(Outcome::AlDia { replantados: 0, caidos: Vec::new() });
    }
    // Nada sin empujar: el corte nuevo trae todo mi trabajo, y replantarlo
    // seria pedirle a git que redescubra por patch-id algo que ya se sabe. Y
    // no lo puede contestar: arriba el cuerpo quedo en su forma canonica.
    if subido.as_deref() == Some(antes.as_str()) {
        git_output(&v.path, &["reset", "--hard", "--quiet", &tip])?;
        return Ok(Outcome::AlDia { replantados: 0, caidos: Vec::new() });
    }

    // Lo mio es lo que la punta del servidor no tiene, y eso es el rango y
    // nada mas. **No se busca el corte para esto**: el corte se busca contra el
    // panorama, que de este lado no esta — y cuando el recorte es idempotente
    // la punta del servidor *es* el corte, con lo que buscarlo ahi encuentra mi
    // propio trabajo y no un `window:`.
    let mios = git_output(&v.path, &["log", "--oneline", &format!("{tip}..{antes}")])?;
    let shas = git_output(&v.path, &["rev-list", "--reverse", &format!("{tip}..{antes}")])?;

    git_output(&v.path, &["reset", "--hard", "--quiet", &tip])?;

    let mut replantados = 0;
    let mut caidos = Vec::new();
    for sha in shas.lines() {
        let subject = git_output(&v.path, &["log", "-1", "--format=%s", sha])?.trim().to_string();
        // El corte **nunca** se replanta: es un commit que borra lo que el
        // recorte dejo afuera, y re-aplicarlo sobre un panorama que crecio
        // no menciona lo nuevo, asi que lo nuevo entra. El de hoy ya vino en
        // el `fetch`; el mio, si el servidor recorto distinto, se cae aca.
        if subject.starts_with("window: ") {
            continue;
        }
        let linea = || {
            mios.lines()
                .find(|l| l.starts_with(&sha[..7.min(sha.len())]))
                .unwrap_or(sha)
                .to_string()
        };
        match cherry_pick_one(&v.path, sha)? {
            Picked::Applied => replantados += 1,
            // Ya subio: el corte nuevo lo contiene y el cherry-pick no aporta.
            Picked::Empty => {}
            // Ya subio **escrito de otra manera**: el panorama guarda la vuelta
            // del round-trip y este commit guarda lo que se tipeo. No hay dos
            // versiones que reconciliar — hay una superada y una vigente.
            Picked::Superseded => {
                caidos.push(format!("{} (ya esta en el corte modulo normalizacion)", linea()))
            }
            // Gana el corte: la planificacion —`status` e `items`— se edita en
            // el panorama y baja regenerando, asi que este commit choca contra
            // un `items` que ya quedo viejo y no proponia nada sobre el.
            Picked::Conflict { ref files, .. } if worklist_core::git::planning_only(files) => {
                caidos.push(format!("{} (la planificacion del sprint es del panorama)", linea()))
            }
            // El renombre, la normalizacion y la clave del sprint se rehacen
            // arriba: si chocan, el corte ya los trae hechos.
            Picked::Conflict { .. } if worklist_core::git::redone_above(&subject) => {
                caidos.push(format!("{} (el corte ya lo trae rehecho)", linea()))
            }
            Picked::Conflict { files, .. } => {
                // La vista vuelve a como estaba: quedar a medias adentro de un
                // cherry-pick abortado es peor que no haber empezado.
                git_output(&v.path, &["reset", "--hard", "--quiet", &antes])?;
                return Ok(Outcome::Amedias { linea: linea(), files });
            }
        }
    }
    Ok(Outcome::AlDia { replantados, caidos })
}

/// El paso 1: se lo pide al servidor, que es de quien es.
fn recortar(bare: &Path, sprint_id: &str) -> Result<()> {
    let out = std::process::Command::new("worklist-server")
        .args(["window", "open", sprint_id])
        .current_dir(bare)
        .output()
        .context(
            "no encuentro `worklist-server`: el paso 1 supone que el servidor esta a mano, \
             y hoy eso es que el bare este en esta maquina",
        )?;
    if !out.status.success() {
        bail!(
            "el servidor no pudo recortar la ventana:\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// El bare del que cuelga este clon. Hoy es un path local; el dia que sea una
/// URL, el paso 1 se queda sin como pedir — ver `concepts/distribution.md`.
fn servidor(view: &Path) -> Result<PathBuf> {
    let url = git_output(view, &["remote", "get-url", "srv"])
        .context("este clon no tiene un remoto `srv`")?
        .trim()
        .to_string();
    let path = PathBuf::from(&url);
    if !path.is_dir() {
        bail!(
            "el remoto `srv` es `{url}`, que no es un directorio de esta maquina: \
             el paso 1 necesita el canal cliente->servidor que todavia no existe"
        );
    }
    Ok(path)
}

/// Las vistas del clon: los worktrees con una rama `secure/**` checkouteada.
///
/// El panorama no aparece y no puede aparecer: no esta de este lado.
fn views(cwd: &Path) -> Result<Vec<View>> {
    let listing = git_output(cwd, &["worktree", "list", "--porcelain"])?;
    let mut out = Vec::new();
    let mut path: Option<PathBuf> = None;
    for line in listing.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            if b.starts_with("secure/") {
                if let Some(p) = path.take() {
                    out.push(View { path: p, branch: b.to_string() });
                }
            }
        }
    }
    out.sort_by(|a, b| a.branch.cmp(&b.branch));
    Ok(out)
}

fn rama_actual(cwd: &Path) -> Result<String> {
    let b = git_output(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string();
    if !b.starts_with("secure/") {
        bail!("`{b}` no es una vista del worklist: parado ahi no hay nada que poner al dia");
    }
    Ok(b)
}

/// `21`, `sprint/21` y `secure/sprint/21` nombran la misma vista.
fn normalizar(v: &str) -> String {
    if v.starts_with("secure/") {
        v.to_string()
    } else if v.chars().all(|c| c.is_ascii_alphanumeric()) {
        format!("secure/sprint/{v}")
    } else {
        format!("secure/{v}")
    }
}
