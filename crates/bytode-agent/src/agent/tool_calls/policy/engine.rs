use super::decision::PolicyDecision;
use super::rule::PLAN_MODE_DENIED_CAPABILITIES;
use crate::agent::AgentMode;
use crate::agent::tool_calls::{ToolCallContext, ToolCallRequest};
use crate::tools::ToolCapability;
use std::path::{Path, PathBuf};

pub struct ToolPolicyEngine;

impl ToolPolicyEngine {
    pub fn evaluate(&self, ctx: &ToolCallContext<'_>, request: &ToolCallRequest) -> PolicyDecision {
        if matches!(ctx.mode, AgentMode::Plan)
            && request
                .descriptor
                .capabilities
                .iter()
                .any(|capability| PLAN_MODE_DENIED_CAPABILITIES.contains(capability))
        {
            return PolicyDecision::Deny(format!(
                "{} is blocked in plan mode",
                request.descriptor.name
            ));
        }

        match request.descriptor.name {
            "read_file" | "search_code" | "get_diagnostics" | "git_status" | "git_log" => {
                PolicyDecision::Allow
            }
            "write_file" => match validate_write_path(
                &ctx.profile.root,
                &request.call.arguments,
                ctx.forbidden_write_patterns,
            ) {
                Ok(()) => PolicyDecision::Ask("write access requires approval".into()),
                Err(reason) => PolicyDecision::Deny(reason),
            },
            "git_diff" | "cargo_check" | "cargo" | "web_search" => {
                PolicyDecision::Ask(format!("{} requires approval", request.descriptor.name))
            }
            name if request
                .descriptor
                .capabilities
                .iter()
                .any(|capability| matches!(capability, ToolCapability::VcsWrite)) =>
            {
                PolicyDecision::Ask(format!("{name} requires approval"))
            }
            _ => PolicyDecision::Deny(format!(
                "{} is not allowed by the current policy",
                request.descriptor.name
            )),
        }
    }
}

fn validate_write_path(
    project_root: &Path,
    arguments: &serde_json::Value,
    forbidden_patterns: &[String],
) -> Result<(), String> {
    let path = arguments
        .get("path")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "write_file is missing a path".to_string())?;

    let resolved = resolve_write_target(project_root, path)?;
    if !resolved.starts_with(project_root) {
        return Err("write path is outside project root".into());
    }

    let resolved_str = resolved.to_string_lossy();
    if forbidden_patterns.iter().any(|pattern| {
        let prefix = pattern.trim_end_matches('*');
        resolved_str.contains(prefix)
    }) {
        return Err("write path matches forbidden_write_patterns".into());
    }

    Ok(())
}

fn resolve_write_target(project_root: &Path, path: &str) -> Result<PathBuf, String> {
    let raw = PathBuf::from(path);
    let resolved = if raw.is_absolute() {
        raw
    } else {
        project_root.join(raw)
    };

    if resolved.exists() {
        return std::fs::canonicalize(&resolved)
            .map_err(|error| format!("cannot resolve write path: {error}"));
    }

    let parent = resolved
        .parent()
        .ok_or_else(|| "write path has no parent directory".to_string())?;
    let canonical_parent = std::fs::canonicalize(parent)
        .map_err(|error| format!("cannot resolve parent directory: {error}"))?;
    let file_name = resolved
        .file_name()
        .ok_or_else(|| "write path has no file name".to_string())?;
    Ok(canonical_parent.join(file_name))
}
