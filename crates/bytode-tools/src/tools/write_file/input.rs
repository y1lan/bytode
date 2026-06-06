use crate::error::{BytodeError, Result};
use serde_json::Value;

pub struct WriteFileInput {
    pub path: String,
    pub content: String,
}

impl WriteFileInput {
    pub fn schema() -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute file path to write"
                },
                "content": {
                    "type": "string",
                    "description": "Complete file content to write"
                },
                "reason": {
                    "type": "string",
                    "description": "Brief explanation of why this change is needed"
                }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        })
    }

    pub fn from_value(args: Value) -> Result<Self> {
        let path = args["path"].as_str().ok_or_else(|| BytodeError::Tool {
            tool: "write_file".into(),
            message: "missing 'path'".into(),
        })?;
        let content = args["content"].as_str().ok_or_else(|| BytodeError::Tool {
            tool: "write_file".into(),
            message: "missing 'content'".into(),
        })?;

        Ok(Self {
            path: path.to_string(),
            content: content.to_string(),
        })
    }
}
