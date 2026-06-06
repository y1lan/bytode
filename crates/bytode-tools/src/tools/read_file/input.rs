use crate::error::{BytodeError, Result};
use serde_json::Value;

pub struct ReadFileInput {
    pub path: String,
    pub offset: usize,
    pub limit: Option<usize>,
}

impl ReadFileInput {
    pub fn schema() -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute file or directory path. Files return numbered content, directories return a listing."
                },
                "offset": {
                    "type": "integer",
                    "description": "1-based line number to start from. Omit to read from beginning.",
                    "minimum": 1,
                    "nullable": true
                },
                "limit": {
                    "type": "integer",
                    "description": "Max lines to return. Omit to read all. Max 500.",
                    "minimum": 1,
                    "maximum": 500,
                    "nullable": true
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    pub fn from_value(args: Value) -> Result<Self> {
        let path = args["path"].as_str().ok_or_else(|| BytodeError::Tool {
            tool: "read_file".into(),
            message: "missing 'path' argument".into(),
        })?;

        Ok(Self {
            path: path.to_string(),
            offset: args["offset"].as_u64().unwrap_or(1) as usize,
            limit: args["limit"].as_u64().map(|value| value as usize),
        })
    }
}
