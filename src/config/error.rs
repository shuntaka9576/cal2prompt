use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Config file not found; please check if '{0}' exists.")]
    ConfigFileNotFoundError(String),

    #[error("Required field '{0}' is not found; please set '{0}' in '{1}'.")]
    RequiredFieldNotFound(String, String),

    #[error("The $HOME environment variable is not set; please set it.")]
    HomeEnvironmentNotFoundError,

    #[error("Lua runtime error: {0}")]
    LuaRuntimeError(String),
}
