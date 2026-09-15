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
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Config::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str) -> anyhow::Result<Config> {
        let config: Config = toml::from_str(text)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../../../provider.example.toml");

    #[test]
    fn parses_example() {
        let c = Config::parse(EXAMPLE).unwrap();
        assert_eq!(c.provider.max_workers, 4);
        let agent = &c.agents["claude-code"];
        assert_eq!(agent.image, "software-factory/worker:latest");
        assert_eq!((agent.models.as_slice(), agent.default_model.as_str()), (["sonnet".to_string(), "opus".to_string()].as_slice(), "sonnet"));
        assert_eq!(c.worker_env["GIT_TOKEN"], "change-me");
    }

    #[test]
    fn rejects_invalid() {
        assert!(Config::parse("").is_err());
        assert!(Config::parse(&format!("{EXAMPLE}\n[typo]\nx = 1\n")).is_err());
        assert!(Config::parse(&EXAMPLE.replace("default_model = \"sonnet\"", "default_model = \"haiku\"")).is_err());
        assert!(Config::parse(&EXAMPLE.replace("[agents.claude-code]", "[agents.claude-code]\nbogus = 1")).is_err());
    }
}
