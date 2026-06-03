use crate::config::Config;
use crate::error::{BytodeError, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    Go,
    Python,
    Java,
    Cpp,
    Unknown,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::Rust => write!(f, "Rust"),
            Language::JavaScript => write!(f, "JavaScript"),
            Language::TypeScript => write!(f, "TypeScript"),
            Language::Go => write!(f, "Go"),
            Language::Python => write!(f, "Python"),
            Language::Java => write!(f, "Java"),
            Language::Cpp => write!(f, "C/C++"),
            Language::Unknown => write!(f, "Unknown"),
        }
    }
}

impl Language {
    pub fn to_str(&self) -> &str {
        match self {
            Language::Rust => "rust",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Go => "go",
            Language::Python => "python",
            Language::Java => "java",
            Language::Cpp => "cpp",
            Language::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub enum BuildSystem {
    Cargo,
    Npm,
    GoMod,
    Pip,
    Maven,
    CMake,
    Unknown,
}

impl std::fmt::Display for BuildSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildSystem::Cargo => write!(f, "cargo"),
            BuildSystem::Npm => write!(f, "npm"),
            BuildSystem::GoMod => write!(f, "go"),
            BuildSystem::Pip => write!(f, "pip"),
            BuildSystem::Maven => write!(f, "maven"),
            BuildSystem::CMake => write!(f, "cmake"),
            BuildSystem::Unknown => write!(f, "unknown"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum TestFramework {
    CargoTest,
}

impl std::fmt::Display for TestFramework {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestFramework::CargoTest => write!(f, "cargo test"),
        }
    }
}

pub struct Marker {
    pub file: &'static str,
    pub language: Language,
    pub build_system: BuildSystem,
    pub content_check: Option<fn(&str) -> bool>,
    pub priority: u8,
}

const MARKERS: &[Marker] = &[
    Marker {
        file: "Cargo.toml",
        language: Language::Rust,
        build_system: BuildSystem::Cargo,
        content_check: Some(|c| c.contains("[package]") || c.contains("[workspace]")),
        priority: 100,
    },
    Marker {
        file: "go.mod",
        language: Language::Go,
        build_system: BuildSystem::GoMod,
        content_check: Some(|c| c.starts_with("module ")),
        priority: 90,
    },
    Marker {
        file: "package.json",
        language: Language::JavaScript,
        build_system: BuildSystem::Npm,
        content_check: Some(|c| c.contains("\"name\"")),
        priority: 80,
    },
    Marker {
        file: "pyproject.toml",
        language: Language::Python,
        build_system: BuildSystem::Pip,
        content_check: Some(|c| c.contains("[project]") || c.contains("[tool.poetry]")),
        priority: 70,
    },
    Marker {
        file: "CMakeLists.txt",
        language: Language::Cpp,
        build_system: BuildSystem::CMake,
        content_check: None,
        priority: 60,
    },
    Marker {
        file: "pom.xml",
        language: Language::Java,
        build_system: BuildSystem::Maven,
        content_check: Some(|c| c.contains("<project")),
        priority: 50,
    },
];

#[derive(Debug, Clone)]
pub struct ProjectProfile {
    pub primary: Language,
    pub all_languages: Vec<Language>,
    pub build_system: BuildSystem,
    pub test_framework: Option<TestFramework>,
    pub root: PathBuf,
    pub source_dirs: Vec<PathBuf>,
    pub is_workspace: bool,
    pub workspace_members: Vec<PathBuf>,
}

impl ProjectProfile {
    pub fn detect(root: &Path, config: &Config) -> Result<Self> {
        if let Some(lang_str) = &config.project_lang_override {
            return Self::from_override(root, lang_str);
        }

        let entries: HashSet<String> = std::fs::read_dir(root)
            .map_err(|e| BytodeError::ProjectDetection(format!("read_dir: {}", e)))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        let mut matches: Vec<&Marker> = MARKERS
            .iter()
            .filter(|m| entries.contains(m.file))
            .collect();
        matches.sort_by_key(|m| std::cmp::Reverse(m.priority));

        let confirmed: Vec<&Marker> = matches
            .iter()
            .filter(|m| {
                if let Some(check) = m.content_check {
                    let path = root.join(m.file);
                    std::fs::read_to_string(&path)
                        .map(|c| check(&c))
                        .unwrap_or(false)
                } else {
                    true
                }
            })
            .copied()
            .collect();

        let primary_marker = confirmed.first().ok_or_else(|| {
            BytodeError::ProjectDetection(format!("no project markers found in {}", root.display()))
        })?;

        let all_languages: Vec<Language> = {
            let mut langs: Vec<_> = confirmed.iter().map(|m| m.language).collect();
            langs.dedup();
            langs
        };

        let is_workspace = Self::check_workspace(root, primary_marker);
        let workspace_members = if is_workspace {
            Self::find_workspace_members(root, primary_marker)
        } else {
            vec![]
        };
        let source_dirs = Self::find_source_dirs(root, primary_marker.language);
        let test_framework = Self::detect_test_framework(root, primary_marker.language);

        Ok(ProjectProfile {
            primary: primary_marker.language,
            all_languages,
            build_system: primary_marker.build_system.clone(),
            test_framework,
            root: root.to_path_buf(),
            source_dirs,
            is_workspace,
            workspace_members,
        })
    }

    fn from_override(root: &Path, lang: &str) -> Result<Self> {
        let (language, build_system) = match lang.to_lowercase().as_str() {
            "rust" => (Language::Rust, BuildSystem::Cargo),
            "go" => (Language::Go, BuildSystem::GoMod),
            "javascript" | "js" => (Language::JavaScript, BuildSystem::Npm),
            "python" | "py" => (Language::Python, BuildSystem::Pip),
            "java" => (Language::Java, BuildSystem::Maven),
            "cpp" | "c++" => (Language::Cpp, BuildSystem::CMake),
            other => {
                return Err(BytodeError::ProjectDetection(format!(
                    "unsupported language override: {}",
                    other
                )));
            }
        };

        Ok(ProjectProfile {
            primary: language,
            all_languages: vec![language],
            build_system,
            test_framework: if matches!(language, Language::Rust) {
                Some(TestFramework::CargoTest)
            } else {
                None
            },
            root: root.to_path_buf(),
            source_dirs: vec![root.join("src")],
            is_workspace: false,
            workspace_members: vec![],
        })
    }

    fn check_workspace(root: &Path, marker: &Marker) -> bool {
        match marker.language {
            Language::Rust => {
                let cargo_toml = root.join("Cargo.toml");
                std::fs::read_to_string(&cargo_toml)
                    .map(|c| c.contains("[workspace]"))
                    .unwrap_or(false)
            }
            Language::JavaScript => {
                let pkg = root.join("package.json");
                std::fs::read_to_string(&pkg)
                    .map(|c| c.contains("\"workspaces\""))
                    .unwrap_or(false)
            }
            _ => false,
        }
    }

    fn find_workspace_members(root: &Path, marker: &Marker) -> Vec<PathBuf> {
        match marker.language {
            Language::Rust => {
                let cargo_toml = root.join("Cargo.toml");
                let content = match std::fs::read_to_string(&cargo_toml) {
                    Ok(c) => c,
                    Err(_) => return vec![],
                };
                Self::parse_cargo_workspace_members(&content, root)
            }
            _ => vec![],
        }
    }

    fn parse_cargo_workspace_members(content: &str, root: &Path) -> Vec<PathBuf> {
        let doc: toml::Value = match toml::from_str(content) {
            Ok(v) => v,
            Err(_) => return vec![],
        };

        let members = match doc
            .get("workspace")
            .and_then(|w| w.get("members"))
            .and_then(|m| m.as_array())
        {
            Some(m) => m,
            None => return vec![],
        };

        members
            .iter()
            .filter_map(|m| m.as_str())
            .flat_map(|pattern| {
                if pattern.contains('*') {
                    if let Some((prefix, _)) = pattern.split_once('*') {
                        let dir = root.join(prefix);
                        if dir.is_dir() {
                            std::fs::read_dir(&dir)
                                .ok()
                                .map(|entries| {
                                    entries
                                        .filter_map(|e| e.ok())
                                        .filter(|e| {
                                            e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                                        })
                                        .filter(|e| e.path().join("Cargo.toml").exists())
                                        .map(|e| e.path())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default()
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    }
                } else {
                    let member = root.join(pattern);
                    if member.join("Cargo.toml").exists() {
                        vec![member]
                    } else {
                        vec![]
                    }
                }
            })
            .collect()
    }

    fn find_source_dirs(root: &Path, language: Language) -> Vec<PathBuf> {
        let candidates: &[&str] = match language {
            Language::Rust => &["src", "tests", "examples", "benches"],
            Language::JavaScript | Language::TypeScript => &["src", "lib", "app"],
            Language::Go => &["cmd", "pkg", "internal"],
            Language::Python => &["src", "tests"],
            Language::Java => &["src/main/java", "src/test/java"],
            _ => &["src"],
        };

        candidates
            .iter()
            .map(|d| root.join(d))
            .filter(|p| p.is_dir())
            .collect()
    }

    fn detect_test_framework(root: &Path, language: Language) -> Option<TestFramework> {
        match language {
            Language::Rust => {
                if root.join("tests").is_dir() {
                    return Some(TestFramework::CargoTest);
                }
                let cargo = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
                if cargo.contains("[dev-dependencies]")
                    || cargo.contains("rstest")
                    || cargo.contains("proptest")
                {
                    return Some(TestFramework::CargoTest);
                }
                None
            }
            _ => None,
        }
    }

    pub fn snapshot(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "Project: {} (build: {})\n",
            self.primary, self.build_system
        ));
        s.push_str(&format!("Root: {}\n", self.root.display()));

        if !self.source_dirs.is_empty() {
            let dirs: Vec<_> = self
                .source_dirs
                .iter()
                .map(|d| d.file_name().unwrap().to_string_lossy())
                .collect();
            s.push_str(&format!("Source: {}\n", dirs.join(", ")));
        }

        if self.is_workspace {
            let members: Vec<_> = self
                .workspace_members
                .iter()
                .map(|m| m.file_name().unwrap().to_string_lossy())
                .collect();
            s.push_str(&format!("Workspace members: {}\n", members.join(", ")));
        }

        if let Some(ref tf) = self.test_framework {
            s.push_str(&format!("Test: {}\n", tf));
        }

        s.push_str("Build output: JSON only. Diagnostics: LSP only.\n");
        s
    }

    pub fn supports_lsp(&self) -> bool {
        matches!(self.primary, Language::Rust)
    }
}
