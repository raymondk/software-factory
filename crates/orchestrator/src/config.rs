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
    /// Worker providers by name, one or more.
    pub providers: BTreeMap<String, Provider>,
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub url: String,
    /// Sent as a bearer token when starting and stopping workers. Like `orchestrator.token`, never served.
    #[serde(skip_serializing)]
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    #[serde(with = "humantime_serde")]
    pub run_timeout: Duration,
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut config = Config::parse(&text).with_context(|| format!("parsing {}", path.display()))?;
        if config.orchestrator.database.is_none() {
            config.orchestrator.database = Some(path.with_file_name("factory.db"));
        }
        Ok(config)
    }

    pub fn parse(text: &str) -> anyhow::Result<Config> {
        let mut config: Config = toml::from_str(text)?;
        if config.orchestrator.public_url.is_none() {
            let listen = config.orchestrator.listen;
            let host = if listen.ip().is_unspecified() { "localhost".to_string() } else { listen.ip().to_string() };
            config.orchestrator.public_url = Some(format!("http://{host}:{}", listen.port()));
        }
        if config.orchestrator.token.is_empty() {
            bail!("orchestrator.token must not be empty");
        }
        if config.providers.is_empty() {
            bail!("providers: at least one provider is required");
        }
        if let Some(name) = config.providers.iter().find(|(_, p)| p.token.is_empty()).map(|(n, _)| n) {
            bail!("providers.{name}.token must not be empty");
        }
        if let Some(state) = config.prompts.keys().find(|s| !crate::STATES.contains(&s.as_str())) {
            bail!("prompts: unknown state {state:?}");
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../../../factory.example.toml");

    #[test]
    fn parses_example() {
        let c = Config::parse(EXAMPLE).unwrap();
        assert_eq!(c.orchestrator.heartbeat_timeout, Duration::from_secs(60));
        assert_eq!(c.agents["claude-code"].run_timeout, Duration::from_secs(3600));
        assert!(c.prompts.contains_key("ready"));
        assert_eq!(c.orchestrator.public_url.as_deref(), Some("http://localhost:8080"));
        assert_eq!(c.scheduler.interval, Duration::from_secs(10));
        assert_eq!(c.providers["local"].url, "http://localhost:8081");
        assert_eq!(c.providers["local"].token, "change-me");
        let c = Config::parse(&EXAMPLE.replace("listen = \"0.0.0.0:8080\"", "listen = \"10.0.0.5:9000\"\npublic_url = \"http://factory:9000\"")).unwrap();
        assert_eq!(c.orchestrator.public_url.as_deref(), Some("http://factory:9000"));
    }

    #[test]
    fn rejects_invalid() {
        assert!(Config::parse("").is_err());
        assert!(Config::parse(&EXAMPLE.replace("\"60s\"", "\"soon\"")).is_err());
        assert!(Config::parse(&EXAMPLE.replace("token = \"change-me\"", "token = \"\"")).is_err());
        assert!(Config::parse(&format!("{EXAMPLE}\n[typo]\nx = 1\n")).is_err());
        assert!(Config::parse(&EXAMPLE.replace("in_progress = ", "bogus = ")).is_err());
        assert!(Config::parse(&EXAMPLE.replace("[providers.local]\nurl = \"http://localhost:8081\"\n", "")).is_err());
        assert!(Config::parse(&EXAMPLE.replace("token = \"change-me\"   # must match", "token = \"\"   # must match")).is_err());
    }
}
