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
use worklist_core::git::{git_output, rev_parse, try_git};

/// Una vista del clon: su worktree y la rama que tiene checkouteada.
pub struct View {
    pub path: PathBuf,
    pub branch: String,
}

/// Como quedo una vista. **`Amedias` no es un error del comando**: con `--all`
/// las demas siguen, porque son operaciones independientes.
enum Outcome {
    AlDia {
        replantados: usize,
        caidos: Vec<String>,
        /// Que dijo el proveedor, o por que no se le pudo preguntar. **Un
        /// `pull` que no pregunto no dice "al dia" a secas**: eso seria
        /// afirmar sobre la mitad que no miro.
        proveedor: Option<Result<String, String>>,
    },
    /// `--dry-run`: se dijo que pasaria, y no se toco nada. **No es "al dia"**
    /// — decirlo seria afirmar algo que esta corrida no hizo.
    Informado,
    Amedias { linea: String, files: Vec<String> },
}

/// Que vistas pidio el que corre el comando, resueltas contra las que hay.
///
/// Lo comparten `pull` y `status`: los dos se paran en una vista o en todas, y
/// que la seleccion sea la misma es lo que hace que uno pueda decir lo que el
/// otro va a hacer.
pub fn vistas_pedidas(cwd: &Path, vista: Option<String>, all: bool) -> Result<Vec<View>> {
    let todas = views(cwd)?;
    let objetivo: Vec<View> = if all {
        todas
    } else {
        let quiero = match vista {
            Some(v) => normalizar(&v),
            None => rama_actual(cwd)?,
        };
        match todas.into_iter().find(|v| v.branch == quiero) {
            Some(v) => vec![v],
            None => bail!("no encuentro un worktree con la rama `{quiero}` en este clon"),
        }
    };
    // Parado afuera del clon del worklist no hay ninguna vista, y eso es lo
    // que mas pasa: la raiz del proyecto y las capas impl son otros repos.
    // Decirlo es mejor que buscar en cero.
    if objetivo.is_empty() {
        bail!(
            "no hay ninguna vista del worklist desde aca: `{}` no es el clon del worklist.\n\
             Las vistas cuelgan de `.worklist/secure/**`.",
            cwd.display()
        );
    }
    Ok(objetivo)
}

pub fn run(vista: Option<String>, all: bool, dry_run: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let objetivo = vistas_pedidas(&cwd, vista, all)?;
    let bare = servidor(&objetivo[0].path)?;
    let mut amedias: Vec<(String, String, Vec<String>)> = Vec::new();

    for v in &objetivo {
        match una(v, &bare, dry_run) {
            Ok(Outcome::AlDia { replantados, caidos, proveedor }) => {
                if all {
                    println!("{}: {}", v.branch, cierre(&proveedor));
                } else {
                    reportar(v, replantados, &caidos, &proveedor);
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

/// Como cierra una vista. **Nunca "al dia" a secas si no se pregunto al
/// proveedor**: la palabra abarca las dos mitades, y decirla habiendo mirado
/// una es el mismo defecto que `sin verificar` contra `coincide`.
fn cierre(proveedor: &Option<Result<String, String>>) -> String {
    match proveedor {
        Some(Ok(resumen)) if resumen.starts_with("0 absorbido") => "al dia".into(),
        Some(Ok(resumen)) => format!("al dia con git; el proveedor: {resumen}"),
        Some(Err(porque)) => format!("al dia con git; al proveedor no se le pudo preguntar ({porque})"),
        None => "al dia con git; el proveedor no se pregunto".into(),
    }
}

fn reportar(v: &View, replantados: usize, caidos: &[String], proveedor: &Option<Result<String, String>>) {
    if replantados > 0 {
        println!("  {replantados} commit(s) replantado(s)");
    }
    // Cada uno dice por que se dejo caer: son dos motivos distintos, y la
    // diferencia entre un descarte y una perdida esta justo ahi.
    for linea in caidos {
        println!("  dejado caer: {linea}");
    }
    println!("{}: {}", v.branch, cierre(proveedor));
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
    // Si el servidor ya tiene **todo lo mio** no hay nada que replantar. La
    // pregunta la contesta `core`, que es donde se puede probar contra un
    // servidor de mentira — y no es *"mi punta es la marca"*: el servidor
    // commitea encima de lo que recibe, asi que despues de un push nunca son lo
    // mismo.
    let todo_subido = worklist_core::window::propagado_entero(bare, &branch_ref, &antes);
    if dry_run {
        // No se le pide el corte al servidor y **no se hace `fetch`**: la
        // punta de hoy se lee del bare, que la tiene. Un dry-run que mueve
        // refs no es un dry-run.
        let Some(tip) = rev_parse(bare, &branch_ref) else {
            bail!("`{}` no existe en el servidor", v.branch);
        };
        let sin_empujar = if todo_subido {
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

    // Paso 0 — lo que cambio en el board, adentro de la ventana. **No lo hace
    // el cliente**: `absorb` es del servidor, escribe en la rama, y de ahi baja
    // como cualquier otra cosa que el servidor haya escrito. Es lo que vuelve
    // cierta la palabra "al dia" — sin esto, `pull` pone al dia la mitad de lo
    // que puede estar viejo y lo dice igual.
    let proveedor = absorber(bare, &branch_ref);

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
        return Ok(Outcome::AlDia { replantados: 0, caidos: Vec::new(), proveedor: None });
    }
    // Nada sin empujar: el corte nuevo trae todo mi trabajo, y replantarlo
    // seria pedirle a git que redescubra por patch-id algo que ya se sabe. Y
    // no lo puede contestar: arriba el cuerpo quedo en su forma canonica.
    if todo_subido {
        git_output(&v.path, &["reset", "--hard", "--quiet", &tip])?;
        return Ok(Outcome::AlDia { replantados: 0, caidos: Vec::new(), proveedor: None });
    }

    // El replante lo hace `core`, que es donde se puede probar contra un
    // servidor de mentira. Lo mio es lo que la punta del servidor no tiene, y
    // eso es el rango y nada mas — **el corte no se busca**: se busca contra el
    // panorama, que de este lado no esta, y cuando el recorte es idempotente la
    // punta del servidor *es* el corte.
    let r = worklist_core::window::replantar(&v.path, &tip, &antes)?;
    if let Some((linea, files)) = r.conflicto {
        return Ok(Outcome::Amedias { linea, files });
    }
    let (replantados, caidos) = (r.replantados, r.caidos);
    Ok(Outcome::AlDia { replantados, caidos, proveedor: Some(proveedor) })
}

/// El paso 0: lo que cambio en el board, adentro de la ventana.
///
/// **Que no se pueda preguntar no frena el resto.** Poner al dia lo de git vale
/// igual, y quedarse sin hacerlo porque el proveedor no contesto seria cambiar
/// una respuesta incompleta por ninguna. Se devuelve lo que paso para que la
/// salida lo diga — un `pull` que no pregunto **no dice "al dia" a secas**.
fn absorber(bare: &Path, branch_ref: &str) -> Result<String, String> {
    let Some(config) = crate::status::args_del_hook(bare) else {
        return Err("el servidor no tiene hooks instalados".into());
    };
    let out = std::process::Command::new("worklist-server")
        .arg("absorb")
        .args(["--ref", branch_ref])
        .args(&config)
        .current_dir(bare)
        .output()
        .map_err(|_| "no encuentro `worklist-server`".to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("").to_string());
    }
    let texto = String::from_utf8_lossy(&out.stdout);
    Ok(texto
        .lines()
        .rev()
        .find(|l| l.starts_with("resumen:"))
        .unwrap_or("")
        .trim_start_matches("resumen:")
        .trim()
        .to_string())
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
pub fn servidor(view: &Path) -> Result<PathBuf> {
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
