use thiserror::Error;

#[derive(Error, Debug)]
pub enum BytodeError {
    #[error("config: {0}")]
    Config(String),

    #[error("project detection: {0}")]
    ProjectDetection(String),

    #[error("LSP: {0}")]
    Lsp(String),

    #[error("LLM: {0}")]
    Llm(String),

    #[error("tool '{tool}': {message}")]
    Tool { tool: String, message: String },

    #[error("IO: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, BytodeError>;
