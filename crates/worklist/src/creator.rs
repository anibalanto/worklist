//! Asignar una clave real a un pedido: buscar por titulo antes de crear, para
//! que un reintento despues de una falla nunca duplique.

use anyhow::{bail, Context, Result};
use std::process::Command;

pub trait Creator {
    fn create_or_find(&self, title: &str, item_type: &str, description: &str) -> Result<String>;
}

/// Escapa un titulo para entrar en un JQL `summary ~ "..."`: backslash y
/// comillas dobles, que son los dos caracteres que rompen la query si van
/// crudos. Sin esto, un titulo real con comillas rompe la busqueda en
/// silencio y el reintento duplica — el defecto que motiva este modulo.
pub fn escape_jql(title: &str) -> String {
    title.replace('\\', "\\\\").replace('"', "\\\"")
}

pub struct AcliCreator {
    project: String,
}

impl AcliCreator {
    pub fn new(project: impl Into<String>) -> Self {
        AcliCreator { project: project.into() }
    }

    fn jql(&self, title: &str) -> String {
        format!(
            "project = {} AND summary ~ \"{}\"",
            self.project,
            escape_jql(title)
        )
    }

    fn search(&self, title: &str) -> Result<Option<String>> {
        let jql = self.jql(title);
        let out = Command::new("acli")
            .args(["jira", "workitem", "search", "--jql", &jql, "--json"])
            .output()
            .context("corriendo acli jira workitem search")?;
        if !out.status.success() {
            bail!("acli search fallo: {}", String::from_utf8_lossy(&out.stderr));
        }
        let text = String::from_utf8(out.stdout)?;
        let parsed: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("parseando salida de acli search: {text}"))?;
        let key = parsed
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|first| first.get("key"))
            .and_then(|k| k.as_str())
            .map(|s| s.to_string());
        Ok(key)
    }

    fn create(&self, title: &str, item_type: &str, description: &str) -> Result<String> {
        let out = Command::new("acli")
            .args([
                "jira", "workitem", "create",
                "--project", &self.project,
                "--type", item_type,
                "--summary", title,
                "--description", description,
                "--json",
            ])
            .output()
            .context("corriendo acli jira workitem create")?;
        if !out.status.success() {
            bail!("acli create fallo: {}", String::from_utf8_lossy(&out.stderr));
        }
        let text = String::from_utf8(out.stdout)?;
        let parsed: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("parseando salida de acli create: {text}"))?;
        parsed
            .get("key")
            .and_then(|k| k.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("acli create no devolvio 'key': {text}"))
    }
}

impl Creator for AcliCreator {
    fn create_or_find(&self, title: &str, item_type: &str, description: &str) -> Result<String> {
        if let Some(key) = self.search(title)? {
            return Ok(key);
        }
        self.create(title, item_type, description)
    }
}

/// Lo que `--dry-run` imprime, sin llamar a `acli`. El escapado acá es sólo
/// para que la línea se lea como el comando real —`Command` nunca pasa por
/// un shell—, pero mostrar comillas sin escapar rompería la lectura igual.
pub fn dry_run_plan(project: &str, item_type: &str, title: &str, description: &str) -> String {
    let display = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "would search: project = {project} AND summary ~ \"{}\"\nwould create: --project {project} --type {item_type} --summary \"{}\" --description \"{}\"",
        escape_jql(title),
        display(title),
        display(description),
    )
}
