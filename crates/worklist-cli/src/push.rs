//! `worklist push`: empujar la vista, decir que escribio el servidor encima, y
//! traducir el rechazo. Ver `commands/push.md`.
//!
//! **No aporta garantias. Aporta que el bucle sea corrible.** Las tres
//! invariantes de `pull` se quedan —no hace falta atomicidad, el chequeo previo
//! no puede ser local, prohibir `git push` no compra nada— y lo que se cayo es
//! la conclusion de que por eso no hiciera falta un comando.
//!
//! Medido: las vistas nacen sin upstream, asi que `git push` a secas falla y
//! `git status` nunca dice "ahead by 1". El bucle `push -> pull -> resolver ->
//! push` no arrancaba.

use anyhow::{bail, Result};
use std::path::Path;
use worklist_core::git::{git_output, rev_parse, try_git};

pub fn run(vista: Option<String>, dry_run: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let vistas = crate::pull::vistas_pedidas(&cwd, vista, false)?;
    let v = &vistas[0];
    let bare = crate::pull::servidor(&v.path)?;

    let branch_ref = format!("refs/heads/{}", v.branch);
    let head = rev_parse(&v.path, "HEAD").unwrap_or_default();

    // Si el servidor ya tiene mi punta no hay nada que empujar, y **la
    // pregunta se hace en el bare**: el commit del otro lado puede no existir
    // de este, asi que preguntarlo local devuelve un error y no una respuesta.
    let ya_esta = rev_parse(&bare, &branch_ref).is_some_and(|tip| {
        try_git(&bare, &["merge-base", "--is-ancestor", &head, &tip])
            .map(|(ok, _)| ok)
            .unwrap_or(false)
    });
    if ya_esta {
        // Correrlo de mas no puede castigar: con el bucle corrible va a pasar
        // seguido. Es la misma decision que hizo idempotente a `pull`.
        println!("{}: nada que empujar", v.branch);
        return Ok(());
    }

    // Cuantos son se cuenta con lo que hay **de este lado**: los commits que
    // ninguna ref de `srv` alcanza. Contra la punta del servidor daria `?`, que
    // es lo que pasa cuando el objeto de alla no esta aca.
    let cuantos = git_output(&v.path, &["rev-list", "--count", "HEAD", "--not", "--remotes=srv"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "?".into());

    if dry_run {
        println!("{}: empujaria {cuantos} commit(s) a srv", v.branch);
        return Ok(());
    }

    // El refspec es explicito y **no depende del upstream**, que es justamente
    // lo que la vista no tiene. Configurarlo viene despues, y para la proxima.
    let (ok, salida) =
        try_git(&v.path, &["push", "srv", &format!("HEAD:{branch_ref}")])?;

    println!("{} → srv   {cuantos} commit(s)", v.branch);
    for linea in lo_que_dijo_el_servidor(&salida) {
        println!("  {linea}");
    }

    if !ok {
        // El rechazo tiene dos motivos y hoy hay que deducir cual. Se nombran,
        // y el que cierra el bucle termina diciendo la palabra.
        bail!("{}", rechazo(&salida));
    }

    // Una vez, y para siempre: a partir de aca `git status` dice "ahead by N"
    // sin que haga falta ningun comando del worklist. Lo hace `push` y no
    // `pull` porque el upstream es *contra que se compara lo que tengo para
    // subir*, y esa es la pregunta de este comando — `pull` no lo mira.
    configurar_upstream(&v.path, &v.branch);

    println!("al dia en el servidor; para traer lo que escribio: worklist pull");
    Ok(())
}

/// Las lineas que el hook imprimio, sin el `remote:` que git les pone adelante
/// ni el ruido de progreso.
fn lo_que_dijo_el_servidor(salida: &str) -> Vec<String> {
    salida
        .lines()
        .filter_map(|l| l.strip_prefix("remote:"))
        .map(|l| l.trim_end().trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with("worklist-server "))
        .collect()
}

/// Que hacer con el rechazo, que es lo unico que el `pre-receive` no dice.
///
/// **Son tres y no dos.** El tercero ni siquiera llega al hook: git rechaza el
/// no-fast-forward por su cuenta, y es el mas frecuente de todos justamente
/// porque el servidor commitea encima de cada push que acepta. Es el que cierra
/// el bucle, asi que es el que mas tiene que decir la palabra.
fn rechazo(salida: &str) -> String {
    // Primero los del hook, que son especificos; el no-fast-forward de git al
    // final, porque su `rejected` tambien aparece cuando el que rechaza es el
    // hook y matchearlo antes taparia el motivo real.
    if salida.contains("no entra al panorama") || salida.contains("choca en") {
        "otra ventana escribio eso. Corre `worklist pull`, resolve, y empuja de nuevo".into()
    } else if salida.contains("el proveedor dice") {
        "el item cambio del otro lado. Mira el board: esto no lo arregla un push".into()
    } else if salida.contains("non-fast-forward")
        || salida.contains("fetch first")
        || salida.contains("rejected")
    {
        "el servidor escribio encima de lo que empujaste. Corre `worklist pull` y empuja de nuevo"
            .into()
    } else {
        "el servidor rechazo el push — ver arriba".into()
    }
}

/// Deja la rama comparable con `git status`. Que falle no invalida el push.
fn configurar_upstream(view: &Path, branch: &str) {
    let remota = format!("srv/{branch}");
    if rev_parse(view, &format!("refs/remotes/{remota}")).is_none() {
        return;
    }
    let _ = try_git(view, &["branch", &format!("--set-upstream-to={remota}"), branch]);
}

#[cfg(test)]
mod tests {
    use super::rechazo;

    /// El que cierra el bucle, y el que la primera version despachaba con un
    /// mensaje generico: git lo pone **antes** del hook, asi que no lleva
    /// ninguna de las dos frases del `pre-receive`.
    #[test]
    fn el_no_fast_forward_manda_a_pull() {
        let salida = " ! [rejected]        HEAD -> secure/sprint/21 (non-fast-forward)\n\
                       error: failed to push some refs";
        assert!(rechazo(salida).contains("worklist pull"), "{}", rechazo(salida));
    }

    /// Y el orden importa: el `rejected` de git tambien aparece cuando el que
    /// rechaza es el hook, asi que matchearlo primero taparia el motivo real
    /// con el mas comun.
    #[test]
    fn el_choque_contra_el_panorama_no_lo_tapa_el_rejected_de_git() {
        let salida = "remote: reject: a2b035d edito ACC-14 no entra al panorama — choca en ACC-14.epic.md\n\
                      ! [remote rejected] HEAD -> secure/sprint/21 (pre-receive hook declined)";
        let r = rechazo(salida);
        assert!(r.contains("otra ventana"), "lo tapo el no-fast-forward: {r}");
        assert!(r.contains("worklist pull"), "{r}");
    }

    /// La deriva del proveedor **no** manda a `pull`: bajar no arregla que
    /// alguien haya movido el item del otro lado.
    #[test]
    fn la_deriva_del_proveedor_manda_al_board_y_no_a_pull() {
        let salida = "remote: reject: ACC-101 status era \"open\" en el tip, el proveedor dice \"done\"\n\
                      ! [remote rejected] HEAD -> secure/sprint/21 (pre-receive hook declined)";
        let r = rechazo(salida);
        assert!(r.contains("board"), "{r}");
        assert!(!r.contains("worklist pull"), "mando a pull, que no lo arregla: {r}");
    }
}
