use crate::error::{BytodeError, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Tool selection policy
#[derive(Debug, Clone)]
pub enum ToolSelection {
    /// User provided an exact list — use it verbatim
    Exact { enabled: Vec<String> },
    /// User extended or trimmed the defaults
    Additive { enable: Vec<String>, disable: Vec<String> },
    /// No user config — use project-detected defaults
    None,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawConfig {
    pub project: Option<RawProjectConfig>,
    pub build: Option<RawBuildConfig>,
    pub tools: Option<RawToolsConfig>,
    pub agent: Option<RawAgentConfig>,
    pub security: Option<RawSecurityConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawProjectConfig {
    pub lang: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawBuildConfig {
    #[serde(default)]
    pub extra_check_flags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawToolsConfig {
    pub enabled: Option<Vec<String>>,
    pub enable: Option<Vec<String>>,
    pub disable: Option<Vec<String>>,
    #[serde(default)]
    pub search: RawSearchConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawSearchConfig {
    #[serde(default = "default_search_engine")]
    pub engine: String,
    #[serde(default = "default_ignore_dirs")]
    pub ignore_dirs: Vec<String>,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

impl Default for RawSearchConfig {
    fn default() -> Self {
        RawSearchConfig {
            engine: default_search_engine(),
            ignore_dirs: default_ignore_dirs(),
            max_results: default_max_results(),
        }
    }
}

fn default_search_engine() -> String { "ripgrep".into() }
fn default_ignore_dirs() -> Vec<String> { vec!["target".into(), ".git".into()] }
fn default_max_results() -> usize { 200 }

#[derive(Debug, Clone, Deserialize)]
pub struct RawAgentConfig {
    #[serde(default = "default_true")]
    pub confirm_before_write: bool,
    #[serde(default = "default_max_consecutive")]
    pub max_consecutive_calls: u32,
    #[serde(default = "default_true")]
    pub auto_check_after_write: bool,
}

fn default_true() -> bool { true }
fn default_max_consecutive() -> u32 { 20 }

#[derive(Debug, Clone, Deserialize)]
pub struct RawSecurityConfig {
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,
    #[serde(default = "default_forbidden")]
    pub forbidden_write_patterns: Vec<String>,
}

fn default_max_file_size() -> u64 { 1_048_576 }
fn default_forbidden() -> Vec<String> { vec!["/etc/*".into()] }

#[derive(Debug, Clone)]
pub struct Config {
    pub project_lang_override: Option<String>,
    pub search: SearchConfig,
    pub agent: AgentConfig,
    pub security: SecurityConfig,
    pub build: BuildConfig,
    pub tools: ToolSelection,
}

#[derive(Debug, Clone)]
pub struct SearchConfig {
    pub engine: String,
    pub ignore_dirs: Vec<String>,
    pub max_results: usize,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub confirm_before_write: bool,
    pub max_consecutive_calls: u32,
    pub auto_check_after_write: bool,
}

#[derive(Debug, Clone)]
pub struct SecurityConfig {
    pub max_file_size: u64,
    pub forbidden_write_patterns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub extra_check_flags: Vec<String>,
}

impl Config {
    pub fn load(project_root: &Path) -> Result<Self> {
        let mut config = Config::default();

        // Layer 2: user config (~/.config/bytode.toml)
        if let Some(user_path) = user_config_path() {
            if user_path.exists() {
                if let Some(raw) = Self::read_file(&user_path)? {
                    config.merge(raw);
                }
            }
        }

        // Layer 3: project config (<root>/.bytode.toml)
        let project_config_path = project_root.join(".bytode.toml");
        if project_config_path.exists() {
            if let Some(raw) = Self::read_file(&project_config_path)? {
                config.merge(raw);
            }
        }

        config.apply_hard_constraints();
        Ok(config)
    }

    fn read_file(path: &Path) -> Result<Option<RawConfig>> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(BytodeError::Config(format!("read {}: {}", path.display(), e))),
        };
        let raw: RawConfig = toml::from_str(&content)
            .map_err(|e| BytodeError::Config(format!("parse {}: {}", path.display(), e)))?;
        Ok(Some(raw))
    }

    fn merge(&mut self, raw: RawConfig) {
        if let Some(p) = raw.project {
            if let Some(lang) = p.lang { self.project_lang_override = Some(lang); }
        }
        if let Some(b) = raw.build {
            self.build.extra_check_flags = b.extra_check_flags;
        }
        if let Some(t) = raw.tools {
            if let Some(enabled) = t.enabled {
                self.tools = ToolSelection::Exact { enabled };
            } else {
                let enable = t.enable.unwrap_or_default();
                let disable = t.disable.unwrap_or_default();
                if !enable.is_empty() || !disable.is_empty() {
                    self.tools = ToolSelection::Additive { enable, disable };
                }
            }
            self.search.engine = t.search.engine;
            self.search.ignore_dirs = t.search.ignore_dirs;
            self.search.max_results = t.search.max_results;
        }
        if let Some(a) = raw.agent {
            self.agent = AgentConfig {
                confirm_before_write: a.confirm_before_write,
                max_consecutive_calls: a.max_consecutive_calls,
                auto_check_after_write: a.auto_check_after_write,
            };
        }
        if let Some(s) = raw.security {
            self.security.max_file_size = s.max_file_size;
            self.security.forbidden_write_patterns.extend(s.forbidden_write_patterns);
        }
    }

    fn apply_hard_constraints(&mut self) {
        let sys_patterns = vec!["/etc/*".into(), "/proc/*".into(), "/sys/*".into()];
        for p in sys_patterns {
            if !self.security.forbidden_write_patterns.contains(&p) {
                self.security.forbidden_write_patterns.push(p);
            }
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            project_lang_override: None,
            search: SearchConfig {
                engine: "ripgrep".into(),
                ignore_dirs: vec!["target".into(), ".git".into()],
                max_results: 200,
            },
            agent: AgentConfig {
                confirm_before_write: true,
                max_consecutive_calls: 20,
                auto_check_after_write: true,
            },
            security: SecurityConfig {
                max_file_size: 1_048_576,
                forbidden_write_patterns: vec!["/etc/*".into()],
            },
            build: BuildConfig {
                extra_check_flags: vec!["--all-targets".into()],
            },
            tools: ToolSelection::None,
        }
    }
}

fn user_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("bytode.toml"))
}
