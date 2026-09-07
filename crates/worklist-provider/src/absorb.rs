//! `worklist-server absorb`: lo que cambio en el board entra a la ventana.
//!
//! **Es la unica direccion que faltaba.** `push-states` sube el status,
//! `propagate` sube lo que la ventana resolvio, `reconcile` adopta issues que
//! ya existen del otro lado — la existencia, no el contenido. Un cambio hecho
//! en el board se **detectaba** —`check-push` rechazaba el push— y no tenia por
//! donde entrar.
//!
//! Es del servidor por los dos criterios: habla con el proveedor y escribe en
//! una rama suya. Y eso es lo que deja al cliente sin cambiar — `pull` lo
//! invoca y despues baja lo que el servidor escribio, como cualquier otra cosa.
//!
//! Ver `commands/absorb.md`.

use crate::provider::Provider;
use crate::states::Estados;
use anyhow::Result;
use std::path::Path;
use worklist_core::git::git_output;

/// Que se hizo con cada clave que difiere.
#[derive(Debug)]
pub enum Paso {
    /// Se escribio el valor del proveedor en el archivo.
    Absorbido { key: String, campo: &'static str, antes: String, ahora: String },
    /// Difiere y **no se toca**, con el motivo. Reportar de mas es ruido;
    /// absorber de mas es escribir.
    Reportado { key: String, campo: &'static str, porque: String },
    /// El proveedor **no informo** esta clave.
    ///
    /// No es que coincida: es que no se vio. Callarlo convierte *"no se pudo
    /// preguntar"* en *"esta todo bien"*, que es el mismo defecto de forma que
    /// `sin verificar` contra `coincide` — y con una instalacion apuntando a un
    /// proveedor de prueba vacio, **son todas**. Medido el 2026-09-07: absorb
    /// cerro con `0 absorbido, 0 reportado` sobre 20 claves que nadie informo.
    SinInformar { key: String },
}

#[derive(Debug, Default)]
pub struct Absorbido {
    pub claves: usize,
    pub pasos: Vec<Paso>,
    /// El commit que quedo, si hubo algo que absorber.
    pub commit: Option<String>,
}

impl Absorbido {
    pub fn absorbidos(&self) -> usize {
        self.pasos.iter().filter(|p| matches!(p, Paso::Absorbido { .. })).count()
    }
    pub fn reportados(&self) -> usize {
        self.pasos.iter().filter(|p| matches!(p, Paso::Reportado { .. })).count()
    }
    /// Las que el proveedor no informo. **Es la cuenta que no puede faltar**:
    /// cero absorbidos y cero reportados sobre veinte claves se lee igual que
    /// "todo coincide", y no es lo mismo.
    pub fn sin_informar(&self) -> usize {
        self.pasos.iter().filter(|p| matches!(p, Paso::SinInformar { .. })).count()
    }
}

/// Trae a la rama de la ventana lo que el proveedor dice y el tip no.
pub fn absorb(
    repo: &Path,
    refname: &str,
    provider: &dyn Provider,
    estados: &Estados,
    dry_run: bool,
) -> Result<Absorbido> {
    let beliefs = crate::check_push::tip_beliefs(repo, refname)?;
    let mut claves: Vec<String> = beliefs.keys().cloned().collect();
    claves.sort();

    let mut out = Absorbido { claves: claves.len(), ..Default::default() };
    if claves.is_empty() {
        return Ok(out);
    }

    // Una sola operacion, con todas las claves juntas: contra Jira es una JQL
    // paginada y no una llamada por item. Para una ventana de 19 es **una**
    // consulta — el numero caro es el panorama de 292, que no es esto.
    let vivo = provider.snapshot(&claves)?;

    // Lo que todavia no subio es lo que no se pisa: entre la marca de
    // propagacion y el tip esta el trabajo que nadie mas vio, y ahi los dos
    // lados escribieron. La negativa es el dato.
    let mios = tocados_desde_la_marca(repo, refname).unwrap_or_default();

    let mut escrituras: Vec<(String, String)> = Vec::new();
    for key in &claves {
        let Some(snap) = vivo.get(key) else {
            out.pasos.push(Paso::SinInformar { key: key.clone() });
            continue;
        };
        // Y una clave informada **sin ningun campo** es lo mismo: el de prueba
        // solo lleva status, asi que sobre titulo y cuerpo no vio nada.
        if snap.status.is_none() && snap.summary.is_none() && snap.description.is_none() {
            out.pasos.push(Paso::SinInformar { key: key.clone() });
            continue;
        }
        let Ok(file) = archivo_de(repo, refname, key) else { continue };
        let Ok(texto) = git_output(repo, &["show", &format!("{refname}:{file}")]) else { continue };
        let mut texto = texto;
        let mut cambio = false;

        // --- titulo: la vuelta es directa ---
        if let (Some(alla), Some(aca)) = (&snap.summary, crate::assign::title_of(&texto)) {
            if *alla != aca {
                if mios.contains(&file) {
                    out.pasos.push(Paso::Reportado {
                        key: key.clone(),
                        campo: "titulo",
                        porque: "cambiado alla y aca desde la ultima propagacion — no se toca"
                            .into(),
                    });
                } else {
                    texto = con_titulo(&texto, alla);
                    cambio = true;
                    out.pasos.push(Paso::Absorbido {
                        key: key.clone(),
                        campo: "titulo",
                        antes: aca,
                        ahora: alla.clone(),
                    });
                }
            }
        }

        // --- status: solo si la vuelta es unica ---
        if let (Some(alla), Some(aca)) = (&snap.status, beliefs.get(key)) {
            let esperado = estados.destino(aca).map(|d| d.status().to_string());
            if esperado.as_deref() != Some(alla.as_str()) {
                let candidatos = estados.desde(alla);
                match candidatos.as_slice() {
                    // **Elegir es inventar.** Que dos estados del worklist
                    // compartan uno del proveedor no es un defecto del comando:
                    // es una limitacion del board, y va a seguir pasando.
                    [] => out.pasos.push(Paso::Reportado {
                        key: key.clone(),
                        campo: "status",
                        porque: format!("el board dice \"{alla}\", que no vuelve a ningun estado del vocabulario"),
                    }),
                    [uno] => {
                        if mios.contains(&file) {
                            out.pasos.push(Paso::Reportado {
                                key: key.clone(),
                                campo: "status",
                                porque: "cambiado alla y aca desde la ultima propagacion — no se toca".into(),
                            });
                        } else {
                            let (nuevo, antes) =
                                worklist_core::states::proponer(&texto, uno, &ahora())?;
                            texto = nuevo;
                            cambio = true;
                            out.pasos.push(Paso::Absorbido {
                                key: key.clone(),
                                campo: "status",
                                antes,
                                ahora: (*uno).to_string(),
                            });
                        }
                    }
                    varios => out.pasos.push(Paso::Reportado {
                        key: key.clone(),
                        campo: "status",
                        porque: format!(
                            "el board dice \"{alla}\", que vuelve a `{}` — no se elige solo",
                            varios.join("` o a `")
                        ),
                    }),
                }
            }
        }

        // --- cuerpo: no, todavia ---
        //
        // 113 de los 149 que difieren son del conversor, asi que absorber hoy
        // reescribiria mas de cien items con cambios que nadie hizo. Se reporta,
        // que es lo que `check-push` ya hace.
        if let Some(adf) = &snap.description {
            if cuerpo_difiere(&texto, adf) {
                out.pasos.push(Paso::Reportado {
                    key: key.clone(),
                    campo: "cuerpo",
                    porque: "el cuerpo no se absorbe todavia: el round-trip no cierra".into(),
                });
            }
        }

        if cambio {
            escrituras.push((file, texto));
        }
    }

    if escrituras.is_empty() || dry_run {
        return Ok(out);
    }
    out.commit = Some(escribir(repo, refname, &escrituras)?);
    Ok(out)
}

/// Los archivos que la ventana toco **despues** de la ultima propagacion: el
/// trabajo que nadie mas vio, y por lo tanto lo que absorber pisaria.
fn tocados_desde_la_marca(repo: &Path, refname: &str) -> Option<Vec<String>> {
    let marca = worklist_core::git::rev_parse(repo, &worklist_core::git::propagated_ref(refname))?;
    let salida = git_output(repo, &["diff", "--name-only", &marca, refname]).ok()?;
    Some(salida.lines().map(|s| s.to_string()).collect())
}

fn archivo_de(repo: &Path, rev: &str, key: &str) -> Result<String> {
    let listing = git_output(repo, &["ls-tree", "-r", "--name-only", rev])?;
    listing
        .lines()
        .find(|n| crate::provider::key_of_filename(n).as_deref() == Some(key))
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("{key} no esta en {rev}"))
}

/// El titulo del frontmatter, reemplazado. Se escribe entre comillas simples
/// porque un titulo lleva `:` y `` ` `` con toda naturalidad.
fn con_titulo(texto: &str, titulo: &str) -> String {
    let escapado = titulo.replace('\'', "''");
    let re = regex::Regex::new(r"(?m)^title:.*$").unwrap();
    re.replace(texto, format!("title: '{escapado}'").as_str()).to_string()
}

/// Si el cuerpo del archivo y el del proveedor dicen distinto.
///
/// Se compara **markdown contra markdown**, y con la misma ida que aplica el
/// que sube: aca un item cita a otro por su archivo y alla eso es una URL.
fn cuerpo_difiere(texto: &str, adf: &str) -> bool {
    let Ok(alla) = worklist_core::body::adf_to_body(adf) else { return false };
    let (_, aca) = worklist_core::body::split_frontmatter(texto);
    alla.trim() != aca.trim()
}

/// Un commit sobre la rama de la ventana, en un worktree temporal.
///
/// El bare no tiene donde escribir, y la rama puede estar checkouteada del otro
/// lado: se trabaja aparte y se mueve la ref, que es lo que hacen las otras
/// pasadas del servidor.
fn escribir(repo: &Path, refname: &str, escrituras: &[(String, String)]) -> Result<String> {
    let tmp = repo.join("../.worklist-absorb");
    let _ = std::fs::remove_dir_all(&tmp);
    git_output(repo, &["worktree", "add", "--detach", "-q", tmp.to_str().unwrap(), refname])?;

    let hecho = (|| -> Result<String> {
        for (file, texto) in escrituras {
            std::fs::write(tmp.join(file), texto)?;
        }
        // El mensaje nombra las claves: el commit lo va a leer alguien que no
        // corrio el comando, cuando `pull` se lo baje.
        let claves: Vec<String> = escrituras
            .iter()
            .filter_map(|(f, _)| crate::provider::key_of_filename(f))
            .collect();
        worklist_core::commit_all(&tmp, &format!("absorb: {}", claves.join(", ")))?;
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

/// Que paso con la membresia de un sprint.
#[derive(Debug)]
pub enum Membresia {
    /// El board saco un item del sprint, y la composicion lo saca tambien.
    Sacado { sprint: String, key: String },
    /// El board tiene uno que la composicion no. **No se agrega**: entrar a un
    /// sprint es planificar, y eso se hace de este lado — lo que el proveedor
    /// arbitra es la baja de lo que el ya no tiene. Se reporta.
    SoloAlla { sprint: String, key: String },
    /// El proveedor no pudo contestar que tiene el sprint. **No es que
    /// coincida**: sin eso no hay con que comparar, y vaciar el `items` porque
    /// una lectura fallo seria catastrofico.
    NoSePudoLeer { sprint: String, porque: String },
    /// El board contesto **vacio** sobre un sprint que la composicion dice que
    /// tiene items. **No se toca**, y es la guarda que mas importa: sacarlos
    /// todos es de otra magnitud que sacar uno, y una lectura vacia no se
    /// distingue de una que no anduvo — un board que responde 200 con una lista
    /// vacia se ve igual que un sprint que existe y no tiene nada.
    VacioSospechoso { sprint: String, tenia: usize },
}

/// Sincroniza el `items` de cada sprint con lo que el board tiene.
///
/// **Corre sobre el panorama**, que es donde vive la composicion. Y es comparar
/// dos listas de claves — que es lo que la composicion prometia volver esto:
/// *sincronizar deja de ser una traduccion y pasa a ser un espejo*.
///
/// El proveedor manda: si el board saco un item del sprint, sale del `items`.
/// Sin esto la baja **no sobrevive a un push**, porque la pasada de sprint
/// agrega lo que la composicion nombra y el board no tiene — medido el
/// 2026-09-07: seis items volvieron al sprint en una sola corrida.
pub fn membresia(
    repo: &Path,
    refname: &str,
    board: &dyn crate::board::Board,
    board_id: &str,
    dry_run: bool,
) -> Result<(Vec<Membresia>, Option<String>)> {
    let producto = worklist_core::product::leer(repo, refname)?;
    let mut pasos = Vec::new();
    let mut sacar: Vec<(String, String)> = Vec::new();

    for s in &producto.sprints {
        // Un sprint sin clave no existe del otro lado: no hay con que comparar,
        // y eso no es una diferencia.
        let Some(key) = &s.key else { continue };
        if s.items.is_empty() {
            continue;
        }
        let alla = match board.sprint_items(board_id, key) {
            Ok(v) => v,
            Err(e) => {
                pasos.push(Membresia::NoSePudoLeer {
                    sprint: s.id.clone(),
                    porque: format!("{e:#}").lines().next().unwrap_or("").to_string(),
                });
                continue;
            }
        };
        // La guarda: vaciar de golpe no se hace solo.
        if alla.is_empty() {
            pasos.push(Membresia::VacioSospechoso {
                sprint: s.id.clone(),
                tenia: s.items.len(),
            });
            continue;
        }
        for item in &s.items {
            if !alla.contains(item) {
                pasos.push(Membresia::Sacado { sprint: s.id.clone(), key: item.clone() });
                sacar.push((s.id.clone(), item.clone()));
            }
        }
        for item in &alla {
            if !s.items.contains(item) {
                pasos.push(Membresia::SoloAlla { sprint: s.id.clone(), key: item.clone() });
            }
        }
    }

    if sacar.is_empty() || dry_run {
        return Ok((pasos, None));
    }
    Ok((pasos, Some(escribir_membresia(repo, refname, &sacar)?)))
}

fn escribir_membresia(repo: &Path, refname: &str, sacar: &[(String, String)]) -> Result<String> {
    let tmp = repo.join("../.worklist-membresia");
    let _ = std::fs::remove_dir_all(&tmp);
    git_output(repo, &["worktree", "add", "--detach", "-q", tmp.to_str().unwrap(), refname])?;

    let hecho = (|| -> Result<String> {
        let path = tmp.join(worklist_core::product::ARCHIVO);
        let mut p = worklist_core::product::de_yaml(&std::fs::read_to_string(&path)?)?;
        for (sprint, key) in sacar {
            if let Some(s) = p.sprints.iter_mut().find(|s| &s.id == sprint) {
                s.items.retain(|i| i != key);
            }
        }
        std::fs::write(&path, p.to_yaml()?)?;
        let que: Vec<String> =
            sacar.iter().map(|(s, k)| format!("{k} del sprint {s}")).collect();
        worklist_core::commit_all(&tmp, &format!("membresia: sale {}", que.join(", ")))?;
        Ok(git_output(&tmp, &["rev-parse", "HEAD"])?.trim().to_string())
    })();

    let _ = git_output(repo, &["worktree", "remove", "--force", tmp.to_str().unwrap()]);
    let head = hecho?;
    git_output(repo, &["update-ref", refname, &head])?;
    Ok(head)
}
