//! Nucleo de la sincronizacion por ventana: renombrar un item sin clave de
//! proveedor y reescribir todo lo que lo nombraba, en un solo commit; y el
//! compare-and-swap que decide si un push a una ventana se acepta.
//!
//! `resolve_batch` no habla con ningun proveedor: quien asigna la clave nueva
//! es quien la llama, pasandole el mapa ya resuelto. `check_push` si habla
//! con uno, via el puerto: `port` declara las operaciones y a que transporte
//! le toca cada una, `board` y `provider` las implementan contra Jira.

pub mod assign;
pub mod board;
pub mod body;
pub mod check_push;
pub mod port;
pub mod propagate;
pub mod provider;
pub mod window;

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const TYPES: [&str; 4] = ["task", "user-story", "epic", "sprint"];

/// Encuentra `<slug>.<tipo>.md` en `repo`, probando los tipos conocidos.
pub fn find_file(repo: &Path, slug: &str) -> Result<(PathBuf, String)> {
    for t in TYPES {
        let p = repo.join(format!("{slug}.{t}.md"));
        if p.exists() {
            return Ok((p, t.to_string()));
        }
    }
    Err(anyhow!("no existe {slug}.<tipo>.md en {}", repo.display()))
}

/// Un id de proveedor lleva mayuscula y guion (ACC-101); un slug local no.
pub fn is_unassigned(slug: &str) -> bool {
    let re = Regex::new(r"^[A-Z]+-\d+$").unwrap();
    !re.is_match(slug)
}

fn boundary(pattern: &str) -> Regex {
    Regex::new(&format!("{pattern}(?:[^A-Za-z0-9_-]|$)")).unwrap()
}

/// Reescribe en `text` toda referencia delimitada a `old_slug` (link, backtick,
/// campo de frontmatter) por `new_id`. Nunca toca una subcadena suelta: un
/// `old_slug` **rodeado** de caracteres de identificador no matchea, de los dos
/// lados. Los ids son base-36 y crecen de a uno, asi que uno siempre es sufijo
/// de otro —`j` de `2j`, `1` de `21`— y el riesgo sube con el contador.
pub fn rewrite_references(text: &str, old_slug: &str, old_type: &str, new_id: &str) -> (String, bool) {
    let mut changed = false;
    let mut out = text.to_string();

    // 1. destinos de link: ](old_slug.tipo.md  [...anchor u otro cierre]
    //
    // Con los `../` que le anteceden: desde `_sprints/` un link a un item se
    // escribe `](../4h.task.md)`, y sin esto no matchea. Ver la task `63`.
    let link_pat = format!(
        r"\]\((?:\.\./)*{}\.{}\.md",
        regex::escape(old_slug),
        regex::escape(old_type)
    );
    let link_re = boundary(&link_pat);
    // El prefijo `../` se conserva: el archivo se movio de nombre, no de lugar.
    out = replace_link(&link_re, &out, &mut changed, new_id, old_type);

    // 2. frontmatter: parent: <slug>  y  relation.<tipo>: [...] o valor suelto
    if let Some(fm_end) = frontmatter_end(&out) {
        let (fm, rest) = out.split_at(fm_end);
        let mut fm = fm.to_string();

        // El delimitador va en un grupo y **se restituye**: sin eso el `\n` se
        // pierde, y cuando `parent:` es la ultima linea del frontmatter el
        // cierre queda pegado —`ACC-14---`— y el bloque deja de separarse.
        // Todo el archivo pasa a ser cuerpo. Ver la task `5i`.
        let parent_re = Regex::new(&format!(
            r"parent:\s*{}([^A-Za-z0-9_-]|$)",
            regex::escape(old_slug)
        ))
        .unwrap();
        if parent_re.is_match(&fm) {
            fm = parent_re
                .replace(&fm, format!("parent: {new_id}${{1}}"))
                .to_string();
            changed = true;
        }

        // Los campos de **lista** del frontmatter, en un solo lugar: agregar uno
        // nuevo es agregarlo aca y no en tres lados. `items` es el del sprint,
        // que la task `4x` agrego y que el renombre nunca miraba.
        let rel_re = Regex::new(r"((?:relation\.[a-zA-Z_]+|items):\s*)(\[[^\]]*\]|[^\n]+)").unwrap();
        let slug_re = boundary(&regex::escape(old_slug));
        fm = rel_re
            .replace_all(&fm, |caps: &regex::Captures| {
                let prefix = &caps[1];
                let body = &caps[2];
                let new_body = replace_boundary_word(&slug_re, body, &mut changed, new_id);
                format!("{prefix}{new_body}")
            })
            .to_string();

        out = format!("{fm}{rest}");
    }

    // 3. ids entre backticks en prosa: `old_slug`
    let backtick_pat = format!("`{}`", regex::escape(old_slug));
    let backtick_re = boundary(&backtick_pat);
    out = replace_boundary(&backtick_re, &out, &mut changed, &format!("`{new_id}`"));

    (out, changed)
}

/// `Regex::replace_all` pero marcando `changed` y sin perder el caracter de
/// cierre que la lookahead-manual de `boundary` consume como parte del match.
fn replace_boundary(re: &Regex, text: &str, changed: &mut bool, new_head: &str) -> String {
    replace_boundary_inner(re, text, changed, new_head, false)
}

/// Como `replace_boundary`, pero exigiendo tambien limite **a la izquierda**.
///
/// Es para el patron que empieza con el slug crudo —el de un `relation.<tipo>`—
/// donde no hay ningun delimitador literal adelante que lo proteja. Sin esto,
/// renombrar `j` entra adentro de `2j` y escribe `2ACC-77`, que no es el id de
/// nada. Ver la task `5m`.
///
/// El limite **no va en el patron**: metido ahi, el primer match se comeria el
/// caracter que separa dos referencias adyacentes —`[j,k]`— y la segunda se
/// quedaria sin limite izquierdo. Se mira el texto de al lado, que no consume.
fn replace_boundary_word(re: &Regex, text: &str, changed: &mut bool, new_head: &str) -> String {
    replace_boundary_inner(re, text, changed, new_head, true)
}

fn replace_boundary_inner(
    re: &Regex,
    text: &str,
    changed: &mut bool,
    new_head: &str,
    check_left: bool,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for m in re.find_iter(text) {
        if check_left && !left_is_boundary(text, m.start()) {
            continue;
        }
        let matched = m.as_str();
        // el ultimo char del match es el delimitador (o vacio si es fin de string)
        let head_len = matched.len() - trailing_delim_len(matched);
        out.push_str(&text[last..m.start()]);
        out.push_str(new_head);
        out.push_str(&matched[head_len..]);
        last = m.end();
        *changed = true;
    }
    out.push_str(&text[last..]);
    out
}

/// El inicio del texto es limite, y tambien cualquier caracter que no sea de
/// identificador.
fn left_is_boundary(text: &str, at: usize) -> bool {
    match text[..at].chars().next_back() {
        None => true,
        Some(c) => !c.is_ascii_alphanumeric() && c != '_' && c != '-',
    }
}

fn trailing_delim_len(matched: &str) -> usize {
    // boundary() agrega (?:[^A-Za-z0-9_-]|$) al final: 0 o 1 byte ascii, o 0 si matcheo $.
    match matched.chars().last() {
        Some(c) if !c.is_ascii_alphanumeric() && c != '_' && c != '-' => c.len_utf8(),
        _ => 0,
    }
}

fn frontmatter_end(text: &str) -> Option<usize> {
    if !text.starts_with("---\n") {
        return None;
    }
    let rest = &text[4..];
    let idx = rest.find("\n---\n")?;
    Some(4 + idx + 5)
}

/// Los ids (`parent` + todo `relation.*`) que el frontmatter de `text` nombra.
pub fn read_frontmatter_refs(text: &str) -> HashSet<String> {
    let mut refs = HashSet::new();
    let Some(end) = frontmatter_end(text) else { return refs };
    let fm = &text[..end];

    let parent_re = Regex::new(r"(?m)^parent:\s*(\S+)").unwrap();
    if let Some(c) = parent_re.captures(fm) {
        refs.insert(c[1].to_string());
    }
    let rel_re = Regex::new(r"relation\.[a-zA-Z_]+:\s*(\[[^\]]*\]|\S+)").unwrap();
    let id_re = Regex::new(r"[A-Za-z0-9_-]+").unwrap();
    for c in rel_re.captures_iter(fm) {
        for id in id_re.find_iter(&c[1]) {
            refs.insert(id.as_str().to_string());
        }
    }
    refs
}

/// Orden topologico de `slugs` (todos sin clave) segun sus referencias a otros
/// slugs del mismo lote. Error si hay un ciclo.
pub fn topo_order(repo: &Path, slugs: &[String]) -> Result<Vec<String>> {
    let slug_set: HashSet<&str> = slugs.iter().map(|s| s.as_str()).collect();
    let mut deps: HashMap<String, HashSet<String>> = HashMap::new();
    for slug in slugs {
        let (path, _) = find_file(repo, slug)?;
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("leyendo {}", path.display()))?;
        let refs = read_frontmatter_refs(&text);
        let filtered: HashSet<String> = refs
            .into_iter()
            .filter(|r| slug_set.contains(r.as_str()))
            .collect();
        deps.insert(slug.clone(), filtered);
    }

    let mut order = Vec::new();
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();

    fn visit(
        s: &str,
        deps: &HashMap<String, HashSet<String>>,
        visiting: &mut HashSet<String>,
        visited: &mut HashSet<String>,
        order: &mut Vec<String>,
    ) -> Result<()> {
        if visited.contains(s) {
            return Ok(());
        }
        if visiting.contains(s) {
            bail!("ciclo detectado en {s}");
        }
        visiting.insert(s.to_string());
        for dep in &deps[s] {
            visit(dep, deps, visiting, visited, order)?;
        }
        visiting.remove(s);
        visited.insert(s.to_string());
        order.push(s.to_string());
        Ok(())
    }

    for s in slugs {
        visit(s, &deps, &mut visiting, &mut visited, &mut order)?;
    }
    Ok(order)
}

/// `git -C <repo>`, sin las variables de **ubicación** que le ganan al `-C`.
///
/// Un hook de recepción hereda `GIT_DIR=.` y a veces `GIT_WORK_TREE`, y esas le
/// ganan al `-C`: cualquier git que corra desde otro directorio —un worktree
/// temporal, por ejemplo— resuelve contra el repo equivocado y falla con *"no
/// es un repositorio git"*. Sacarlas acá y no en el script del hook es lo que
/// hace que la librería no dependa de quién la llama.
///
/// **Las de objetos no se tocan**, y la diferencia importa: en `pre-receive`
/// los commits que llegan todavía no están en el repo, viven en una cuarentena
/// que `GIT_QUARANTINE_PATH` y `GIT_OBJECT_DIRECTORY` señalan. Quitarlas deja a
/// git sin poder leer lo que el push trae — `bad object <sha>`— y eso no se
/// notaba mientras el compare-and-swap sólo miraba el tip anterior.
pub(crate) fn git_command(repo: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo);
    for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        cmd.env_remove(var);
    }
    cmd
}

/// Si `branch` esta checkouteada en algun worktree, cual.
///
/// `git branch -f` se niega a mover una rama con worktree activo; `update-ref`
/// no, y la mueve dejando el indice del worktree apuntando al arbol anterior.
/// El resultado es un `git status` con archivos "modificados" que nadie toco, y
/// un `git merge` que se niega a seguir por cambios locales que no existen.
pub(crate) fn checked_out_at(repo: &Path, branch: &str) -> Option<String> {
    let listing = git_command(repo)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())?;
    let mut path = None;
    for line in listing.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.to_string());
        } else if line.strip_prefix("branch ") == Some(branch) {
            return path;
        }
    }
    None
}

fn git(repo: &Path, args: &[&str]) -> Result<()> {
    let status = git_command(repo)
        .args(args)
        .status()
        .with_context(|| format!("corriendo git {:?}", args))?;
    if !status.success() {
        bail!("git {:?} fallo con {status}", args);
    }
    Ok(())
}

/// Commitea lo que haya en el arbol, con el mensaje dado. El servidor deja
/// su trabajo como un commit propio: nunca reescribe el del cliente.
pub fn commit_all(repo: &Path, msg: &str) -> Result<()> {
    git(repo, &["add", "-A"])?;
    git(repo, &["commit", "-q", "-m", msg])
}

/// Renombra un item y reescribe sus referencias en todo el repo, en un commit.
/// Devuelve los nombres de archivo tocados (sin contar el propio renombrado).
/// Todos los `.md` de la capa, **incluidos los de subdirectorios**.
///
/// Los items viven en la raiz, pero un `.sprint.md` vive en `_sprints/` y
/// referencia items: recorrer solo la raiz lo dejaba con los slugs viejos. Y
/// bajar no puede confundir un directorio con un item, porque en el worklist
/// **todo directorio empieza con `_`** y un nombre asi nunca es un id.
fn markdown_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut pila = vec![root.to_path_buf()];
    while let Some(dir) = pila.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            if p.is_dir() {
                if name != ".git" {
                    pila.push(p);
                }
            } else if p.extension().and_then(|x| x.to_str()) == Some("md") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Como `replace_boundary`, pero conservando los `../` que el match capturo:
/// lo que cambia es el nombre del archivo, no donde esta.
fn replace_link(
    re: &Regex,
    text: &str,
    changed: &mut bool,
    new_id: &str,
    item_type: &str,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for m in re.find_iter(text) {
        let matched = m.as_str();
        let dots = matched
            .strip_prefix("](")
            .map(|r| r.len() - r.trim_start_matches("../").len())
            .unwrap_or(0);
        let prefijo = &matched[2..2 + dots];
        let head_len = matched.len() - trailing_delim_len(matched);
        out.push_str(&text[last..m.start()]);
        out.push_str(&format!("]({prefijo}{new_id}.{item_type}.md"));
        out.push_str(&matched[head_len..]);
        last = m.end();
        *changed = true;
    }
    out.push_str(&text[last..]);
    out
}

pub fn rename_one(repo: &Path, old_slug: &str, new_id: &str) -> Result<Vec<String>> {
    let (src, item_type) = find_file(repo, old_slug)?;
    let dst_name = format!("{new_id}.{item_type}.md");

    let mut touched = Vec::new();
    for path in markdown_files(repo) {
        let text = std::fs::read_to_string(&path)?;
        let (new_text, changed) = rewrite_references(&text, old_slug, &item_type, new_id);
        if changed {
            std::fs::write(&path, new_text)?;
            touched.push(
                path.strip_prefix(repo).unwrap_or(&path).to_string_lossy().to_string(),
            );
        }
    }

    git(repo, &["mv", src.file_name().unwrap().to_str().unwrap(), &dst_name])?;
    git(repo, &["add", "-A"])?;
    let msg = if touched.is_empty() {
        format!("rename {old_slug} -> {new_id}")
    } else {
        format!("rename {old_slug} -> {new_id} ({} refs)", touched.len())
    };
    git(repo, &["commit", "-q", "-m", &msg])?;
    Ok(touched)
}

/// Renombra un lote de pedidos en orden topologico. Si hay un ciclo, no
/// escribe nada: `topo_order` falla antes de tocar el repo.
pub fn resolve_batch(repo: &Path, slug_to_id: &HashMap<String, String>) -> Result<()> {
    let slugs: Vec<String> = slug_to_id.keys().cloned().collect();
    let order = topo_order(repo, &slugs)?;
    for slug in order {
        let new_id = &slug_to_id[&slug];
        rename_one(repo, &slug, new_id)?;
    }
    Ok(())
}
