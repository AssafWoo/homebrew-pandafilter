use anyhow::Result;
use panda_core::config::CcrConfig;

const DEFAULT_CONFIG: &str = include_str!("../../config/default_filters.toml");

pub fn load_config() -> Result<CcrConfig> {
    // 1. Try ./panda.toml
    if let Ok(content) = std::fs::read_to_string("panda.toml") {
        if let Ok(cfg) = toml::from_str(&content) {
            return Ok(cfg);
        }
    }

    // 2. Try ~/.config/panda/config.toml
    if let Some(config_dir) = dirs::config_dir() {
        let path = config_dir.join("panda").join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = toml::from_str(&content) {
                return Ok(cfg);
            }
        }
    }

    // 3. Embedded default
    Ok(toml::from_str(DEFAULT_CONFIG)?)
}
