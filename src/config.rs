use serde::Deserialize;
use std::fmt;

#[derive(Deserialize)]
pub struct Config {
    pub device_ip: String,
    pub device_id: String,
    pub local_key: String,
}

#[derive(Debug)]
pub enum ConfigError {
    FileNotFound(String),
    ParseError(String),
    InvalidLocalKey,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::FileNotFound(path) => write!(f, "Config file not found: {path}"),
            ConfigError::ParseError(msg) => write!(f, "Failed to parse config: {msg}"),
            ConfigError::InvalidLocalKey => write!(f, "local_key must be exactly 16 characters"),
        }
    }
}

impl std::error::Error for ConfigError {}

pub fn load_config(path: &str) -> Result<Config, ConfigError> {
    let contents = std::fs::read_to_string(path)
        .map_err(|_| ConfigError::FileNotFound(path.to_owned()))?;

    let config: Config = toml::from_str(&contents)
        .map_err(|e| ConfigError::ParseError(e.to_string()))?;

    if config.local_key.len() != 16 {
        return Err(ConfigError::InvalidLocalKey);
    }

    Ok(config)
}
