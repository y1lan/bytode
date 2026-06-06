use crate::error::{BytodeError, Result};
use serde_json::Value;

pub struct SearchCodeInput {
    pub pattern: String,
    pub path: Option<String>,
}

impl SearchCodeInput {
    pub fn schema() -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Ripgrep-compatible regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in. Omit to search entire project.",
                    "nullable": true
                }
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    pub fn from_value(args: Value) -> Result<Self> {
        let pattern = args["pattern"].as_str().ok_or_else(|| BytodeError::Tool {
            tool: "search_code".into(),
            message: "missing 'pattern'".into(),
        })?;

        Ok(Self {
            pattern: pattern.to_string(),
            path: args["path"].as_str().map(str::to_string),
        })
    }
}
