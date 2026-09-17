//! Settings in `~/.gambit/config.yml`. Nothing has to be set: with an
//! empty file gambit plays you against the `claude` command.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Who moves the black pieces: `claude`, `anthropic`, `openai` or
    /// `command`.
    #[serde(default = "default_opponent")]
    pub opponent: String,

    /// The model to ask for. Empty leaves the choice to the tool itself.
    #[serde(default)]
    pub model: String,

    /// An API key. Empty reads `ANTHROPIC_API_KEY` or `OPENAI_API_KEY`
    /// from the environment instead.
    #[serde(default)]
    pub api_key: String,

    /// Where an OpenAI-shaped API lives: OpenAI itself, OpenRouter, or a
    /// server on your own machine.
    #[serde(default = "default_base_url")]
    pub base_url: String,

    /// A command of your own, for `opponent: command`. It gets the
    /// question on standard input and prints one move.
    #[serde(default)]
    pub command: String,

    /// The side you play: `white` or `black`.
    #[serde(default = "default_side")]
    pub side: String,

    /// A lichess token with the "Play games with the board API" right, from
    /// <https://lichess.org/account/oauth/token/create?scopes[]=board:play>.
    /// Empty reads `LICHESS_TOKEN` from the environment instead.
    #[serde(default)]
    pub lichess_token: String,
}

fn default_opponent() -> String { "claude".into() }
fn default_base_url() -> String { "https://api.openai.com/v1".into() }
fn default_side() -> String { "white".into() }

impl Default for Config {
    fn default() -> Self {
        Config {
            opponent: default_opponent(),
            model: String::new(),
            api_key: String::new(),
            base_url: default_base_url(),
            command: String::new(),
            side: default_side(),
            lichess_token: String::new(),
        }
    }
}

pub fn dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".gambit")
}

pub fn path() -> PathBuf { dir().join("config.yml") }

pub fn load() -> Config {
    std::fs::read_to_string(path()).ok()
        .and_then(|s| serde_yaml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(cfg: &Config) -> Result<(), String> {
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    let text = serde_yaml::to_string(cfg).map_err(|e| e.to_string())?;
    std::fs::write(path(), text).map_err(|e| e.to_string())
}
