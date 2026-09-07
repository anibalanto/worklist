//! `worklist status`: en que estado esta la vista, con una linea por pregunta.
//! Ver `commands/status.md`.
//!
//! Son cuatro preguntas y **no cuestan lo mismo**: tres se contestan con git y
//! la cuarta es una consulta por item al proveedor. Un chequeo que las mezcle
//! es caro siempre o mentiroso siempre, asi que lo barato corre siempre y lo
//! caro se pide.
//!
//! No escribe nada. Es el unico comando del que se puede decir eso.

use anyhow::{bail, Context, Result};
use std::path::Path;
use worklist_core::git::{git_output, rev_parse, try_git};

/// Que contesto cada pregunta. `Sin` es *"no se pregunto"*, que **no es**
/// *"esta bien"* — dar por bueno lo que nadie miro es el mismo defecto de
/// forma que confundir "no habia trabajo" con "el trabajo no se hizo".
enum Linea {
    Bien(String),
    Atender { dice: String, hacer: String },
    Sin { dice: String, hacer: String },
}

pub fn run(vista: Option<String>, all: bool, verify: bool, exit_code: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let vistas = crate::pull::vistas_pedidas(&cwd, vista, all)?;
    let bare = crate::pull::servidor(&vistas[0].path)?;

    let mut hay_que_atender = false;
    for (i, v) in vistas.iter().enumerate() {
        if i > 0 {
            println!();
        }
        hay_que_atender |= una(v, &bare, verify)?;
    }

    // El retorno informa si se pudo contestar, no la respuesta. Quien encadena
    // pide lo otro; el default lo lee una persona, que no quiere que su shell
    // se ponga roja por un renglon informativo.
    if exit_code && hay_que_atender {
        std::process::exit(1);
    }
    Ok(())
}

/// Las cuatro preguntas sobre una vista. Devuelve si algo necesita atencion.
fn una(v: &crate::pull::View, bare: &Path, verify: bool) -> Result<bool> {
    let branch_ref = format!("refs/heads/{}", v.branch);
    let srv_ref = format!("refs/remotes/srv/{}", v.branch);
    let head = rev_parse(&v.path, "HEAD").context("la vista no tiene HEAD")?;

    // La punta de hoy se lee del bare, que la tiene. No se hace `fetch`:
    // preguntar como esta algo no puede moverlo.
    let Some(tip) = rev_parse(bare, &branch_ref) else {
        bail!("`{}` no existe en el servidor", v.branch);
    };

    // 1 — al dia con el servidor: tengo lo que el servidor resolvio?
    let (tengo_lo_suyo, _) = try_git(&v.path, &["merge-base", "--is-ancestor", &tip, &head])?;
    let servidor = if tengo_lo_suyo {
        Linea::Bien("al dia".into())
    } else {
        let atras = git_output(&v.path, &["rev-list", "--count", &format!("{head}..{tip}")])
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "?".into());
        Linea::Atender { dice: format!("{atras} commit(s) atras"), hacer: "worklist pull".into() }
    };

    // 2 — sin empujar: hay trabajo que no salio de esta maquina?
    //
    // Se pregunta contra `srv/<rama>` y no contra el upstream de la rama,
    // porque las vistas nacen sin upstream —`git worktree add` no lo
    // configura— y por eso `git status` nunca lo dice.
    let base = rev_parse(&v.path, &srv_ref).unwrap_or_else(|| tip.clone());
    let sin_empujar = git_output(&v.path, &["rev-list", "--count", &format!("{base}..{head}")])?
        .trim()
        .parse::<usize>()
        .unwrap_or(0);
    let empuje = if sin_empujar == 0 {
        Linea::Bien("nada".into())
    } else {
        Linea::Atender {
            dice: format!("{sin_empujar} commit(s)"),
            hacer: "worklist push".into(),
        }
    };

    // 3 — local: hay algo sin commitear?
    let sucio = git_output(&v.path, &["status", "--porcelain"])?;
    let sucios = sucio.lines().filter(|l| !l.trim().is_empty()).count();
    let local = if sucios == 0 {
        Linea::Bien("limpio".into())
    } else {
        Linea::Atender { dice: format!("{sucios} archivo(s) sin commitear"), hacer: String::new() }
    };

    // 4 — la cara. No la hace el cliente: no tiene credenciales, asi que se la
    // pide al servidor, que ya tiene el comando que compara sin rechazar.
    let proveedor = if verify {
        preguntarle_al_proveedor(bare, &branch_ref)?
    } else {
        Linea::Sin {
            dice: "sin verificar".into(),
            hacer: "worklist status --verify".into(),
        }
    };

    println!("  vista        {:<22} {}", v.branch, clase(&v.branch));
    for (etiqueta, l) in
        [("servidor", &servidor), ("sin empujar", &empuje), ("local", &local), ("proveedor", &proveedor)]
    {
        match l {
            Linea::Bien(dice) => println!("  {etiqueta:<12} {dice}"),
            Linea::Atender { dice, hacer } | Linea::Sin { dice, hacer } => {
                if hacer.is_empty() {
                    println!("  {etiqueta:<12} {dice}");
                } else {
                    println!("  {etiqueta:<12} {dice:<22} → {hacer}");
                }
            }
        }
    }

    Ok([&servidor, &empuje, &local].iter().any(|l| matches!(l, Linea::Atender { .. })))
}

/// Lo caro, y lo hace el servidor.
fn preguntarle_al_proveedor(bare: &Path, branch_ref: &str) -> Result<Linea> {
    let out = std::process::Command::new("worklist-server")
        .args(["check-push", "--dry-run", "--ref", branch_ref])
        .current_dir(bare)
        .output();
    let Ok(out) = out else {
        return Ok(Linea::Sin {
            dice: "no se pudo preguntar".into(),
            hacer: "el servidor no esta a mano".into(),
        });
    };
    if !out.status.success() {
        return Ok(Linea::Sin {
            dice: "no se pudo preguntar".into(),
            hacer: String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("").to_string(),
        });
    }
    // La ultima linea de `check-push --dry-run` es el resumen por campo, que es
    // el numero que decide. Se muestra tal cual: dos formatos serian dos cosas
    // que mantener de acuerdo.
    let texto = String::from_utf8_lossy(&out.stdout);
    let resumen = texto.lines().rev().find(|l| l.starts_with("resumen:")).unwrap_or("").to_string();
    if resumen.contains("status 0") && resumen.contains("titulo 0") && resumen.contains("cuerpo 0") {
        Ok(Linea::Bien("coincide".into()))
    } else if resumen.is_empty() {
        Ok(Linea::Sin { dice: "no informo".into(), hacer: String::new() })
    } else {
        Ok(Linea::Atender {
            dice: resumen.trim_start_matches("resumen:").trim().to_string(),
            hacer: String::new(),
        })
    }
}

/// Que clase de vista es. Hoy hay una sola, y decirlo es lo que deja lugar a
/// la pregunta que todavia no se contesta: que significa "al dia" para una
/// vista dinamica, que no se empuja.
fn clase(branch: &str) -> &'static str {
    if branch.starts_with("secure/sprint/") {
        "ventana"
    } else {
        "vista"
    }
}
