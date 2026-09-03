//! Optional runtime configuration loaded next to the executable.

use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

const CONFIG_FILE: &str = "wiki.yaml";

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct WikiConfig {
    #[serde(default)]
    pub rag: RagConfig,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct RagConfig {
    pub provider: Option<String>,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub timeout_secs: Option<u64>,
    pub embedding: Option<String>,
    pub lexical_weight: Option<f64>,
    pub graph_weight: Option<f64>,
    pub vector_weight: Option<f64>,
    pub max_chars: Option<usize>,
}

#[derive(Debug)]
pub enum ConfigError {
    Io(String),
    Parse(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) | Self::Parse(message) => f.write_str(message),
        }
    }
}

pub fn load_default() -> Result<(WikiConfig, Option<PathBuf>), ConfigError> {
    let executable = std::env::current_exe().ok();
    load_for_executable(executable.as_deref())
}

fn load_for_executable(
    executable: Option<&Path>,
) -> Result<(WikiConfig, Option<PathBuf>), ConfigError> {
    let Some(path) = executable
        .and_then(Path::parent)
        .map(|directory| directory.join(CONFIG_FILE))
    else {
        return Ok((WikiConfig::default(), None));
    };
    if !path.is_file() {
        return Ok((WikiConfig::default(), Some(path)));
    }
    load_from_path(&path).map(|config| (config, Some(path)))
}

pub fn load_from_path(path: &Path) -> Result<WikiConfig, ConfigError> {
    let content = fs::read_to_string(path)
        .map_err(|error| ConfigError::Io(format!("cannot read {}: {error}", path.display())))?;
    serde_yaml::from_str(&content)
        .map_err(|error| ConfigError::Parse(format!("cannot parse {}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_rag_yaml_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(CONFIG_FILE);
        fs::write(
            &path,
            "rag:\n  provider: openai-compatible\n  endpoint: http://127.0.0.1:9000/v1/chat/completions\n  model: local-model\n  api_key_env: TEST_WIKI_KEY\n  timeout_secs: 12\n  max_chars: 4000\n",
        )
        .unwrap();
        let config = load_from_path(&path).unwrap();
        assert_eq!(config.rag.provider.as_deref(), Some("openai-compatible"));
        assert_eq!(config.rag.model.as_deref(), Some("local-model"));
        assert_eq!(config.rag.timeout_secs, Some(12));
        assert_eq!(config.rag.max_chars, Some(4_000));
    }

    #[test]
    fn missing_default_file_is_not_an_error() {
        let (config, path) = load_default().unwrap();
        assert!(config.rag.provider.is_none());
        assert!(path.is_none() || path.unwrap().ends_with(CONFIG_FILE));
    }

    #[test]
    fn default_config_is_loaded_from_executable_directory() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("wiki.exe");
        let config_path = directory.path().join(CONFIG_FILE);
        fs::write(
            &config_path,
            "rag:\n  provider: extractive\n  max_chars: 1234\n",
        )
        .unwrap();

        let (config, path) = load_for_executable(Some(&executable)).unwrap();

        assert_eq!(path, Some(config_path));
        assert_eq!(config.rag.provider.as_deref(), Some("extractive"));
        assert_eq!(config.rag.max_chars, Some(1234));
    }
}
