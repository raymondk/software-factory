use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub project: Project,
    pub orchestrator: Orchestrator,
    pub scheduler: Scheduler,
    pub agents: BTreeMap<String, Agent>,
    /// Prompt template per workable state, shared by all agents.
    pub prompts: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
    pub repos: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Orchestrator {
    pub listen: SocketAddr,
    /// Never leaves the process: `GET /config` serves everything but this.
    #[serde(skip_serializing)]
    pub token: String,
    #[serde(with = "humantime_serde")]
    pub heartbeat_timeout: Duration,
    /// Worker log lines older than this are purged: a run's lines once it ended that long ago, run-less lines by
    /// age. Runs themselves are kept. Defaults to 7 days.
    #[serde(default = "default_log_retention", with = "humantime_serde")]
    pub log_retention: Duration,
    /// SQLite file. Defaults to `factory.db` next to the config file.
    pub database: Option<PathBuf>,
    /// How workers reach this orchestrator. Defaults to `http://<listen host or localhost>:<port>`.
    pub public_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scheduler {
    pub max_workers: u32,
    #[serde(default = "default_interval", with = "humantime_serde")]
    pub interval: Duration,
}

fn default_interval() -> Duration {
    Duration::from_secs(10)
}

fn default_log_retention() -> Duration {
    Duration::from_secs(7 * 24 * 3600)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    #[serde(with = "humantime_serde")]
    pub run_timeout: Duration,
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let mut config = Config::from_table(load_table(path)?).with_context(|| format!("in {}", path.display()))?;
        if config.orchestrator.database.is_none() {
            config.orchestrator.database = Some(path.with_file_name("factory.db"));
        }
        Ok(config)
    }

    pub fn parse(text: &str) -> anyhow::Result<Config> {
        Config::from_table(toml::from_str(text)?)
    }

    fn from_table(table: toml::Table) -> anyhow::Result<Config> {
        let mut config: Config = table.try_into()?;
        if config.orchestrator.public_url.is_none() {
            let listen = config.orchestrator.listen;
            let host = if listen.ip().is_unspecified() { "localhost".to_string() } else { listen.ip().to_string() };
            config.orchestrator.public_url = Some(format!("http://{host}:{}", listen.port()));
        }
        if config.orchestrator.token.is_empty() {
            bail!("orchestrator.token must not be empty");
        }
        if let Some(state) = config.prompts.keys().find(|s| !crate::STATES.contains(&s.as_str())) {
            bail!("prompts: unknown state {state:?}");
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

    impl Config {
        fn log_retention_days(&self) -> u64 {
            self.orchestrator.log_retention.as_secs() / 86400
        }
    }

    const EXAMPLE: &str = include_str!("../../../factory.example.toml");

    #[test]
    fn parses_example() {
        let c = Config::parse(EXAMPLE).unwrap();
        assert_eq!(c.orchestrator.heartbeat_timeout, Duration::from_secs(60));
        assert_eq!(c.agents["claude-code"].run_timeout, Duration::from_secs(3600));
        assert!(c.prompts.contains_key("ready"));
        assert_eq!(c.orchestrator.public_url.as_deref(), Some("http://localhost:8080"));
        assert_eq!(c.scheduler.interval, Duration::from_secs(10));
        assert_eq!(c.log_retention_days(), 7);
        assert_eq!(Config::parse(&EXAMPLE.replace("heartbeat_timeout = \"60s\"", "heartbeat_timeout = \"60s\"\nlog_retention = \"2days\"")).unwrap().log_retention_days(), 2);
        let c = Config::parse(&EXAMPLE.replace("listen = \"0.0.0.0:8080\"", "listen = \"10.0.0.5:9000\"\npublic_url = \"http://factory:9000\"")).unwrap();
        assert_eq!(c.orchestrator.public_url.as_deref(), Some("http://factory:9000"));
    }

    #[test]
    fn secrets_file_wins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("factory.toml"), EXAMPLE).unwrap();
        std::fs::write(dir.path().join("factory.secrets.toml"), "[orchestrator]\ntoken = \"real\"\n").unwrap();
        let c = Config::load(&dir.path().join("factory.toml")).unwrap();
        assert_eq!((c.orchestrator.token.as_str(), c.scheduler.max_workers), ("real", 4));
        std::fs::write(dir.path().join("factory.secrets.toml"), "[orchestrator]\nbogus = 1\n").unwrap();
        assert!(Config::load(&dir.path().join("factory.toml")).is_err());
    }

    #[test]
    fn rejects_invalid() {
        assert!(Config::parse("").is_err());
        assert!(Config::parse(&EXAMPLE.replace("\"60s\"", "\"soon\"")).is_err());
        assert!(Config::parse(&EXAMPLE.replace("token = \"change-me\"", "token = \"\"")).is_err());
        assert!(Config::parse(&format!("{EXAMPLE}\n[typo]\nx = 1\n")).is_err());
        assert!(Config::parse(&EXAMPLE.replace("in_progress = ", "bogus = ")).is_err());
    }
}
