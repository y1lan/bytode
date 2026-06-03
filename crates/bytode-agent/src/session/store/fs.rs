//! Session path computation, directory layout, and crash-safe artifact IO.

use crate::error::{BytodeError, Result};
use crate::session::model::{ArtifactId, ArtifactKind, ArtifactRef, EntryMeta, SessionId};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

// Inline limits applied when an entry is first recorded. Payloads larger than
// these are archived immediately; the entry keeps a preview + ArtifactRef.
pub const TOOL_OUTPUT_INLINE_LIMIT: usize = 16 * 1024;
pub const COMMAND_OUTPUT_INLINE_LIMIT: usize = 16 * 1024;
pub const FILE_SNAPSHOT_INLINE_LIMIT: usize = 32 * 1024;
pub const DIFF_INLINE_LIMIT: usize = 32 * 1024;
pub const DIAGNOSTICS_INLINE_LIMIT: usize = 16 * 1024;
pub const TOOL_ARGS_INLINE_LIMIT: usize = 16 * 1024;

/// Record-time inline limit for a payload of the given kind.
pub fn record_inline_limit(kind: ArtifactKind) -> usize {
    match kind {
        ArtifactKind::ToolArgs => TOOL_ARGS_INLINE_LIMIT,
        ArtifactKind::CommandOutput => COMMAND_OUTPUT_INLINE_LIMIT,
        ArtifactKind::FileSnapshot => FILE_SNAPSHOT_INLINE_LIMIT,
        ArtifactKind::Diff => DIFF_INLINE_LIMIT,
        ArtifactKind::Diagnostics => DIAGNOSTICS_INLINE_LIMIT,
        ArtifactKind::ToolOutput | ArtifactKind::SearchResult | ArtifactKind::CompactArchive => {
            TOOL_OUTPUT_INLINE_LIMIT
        }
    }
}

const PREVIEW_MAX_CHARS: usize = 800;
const PREVIEW_MAX_LINES: usize = 20;

/// Stable excerpt of a payload, used as the in-log preview.
pub fn make_preview(content: &str) -> String {
    let mut out = String::new();
    let mut lines = 0;
    for ch in content.chars() {
        if out.chars().count() >= PREVIEW_MAX_CHARS || lines >= PREVIEW_MAX_LINES {
            out.push_str("\n...");
            break;
        }
        if ch == '\n' {
            lines += 1;
        }
        out.push(ch);
    }
    out
}

/// sha256 hex digest of the given bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Root for all sessions: `<state_dir>/bytode/sessions/`. Errors when the
/// platform has no state directory — no fallback to home.
pub fn sessions_root() -> Result<PathBuf> {
    let state = dirs::state_dir()
        .ok_or_else(|| BytodeError::Session("no state_dir on this platform".into()))?;
    Ok(state.join("bytode").join("sessions"))
}

/// Stable 16-hex hash of the project root path.
pub fn project_hash(project_root: &Path) -> String {
    let digest = sha256_hex(project_root.to_string_lossy().as_bytes());
    digest[..16].to_string()
}

/// Default per-project session id: `project-<hash>`.
pub fn default_session_id(project_root: &Path) -> SessionId {
    SessionId(format!("project-{}", project_hash(project_root)))
}

/// New REPL session id: `project-<hash>-<unix_secs_hex>`.
pub fn new_session_id(project_root: &Path, unix_secs: u64) -> SessionId {
    SessionId(format!(
        "project-{}-{:x}",
        project_hash(project_root),
        unix_secs
    ))
}

/// Absolute session root for a given session id.
pub fn session_root(session_id: &SessionId) -> Result<PathBuf> {
    Ok(sessions_root()?.join(session_id.as_str()))
}

/// Create the fixed session directory tree.
pub fn ensure_session_tree(session_root: &Path) -> Result<()> {
    std::fs::create_dir_all(session_root.join("events"))?;
    let artifacts = session_root.join("artifacts");
    for sub in [
        "args",
        "tool",
        "command",
        "file",
        "diff",
        "diagnostics",
        "search",
        "compact",
    ] {
        std::fs::create_dir_all(artifacts.join(sub))?;
    }
    Ok(())
}

/// Reads and writes artifact payloads under a session root.
pub struct ArtifactStore {
    session_root: PathBuf,
}

impl ArtifactStore {
    pub fn new(session_root: PathBuf) -> Self {
        ArtifactStore { session_root }
    }

    /// Crash-safe write: temp file -> flush -> rename, before the caller appends
    /// the referencing log entry. Content addressing makes repeated identical
    /// payloads collapse to one file.
    pub fn write(
        &self,
        meta: &EntryMeta,
        kind: ArtifactKind,
        content: &str,
    ) -> Result<ArtifactRef> {
        let bytes = content.as_bytes();
        let sha = sha256_hex(bytes);
        let relative_path = PathBuf::from("artifacts")
            .join(kind.subdir())
            .join(format!("{sha}.txt"));
        let abs = self.session_root.join(&relative_path);
        let dir = abs
            .parent()
            .ok_or_else(|| BytodeError::Session("artifact path has no parent".into()))?;
        std::fs::create_dir_all(dir)?;

        let tmp = dir.join(format!(".{sha}.tmp"));
        {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(bytes)?;
            file.flush()?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, &abs)?;

        Ok(ArtifactRef {
            id: ArtifactId(sha.clone()),
            kind,
            relative_path,
            source_entry_id: meta.id.clone(),
            byte_len: bytes.len() as u64,
            sha256: sha,
            preview: make_preview(content),
        })
    }

    fn abs_path(&self, art: &ArtifactRef) -> PathBuf {
        self.session_root.join(&art.relative_path)
    }

    /// Whether the artifact file exists on disk.
    pub fn exists(&self, art: &ArtifactRef) -> bool {
        self.abs_path(art).exists()
    }

    /// Read raw payload, verifying the sha256. Returns a diagnostic error on
    /// missing file or checksum mismatch instead of panicking.
    pub fn read_verified(&self, art: &ArtifactRef) -> Result<String> {
        let path = self.abs_path(art);
        let bytes = std::fs::read(&path)?;
        let actual = sha256_hex(&bytes);
        if actual != art.sha256 {
            return Err(BytodeError::Session(format!(
                "artifact checksum mismatch: {}",
                art.relative_path.display()
            )));
        }
        String::from_utf8(bytes)
            .map_err(|e| BytodeError::Session(format!("artifact not utf-8: {e}")))
    }
}
