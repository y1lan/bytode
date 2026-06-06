use super::{ToolEntry, ToolProvider, ToolProviderId};
use crate::lsp::LspClient;
use crate::tools::ToolAvailability;
use crate::tools::cargo::{CargoTool, CheckTool};
use crate::tools::git::{GitCommitTool, GitDiffTool, GitLogTool, GitPushTool, GitStatusTool};
use crate::tools::lsp::DiagnosticsTool;
use crate::tools::read_file::ReadFileTool;
use crate::tools::search_code::SearchTool;
use crate::tools::web::SearchWebTool;
use crate::tools::write_file::WriteFileTool;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct BuiltinToolConfig {
    pub confirm_before_write: bool,
    pub max_file_size: u64,
    pub forbidden_write_patterns: Vec<String>,
    pub ignore_dirs: Vec<String>,
    pub max_results: usize,
    pub extra_check_flags: Vec<String>,
    pub web_timeout_secs: u64,
    pub web_proxy: Option<String>,
}

pub struct BuiltinToolProvider {
    tools: Vec<ToolEntry>,
}

impl BuiltinToolProvider {
    pub fn new(
        project_root: PathBuf,
        config: BuiltinToolConfig,
        lsp: Option<Arc<LspClient>>,
    ) -> Self {
        let diagnostics_lsp = lsp.clone();

        let mut tools = vec![
            ToolEntry::new(
                Box::new(ReadFileTool {
                    project_root: project_root.clone(),
                }),
                ToolAvailability::Always,
            ),
            ToolEntry::new(
                Box::new(WriteFileTool {
                    project_root: project_root.clone(),
                    confirm_before_write: config.confirm_before_write,
                    max_file_size: config.max_file_size,
                    forbidden_patterns: config.forbidden_write_patterns,
                    lsp,
                }),
                ToolAvailability::Always,
            ),
            ToolEntry::new(
                Box::new(SearchTool {
                    project_root: project_root.clone(),
                    ignore_dirs: config.ignore_dirs,
                    max_results: config.max_results,
                }),
                ToolAvailability::Always,
            ),
            ToolEntry::new(
                Box::new(CheckTool {
                    extra_args: config.extra_check_flags,
                }),
                ToolAvailability::PrimaryLanguage {
                    requires: &["rust"],
                },
            ),
        ];

        if let Some(lsp) = diagnostics_lsp {
            tools.push(ToolEntry::new(
                Box::new(DiagnosticsTool { lsp }),
                ToolAvailability::DetectedLanguage {
                    languages: &["rust"],
                },
            ));
        }

        tools.extend([
            ToolEntry::new(
                Box::new(SearchWebTool {
                    timeout_secs: config.web_timeout_secs,
                    proxy: config.web_proxy,
                }),
                ToolAvailability::Always,
            ),
            ToolEntry::new(
                Box::new(CargoTool {
                    project_root: project_root.clone(),
                }),
                ToolAvailability::PrimaryLanguage {
                    requires: &["rust"],
                },
            ),
            ToolEntry::new(
                Box::new(GitStatusTool {
                    project_root: project_root.clone(),
                }),
                ToolAvailability::Always,
            ),
            ToolEntry::new(
                Box::new(GitDiffTool {
                    project_root: project_root.clone(),
                }),
                ToolAvailability::Always,
            ),
            ToolEntry::new(
                Box::new(GitLogTool {
                    project_root: project_root.clone(),
                }),
                ToolAvailability::Always,
            ),
            ToolEntry::new(
                Box::new(GitCommitTool {
                    project_root: project_root.clone(),
                }),
                ToolAvailability::Always,
            ),
            ToolEntry::new(
                Box::new(GitPushTool { project_root }),
                ToolAvailability::Always,
            ),
        ]);

        Self { tools }
    }
}

impl ToolProvider for BuiltinToolProvider {
    fn provider_id(&self) -> ToolProviderId {
        ToolProviderId("builtin")
    }

    fn list_tools(&self) -> Vec<&ToolEntry> {
        self.tools.iter().collect()
    }

    fn into_tools(self: Box<Self>) -> Vec<ToolEntry> {
        self.tools
    }
}
