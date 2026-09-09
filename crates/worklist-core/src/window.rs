//! El recorte de una ventana: qué archivos lleva, y producir la rama.
//!
//! Una rama segura se hace responsable de poder verificarse entera contra el
//! proveedor, y **por eso** no puede tener todos los items: la responsabilidad
//! es lo que la acota. Ver `concepts/sync.md`.

use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

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

/// Un item del arbol: su id, su archivo, y de quien cuelga.
struct Item {
    file: String,
    parent: Option<String>,
}

fn read_items(repo: &Path, rev: &str) -> Result<HashMap<String, Item>> {
    let listing = git_output(repo, &["ls-tree", "-r", "--name-only", rev])?;
    let mut items = HashMap::new();
    for name in listing.lines() {
        if name.starts_with("_sprints/") || !name.ends_with(".md") {
            continue;
        }
        let Some(id) = crate::TYPES
            .iter()
            .find_map(|t| name.strip_suffix(&format!(".{t}.md")))
        else {
            continue;
        };
        if id.contains('/') {
            continue;
        }
        let text = git_output(repo, &["show", &format!("{rev}:{name}")])?;
        let parent = regex::Regex::new(r"(?m)^parent:\s*(\S+)")
            .unwrap()
            .captures(&text)
            .map(|c| c[1].to_string());
        items.insert(id.to_string(), Item { file: name.to_string(), parent });
    }
    Ok(items)
}


/// Los archivos que lleva la ventana del sprint `sprint_id` en `rev`.
///
/// El `.sprint.md`, los items que declara **con todo su subarbol**, los
/// ancestros de cada uno —en la practica la epica, que viaja de solo lectura
/// para que la cadena `parent` cierre adentro— y el vocabulario de estados,
/// que viaja por lo mismo: el cliente no tiene el panorama de donde leerlo.
pub fn window_files(repo: &Path, rev: &str, sprint_id: &str) -> Result<Vec<String>> {
    // El `items` sale de la composicion, que vive en el panorama de donde se
    // corta. **No entra a la ventana**: es del servidor, y una copia del lado
    // del cliente es una fuente de verdad que solo puede quedarse vieja — el
    // mismo motivo por el que el panorama tampoco baja.
    let producto = crate::product::leer(repo, rev)?;
    let Some(sprint) = producto.sprint(sprint_id) else {
        bail!("la composicion de {rev} no tiene el sprint `{sprint_id}`");
    };
    let declared = sprint.items.clone();
    let items = read_items(repo, rev)?;

    // hijos: se calculan, no se mantienen
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, it) in &items {
        if let Some(p) = &it.parent {
            children.entry(p.as_str()).or_default().push(id.as_str());
        }
    }

    let mut keep: HashSet<String> = HashSet::new();
    let mut pending: Vec<String> = Vec::new();

    for id in &declared {
        if !items.contains_key(id) {
            bail!("la composicion nombra a `{id}` en el sprint {sprint_id}, y no esta en {rev}");
        }
        pending.push(id.clone());
    }

    // subarbol: los hijos van con el padre
    while let Some(id) = pending.pop() {
        if !keep.insert(id.clone()) {
            continue;
        }
        for c in children.get(id.as_str()).into_iter().flatten() {
            pending.push(c.to_string());
        }
    }

    // ancestros, de solo lectura: la cadena `parent` tiene que cerrar
    let members: Vec<String> = keep.iter().cloned().collect();
    for id in members {
        let mut cur = items[&id].parent.clone();
        while let Some(p) = cur {
            if !items.contains_key(&p) || !keep.insert(p.clone()) {
                break;
            }
            cur = items[&p].parent.clone();
        }
    }

    let mut files: Vec<String> = keep.iter().map(|id| items[id].file.clone()).collect();
    // El `.sprint.md` **todavia viaja**, y ya no es de donde sale el `items`
    // — ni aca, ni en `resolve_sprint`, que tambien paso a leer la
    // composicion. Lo que le queda es el nombre y el `key`: mientras las
    // pasadas de sprint sigan anotando ahi, el archivo se va con ellas — es lo
    // que falta de la segunda mitad de `ACC-305`. Ver `concepts/composition.md`.
    let sprint_file = format!("_sprints/{sprint_id}.sprint.md");
    if git_output(repo, &["cat-file", "-e", &format!("{rev}:{sprint_file}")]).is_ok() {
        files.push(sprint_file);
    }
    // El vocabulario, si el proyecto lo declara: sin el, `state change` parado
    // en la ventana cae al vocabulario por defecto y rechaza un estado que el
    // proyecto si declara. Ver `concepts/states.md`.
    if git_output(repo, &["cat-file", "-e", &format!("{rev}:{}", crate::states::ARCHIVO)]).is_ok() {
        files.push(crate::states::ARCHIVO.to_string());
    }
    files.sort();
    Ok(files)
}

/// Produce la rama de la ventana con esos archivos y nada mas.
/// Los commits que tiene `branch` y no tiene `head`: lo que un corte nuevo
/// descartaria.
///
/// Vacio quiere decir que la rama no existe, o que su punta ya esta contenida
/// en el corte nuevo. Cualquier otra cosa es trabajo que se perderia.
fn would_discard(repo: &Path, branch: &str, head: &str) -> Vec<String> {
    if git_output(repo, &["rev-parse", "--verify", "--quiet", branch]).is_err() {
        return Vec::new();
    }
    git_output(repo, &["log", "--oneline", &format!("{head}..{branch}")])
        .map(|o| o.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default()
}

/// El corte que la ventana tiene hoy, si tiene uno.
fn cut_actual(repo: &Path, branch: &str, from: &str) -> Option<String> {
    if git_output(repo, &["rev-parse", "--verify", "--quiet", branch]).is_err() {
        return None;
    }
    crate::git::cut_commit(repo, branch, from).ok()
}

/// El arbol de un commit: lo que dice, sin cuando ni sobre que se dijo.
fn tree_of(repo: &Path, commit: &str) -> Option<String> {
    git_output(repo, &["rev-parse", &format!("{commit}^{{tree}}")]).ok().map(|s| s.trim().to_string())
}

/// Lo que la ventana tiene **encima de su corte**: su trabajo, sin el recorte.
fn work_above_cut(repo: &Path, branch: &str, from: &str) -> Result<Option<(String, Vec<String>)>> {
    if git_output(repo, &["rev-parse", "--verify", "--quiet", branch]).is_err() {
        return Ok(None);
    }
    let cut = crate::git::cut_commit(repo, branch, from)?;
    let work = git_output(repo, &["log", "--oneline", &format!("{cut}..{branch}")])?;
    Ok(Some((cut, work.lines().map(|l| l.to_string()).collect())))
}

pub fn open(
    repo: &Path,
    sprint_id: &str,
    from: &str,
    dry_run: bool,
    force: bool,
) -> Result<(Vec<String>, String)> {
    let files = window_files(repo, from, sprint_id)?;
    if dry_run {
        return Ok((files, String::new()));
    }

    let branch = format!("refs/heads/secure/sprint/{sprint_id}");

    // Antes de escribir nada: mover una rama con worktree activo deja el indice
    // desincronizado, y el sintoma no dice la causa — archivos "modificados"
    // que nadie toco. Ni con `--force`: forzar autoriza a descartar commits a
    // sabiendas, no a dejar un checkout inconsistente.
    if let Some(wt) = crate::checked_out_at(repo, &branch) {
        bail!(
            "secure/sprint/{sprint_id} esta checkouteada en un worktree y no se puede mover:\n\
             \x20 {wt}\n\
             \n\
             moverla dejaria ese worktree con el indice del arbol anterior. Saca el\n\
             worktree primero."
        );
    }
    // **El corte es derivado, asi que recortar de nuevo sobre lo mismo no
    // produce nada.** Y no es una optimizacion: recortar escribe un commit, y
    // un commit lleva la hora adentro del hash, asi que hacerlo igual
    // reescribe la rama para decir lo que ya decia. Todo el que la tenga
    // clonada queda sin poder fast-forwardear, por nada.
    //
    // Se pregunta en dos pasos, del barato al exacto. El barato: si el
    // panorama no se movio desde que se hizo el corte, no hay de donde salga
    // una diferencia — ni el `items`, que vive arriba.
    let corte_hoy = if force { None } else { cut_actual(repo, &branch, from) };
    if let Some(ref corte) = corte_hoy {
        let padre = git_output(repo, &["rev-parse", &format!("{corte}^")]).ok();
        let panorama = git_output(repo, &["rev-parse", from]).ok();
        if padre.is_some() && padre == panorama {
            let head = git_output(repo, &["rev-parse", &branch])?.trim().to_string();
            return Ok((files, head));
        }
    }

    // Lo que la ventana tiene encima de su corte: **eso se replanta**, no se
    // descarta. El corte viejo no: es un commit que borra una lista fija de
    // rutas, y re-aplicarlo sobre un panorama que crecio no menciona lo nuevo,
    // asi que lo nuevo entra. Ver `concepts/propagation.md`.
    //
    // Y si la ventana esta propagada entera, el corte nuevo la contiene **por
    // construccion**: sale del panorama, que ya tiene todo su trabajo.
    // Preguntarselo commit por commit es pedirle a git que redescubra por
    // patch-id algo que el servidor ya tiene anotado en una ref — y el
    // patch-id no lo puede contestar, porque arriba el cuerpo quedo guardado
    // en su forma canonica y abajo como se tipeo. Es el mismo trato que la ref
    // ya recibe subiendo: un dato que alguien sostiene se guarda, no se busca.
    let propagada = crate::git::rev_parse(repo, &crate::git::propagated_ref(&branch))
        .is_some_and(|subido| crate::git::rev_parse(repo, &branch).as_deref() == Some(&subido));
    let previo =
        if force || propagada { None } else { work_above_cut(repo, &branch, from)? };

    let tmp = repo.join(format!("../.worklist-window-{sprint_id}"));
    let _ = std::fs::remove_dir_all(&tmp);
    git_output(repo, &["worktree", "add", "--detach", "-q", tmp.to_str().unwrap(), from])?;

    // El corte queda afuera del cierre para poder anotarlo despues: es el
    // punto hasta el cual la rama nueva esta propagada por construccion.
    let mut corte_nuevo = String::new();
    // Y si el recorte nuevo dice lo mismo que el que ya esta, la rama no se
    // toca: se devuelve su punta tal cual.
    let mut sin_cambios = false;
    let result = (|| -> Result<String> {
        let keep: HashSet<&str> = files.iter().map(|s| s.as_str()).collect();
        let listing = git_output(&tmp, &["ls-files"])?;
        for name in listing.lines() {
            if !keep.contains(name) {
                git_output(&tmp, &["rm", "-q", "--cached", name])?;
                let _ = std::fs::remove_file(tmp.join(name));
            }
        }
        crate::commit_all(&tmp, &format!("window: sprint/{sprint_id} recortado desde {from}"))?;

        // El paso exacto: el panorama se movio, pero puede no haberse movido
        // **para esta ventana** —cualquier push a cualquiera de las otras lo
        // adelanta—, y ahi el corte nuevo dice exactamente lo mismo. Se
        // comparan los arboles, que es lo que el corte significa; el sha no
        // sirve porque lleva la hora y el padre adentro.
        if let (Some(corte), false) = (corte_hoy.as_deref(), force) {
            let nuevo = git_output(&tmp, &["rev-parse", "HEAD^{tree}"])?.trim().to_string();
            if tree_of(repo, corte).as_deref() == Some(nuevo.as_str()) {
                sin_cambios = true;
                return Ok(git_output(repo, &["rev-parse", &branch])?.trim().to_string());
            }
        }
        let corte = git_output(&tmp, &["rev-parse", "HEAD"])?.trim().to_string();
        corte_nuevo = corte.clone();

        // El replante. El caso sano —la ventana propagada entera— ni siquiera
        // llega aca: lo contesta la ref de arriba. Esto es para la ventana que
        // subio a medias, y ahi lo ya propagado se cae de a uno.
        let Some((cut, work)) = previo else { return Ok(corte) };
        if work.is_empty() {
            return Ok(corte);
        }
        // De a un commit y no con `rebase --onto`: el descarte por patch-id de
        // `rebase` compara contra el corte **viejo**, y lo que ya subio vive en
        // el panorama, que es ancestro del corte **nuevo**. El cherry-pick
        // pregunta contra el arbol que tiene delante, que es lo que importa.
        let shas = git_output(repo, &["rev-list", "--reverse", &format!("{cut}..{branch}")])?;
        for sha in shas.lines() {
            let subject = git_output(&tmp, &["log", "-1", "--format=%s", sha])?.trim().to_string();
            let corto = &sha[..7.min(sha.len())];
            match crate::git::cherry_pick_one(&tmp, sha)? {
                crate::git::Picked::Applied | crate::git::Picked::Empty => {}
                // Ya esta en el corte, escrito de otra manera. Se deja caer y
                // se dice: es la diferencia entre un descarte y una perdida.
                crate::git::Picked::Superseded => eprintln!(
                    "  dejado caer: {corto} — ya esta en el corte modulo normalizacion"
                ),
                // Y si lo unico que choca es el `.sprint.md`, gana el corte:
                // la planificacion se edita arriba y baja regenerando.
                crate::git::Picked::Conflict { ref files, .. }
                    if crate::git::planning_only(files) =>
                {
                    eprintln!("  dejado caer: {corto} — la planificacion del sprint es del panorama")
                }
                // Y los que el servidor rehace arriba: si chocan es porque el
                // corte ya los trae hechos. Que la pregunta llegue recien
                // despues del choque es lo que protege a la ventana adelantada
                // —si el panorama no los tuviera, el replante aplica limpio.
                crate::git::Picked::Conflict { .. } if crate::git::redone_above(&subject) => {
                    eprintln!("  dejado caer: {corto} — el corte ya lo trae rehecho")
                }
                crate::git::Picked::Conflict { files, output } => bail!(
                    "el corte nuevo esta, pero un commit de la ventana no se pudo replantar:\n\
                     \x20 {linea}\n\
                     \x20 choca en: {files}\n\
                     \n\
                     pasa cuando el trabajo toca un item que el `items` de hoy ya no lleva.\n\
                     Para tirarlo a sabiendas: --force\n\
                     \n{output}",
                    linea = work
                        .iter()
                        .find(|l| l.starts_with(&sha[..7.min(sha.len())]))
                        .cloned()
                        .unwrap_or_else(|| sha.to_string()),
                    files = files.join(", "),
                    output = output.trim(),
                ),
            }
        }
        Ok(git_output(&tmp, &["rev-parse", "HEAD"])?.trim().to_string())
    })();

    let _ = git_output(repo, &["worktree", "remove", "--force", tmp.to_str().unwrap()]);
    let head = result?;
    if sin_cambios {
        // Nada que escribir: ni la rama, ni la marca. Recortar contesto que no
        // habia nada que recortar, que es una respuesta y no un trabajo.
        return Ok((files, head));
    }

    // Con `--force` el replante no corrio: lo que la ventana tenia encima se
    // tira, y se dice cuanto. Es una decision explicita, no un efecto.
    if force {
        let descartado = would_discard(repo, &branch, &head);
        if !descartado.is_empty() {
            eprintln!(
                "--force: {} commit(s) de la ventana quedan afuera del corte nuevo",
                descartado.len()
            );
            for l in descartado.iter().take(3) {
                eprintln!("  {l}");
            }
        }
    }

    git_output(repo, &["update-ref", &branch, &head])?;

    // Y la contabilidad de la propagacion se mueve con la rama.
    //
    // Regenerar reescribe la historia, asi que la ref se queda apuntando a un
    // commit que ya no es ancestro de nada. `pending` la usa como piso —`log
    // <ref>..<tip>`— y con el piso afuera de la rama ese rango es **la
    // ventana entera**: el proximo push re-propagaria todo lo que ya subio.
    //
    // No es hipotetico desde que `pull` regenera en cada invocacion.
    let anotar = if propagada {
        // Subio entera y el corte la contiene: el tip nuevo esta propagado.
        Some(head.clone())
    } else if !corte_nuevo.is_empty() {
        // Lo que sale del panorama esta propagado por construccion. Lo que
        // quedo encima es el trabajo que todavia no subio, que es justo lo que
        // la ref tiene que dejar afuera.
        Some(corte_nuevo)
    } else {
        None
    };
    if let Some(hasta) = anotar {
        git_output(repo, &["update-ref", &crate::git::propagated_ref(&branch), &hasta])?;
    }

    Ok((files, head))
}

/// Si el servidor ya tiene **todo** lo de esta vista.
///
/// No es *"mi punta es la marca"*: el servidor commitea encima de lo que
/// recibe —el `rename`, el `normalize:`, la clave del sprint— asi que despues
/// de un push la marca esta **adelante** de mi HEAD y las dos comparaciones dan
/// distinto.
///
/// Medido el 2026-09-07: comparando por igualdad, el commit que **creaba** un
/// item se replantaba despues de que el servidor lo renombrara, y volvia a
/// escribir el archivo con el slug viejo — la vista quedaba con el item dos
/// veces, con dos nombres.
///
/// Se pregunta **en el bare**: la marca apunta a la historia del servidor, que
/// un `fetch` no necesariamente trae, y ahi el objeto puede no existir de este
/// lado.
pub fn propagado_entero(bare: &Path, branch_ref: &str, head: &str) -> bool {
    let Some(marca) = crate::git::rev_parse(bare, &crate::git::propagated_ref(branch_ref)) else {
        return false;
    };
    crate::git::try_git(bare, &["merge-base", "--is-ancestor", head, &marca])
        .map(|(ok, _)| ok)
        .unwrap_or(false)
}

/// Como quedo un replante: que se aplico, que se dejo caer y por que.
pub struct Replante {
    pub replantados: usize,
    /// Lo que no se replanto **porque ya estaba**, con el motivo al lado. Son
    /// tres, y decirlos aparte es la diferencia entre un descarte y una perdida.
    pub caidos: Vec<String>,
    /// El unico choque que informa algo: el trabajo toca un item que el `items`
    /// de hoy ya no lleva.
    pub conflicto: Option<(String, Vec<String>)>,
}

/// Replanta sobre `tip` lo que la vista tiene y `tip` no.
///
/// Es el paso 3 de `worklist pull`, y vive aca —y no en el binario del
/// cliente— porque es donde se puede probar contra un servidor de mentira.
///
/// **El corte nunca se replanta**: es un commit que borra lo que el recorte
/// dejo afuera, y re-aplicarlo sobre un panorama que crecio no menciona lo
/// nuevo, asi que lo nuevo entra.
pub fn replantar(view: &Path, tip: &str, desde: &str) -> Result<Replante> {
    let mios = git_output(view, &["log", "--oneline", &format!("{tip}..{desde}")])?;
    let shas = git_output(view, &["rev-list", "--reverse", &format!("{tip}..{desde}")])?;
    let antes = git_output(view, &["rev-parse", "HEAD"])?.trim().to_string();

    git_output(view, &["reset", "--hard", "--quiet", tip])?;

    let mut r = Replante { replantados: 0, caidos: Vec::new(), conflicto: None };
    for sha in shas.lines() {
        let subject = git_output(view, &["log", "-1", "--format=%s", sha])?.trim().to_string();
        if subject.starts_with("window: ") {
            continue;
        }
        let linea = mios
            .lines()
            .find(|l| l.starts_with(&sha[..7.min(sha.len())]))
            .unwrap_or(sha)
            .to_string();
        match crate::git::cherry_pick_one(view, sha)? {
            crate::git::Picked::Applied => r.replantados += 1,
            crate::git::Picked::Empty => {}
            crate::git::Picked::Superseded => {
                r.caidos.push(format!("{linea} (ya esta en el corte modulo normalizacion)"))
            }
            crate::git::Picked::Conflict { ref files, .. } if crate::git::planning_only(files) => {
                r.caidos.push(format!("{linea} (la planificacion del sprint es del panorama)"))
            }
            crate::git::Picked::Conflict { .. } if crate::git::redone_above(&subject) => {
                r.caidos.push(format!("{linea} (el corte ya lo trae rehecho)"))
            }
            crate::git::Picked::Conflict { files, .. } => {
                // La vista vuelve a como estaba: quedar a medias adentro de un
                // cherry-pick abortado es peor que no haber empezado.
                git_output(view, &["reset", "--hard", "--quiet", &antes])?;
                r.conflicto = Some((linea, files));
                return Ok(r);
            }
        }
    }
    Ok(r)
}
