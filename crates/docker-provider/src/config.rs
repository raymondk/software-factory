use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;

use anyhow::{bail, Context};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub provider: Provider,
    /// The agents this provider can run, by name.
    pub agents: BTreeMap<String, Agent>,
    /// Extra environment for every worker container (credentials).
    #[serde(default)]
    pub worker_env: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub listen: SocketAddr,
    pub max_workers: u32,
    /// Bearer token the orchestrator must send to start or stop workers.
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    pub image: String,
    pub models: Vec<String>,
    pub default_model: String,
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        Config::from_table(load_table(path)?).with_context(|| format!("in {}", path.display()))
    }

    pub fn parse(text: &str) -> anyhow::Result<Config> {
        Config::from_table(toml::from_str(text)?)
    }

    fn from_table(table: toml::Table) -> anyhow::Result<Config> {
        let config: Config = table.try_into()?;
        if config.provider.token.is_empty() {
            bail!("provider.token must not be empty");
        }
        if config.agents.is_empty() {
            bail!("agents: at least one agent is required");
        }
        for (name, agent) in &config.agents {
            if !agent.models.contains(&agent.default_model) {
                bail!("agents.{name}: default_model {:?} is not in models", agent.default_model);
            }
        }
        Ok(config)
    }
}

/// Reads `path`, then `<stem>.secrets.toml` beside it if present, whose values win. Tables merge key by key, so a
/// committed `factory.toml` can hold everything but the secrets and the gitignored secrets file the rest.
fn load_table(path: &Path) -> anyhow::Result<toml::Table> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut table: toml::Table = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let secrets = path.with_file_name(format!("{stem}.secrets.toml"));
    match std::fs::read_to_string(&secrets) {
        Ok(text) => merge(&mut table, toml::from_str(&text).with_context(|| format!("parsing {}", secrets.display()))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("reading {}", secrets.display())),
    }
    Ok(table)
}

fn merge(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../../../provider.example.toml");

    #[test]
    fn parses_example() {
        let c = Config::parse(EXAMPLE).unwrap();
        assert_eq!(c.provider.max_workers, 4);
        assert_eq!(c.provider.token, "change-me");
        let agent = &c.agents["claude-code"];
        assert_eq!(agent.image, "ghcr.io/raymondk/software-factory/worker:latest");
        assert_eq!((agent.models.as_slice(), agent.default_model.as_str()), (["sonnet".to_string(), "opus".to_string()].as_slice(), "sonnet"));
        assert_eq!(c.worker_env["GIT_TOKEN"], "change-me");
    }

    #[test]
    fn secrets_file_wins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("provider.toml"), EXAMPLE).unwrap();
        std::fs::write(dir.path().join("provider.secrets.toml"), "[provider]\ntoken = \"real\"\n[worker_env]\nGIT_TOKEN = \"ghp\"\nEXTRA = \"1\"\n").unwrap();
        let c = Config::load(&dir.path().join("provider.toml")).unwrap();
        assert_eq!(c.provider.token, "real");
        assert_eq!((c.worker_env["GIT_TOKEN"].as_str(), c.worker_env["CLAUDE_CODE_OAUTH_TOKEN"].as_str(), c.worker_env["EXTRA"].as_str()), ("ghp", "change-me", "1"));
        assert_eq!(c.agents.len(), 1);
    }

    #[test]
    fn rejects_invalid() {
        assert!(Config::parse("").is_err());
        assert!(Config::parse(&EXAMPLE.replace("token = \"change-me\"", "token = \"\"")).is_err());
        assert!(Config::parse(&format!("{EXAMPLE}\n[typo]\nx = 1\n")).is_err());
        assert!(Config::parse(&EXAMPLE.replace("default_model = \"sonnet\"", "default_model = \"haiku\"")).is_err());
        assert!(Config::parse(&EXAMPLE.replace("[agents.claude-code]", "[agents.claude-code]\nbogus = 1")).is_err());
    }
}
