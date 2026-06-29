use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
  #[error("{0}")]
  Message(String),
  #[error("io error at {path}: {source}")]
  Io {
    path: String,
    #[source]
    source: std::io::Error,
  },
  #[error("request error: {0}")]
  Request(#[from] reqwest::Error),
  #[error("json error at {path}: {source}")]
  Json {
    path: String,
    #[source]
    source: serde_json::Error,
  },
  #[error("yaml error at {path}: {source}")]
  Yaml {
    path: String,
    #[source]
    source: serde_yaml::Error,
  },
  #[error("toml parse error at {path}: {source}")]
  Toml {
    path: String,
    #[source]
    source: toml::de::Error,
  },
  #[error("toml serialize error: {0}")]
  TomlSerialize(#[from] toml::ser::Error),
}

impl AppError {
  pub fn io(path: &Path, source: std::io::Error) -> Self {
    Self::Io {
      path: path.display().to_string(),
      source,
    }
  }

  pub fn json(path: &Path, source: serde_json::Error) -> Self {
    Self::Json {
      path: path.display().to_string(),
      source,
    }
  }

  pub fn yaml(path: &Path, source: serde_yaml::Error) -> Self {
    Self::Yaml {
      path: path.display().to_string(),
      source,
    }
  }
}
