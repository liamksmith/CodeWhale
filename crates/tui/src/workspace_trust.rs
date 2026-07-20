//! 每个工作区的外部路径信任列表，代理可在不触发 `PathEscape` 错误的情况下读写这些路径 (#29)。
//!
//! 存储位置：`~/.deepseek/workspace-trust.json`。该文件是一个 JSON 对象，
//! 将每个工作区的规范路径映射到该工作区用户显式信任的规范路径排序列表。
//! 在工作区 A 中授予的信任在从工作区 B 运行时无效。
//!
//! 威胁模型：这是用户对工作区沙箱否则会拒绝的路径的主动选择加入。
//! 信任列表授予的唯一访问权限是通过 CodeWhale 自身的文件工具（`read_file`、`write_file` 等）——
//! 它不会放宽用于 shell 命令的 OS 沙箱配置文件（Seatbelt/Landlock）。
//! 沙箱配置文件的扩展被单独跟踪，以便 Shell 工具在未来的版本中可以选择相同的路径。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::utils::write_atomic;

const TRUST_FILE_NAME: &str = "workspace-trust.json";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct TrustFile {
    /// 工作区规范路径 → 排序的唯一信任路径映射。
    #[serde(default)]
    workspaces: BTreeMap<String, Vec<String>>,
}

/// 单个工作区的内存中信任列表，在加载时快照。
/// 工具查阅此快照以决定工作区外的路径是否被允许；引擎在 `/trust` 变更后刷新它。
#[derive(Debug, Default, Clone)]
pub struct WorkspaceTrust {
    paths: Vec<PathBuf>,
}

impl WorkspaceTrust {
    #[must_use]
    #[allow(dead_code)]
    pub fn empty() -> Self {
        Self { paths: Vec::new() }
    }

    /// 从磁盘加载 `workspace` 的信任路径快照。缺失或格式错误的文件
    /// 返回空列表而不是错误，这样损坏的信任文件不会阻塞 TUI；下次变更时会重写它。
    #[must_use]
    pub fn load_for(workspace: &Path) -> Self {
        match trust_file_path() {
            Some(path) => Self::load_from_file(workspace, &path),
            None => Self::empty(),
        }
    }

    fn load_from_file(workspace: &Path, file_path: &Path) -> Self {
        let key = workspace_key(workspace);
        let file = read_trust_file_at(file_path).unwrap_or_default();
        let paths = file
            .workspaces
            .get(&key)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();
        Self { paths }
    }

    /// 返回规范形式的信任路径。
    #[must_use]
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// 判断候选项是否被信任：候选项（在规范归一化后）以某个信任前缀开头。
    /// 目录信任授予对该目录下任何内容的访问权限。
    #[must_use]
    #[allow(dead_code)]
    pub fn permits(&self, candidate: &Path) -> bool {
        let canonical = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.to_path_buf());
        self.paths
            .iter()
            .any(|trusted| canonical.starts_with(trusted))
    }
}

/// 向 `workspace` 的信任列表添加 `path` 并持久化。返回实际存储的规范信任路径，
/// 以便调用者可以将其反馈给用户。
pub fn add(workspace: &Path, path: &Path) -> Result<PathBuf> {
    let trust_path = trust_file_path()
        .context("home directory not available; cannot persist workspace trust list")?;
    add_at(workspace, path, &trust_path)
}

fn add_at(workspace: &Path, path: &Path, trust_path: &Path) -> Result<PathBuf> {
    let canonical = canonicalize_or_keep(path);
    let key = workspace_key(workspace);
    let mut file = read_trust_file_at(trust_path).unwrap_or_default();
    let entry = file.workspaces.entry(key).or_default();
    let stored = canonical.to_string_lossy().to_string();
    if !entry.iter().any(|p| p == &stored) {
        entry.push(stored.clone());
        entry.sort();
        entry.dedup();
    }
    write_trust_file_at(&file, trust_path)?;
    Ok(canonical)
}

/// 从 `workspace` 的信任列表中移除 `path`。当条目实际被移除时返回 true。
pub fn remove(workspace: &Path, path: &Path) -> Result<bool> {
    let Some(trust_path) = trust_file_path() else {
        return Ok(false);
    };
    remove_at(workspace, path, &trust_path)
}

fn remove_at(workspace: &Path, path: &Path, trust_path: &Path) -> Result<bool> {
    let canonical = canonicalize_or_keep(path);
    let key = workspace_key(workspace);
    let mut file = read_trust_file_at(trust_path).unwrap_or_default();
    let stored = canonical.to_string_lossy().to_string();
    let removed = match file.workspaces.get_mut(&key) {
        Some(entry) => {
            let len_before = entry.len();
            entry.retain(|p| p != &stored);
            let changed = entry.len() != len_before;
            if entry.is_empty() {
                file.workspaces.remove(&key);
            }
            changed
        }
        None => false,
    };
    if removed {
        write_trust_file_at(&file, trust_path)?;
    }
    Ok(removed)
}

fn workspace_key(workspace: &Path) -> String {
    canonicalize_or_keep(workspace)
        .to_string_lossy()
        .into_owned()
}

fn canonicalize_or_keep(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn trust_file_path() -> Option<PathBuf> {
    codewhale_config::ensure_state_dir(".")
        .ok()
        .map(|dir| dir.join(TRUST_FILE_NAME))
}

fn read_trust_file_at(path: &Path) -> Result<TrustFile> {
    if !path.exists() {
        return Ok(TrustFile::default());
    }
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

fn write_trust_file_at(file: &TrustFile, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(file).context("serialize trust file")?;
    write_atomic(path, json.as_bytes()).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// 设置一个隔离的伪 `~/.deepseek/workspace-trust.json` 位置。
    /// 返回 tmpdir（在测试期间保持存活）以及传递给 `*_at` 辅助函数的显式信任文件路径——
    /// 避免触及 `$HOME`，以便测试安全地并行运行。
    fn isolated_trust_path() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().expect("tempdir");
        let trust_path = tmp.path().join(".deepseek").join("workspace-trust.json");
        (tmp, trust_path)
    }

    #[test]
    fn empty_trust_for_unknown_workspace() {
        let (tmp, trust_path) = isolated_trust_path();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let trust = WorkspaceTrust::load_from_file(&workspace, &trust_path);
        assert!(trust.paths().is_empty());
        assert!(!trust.permits(Path::new("/anywhere")));
    }

    #[test]
    fn add_persists_and_load_returns_path() {
        let (tmp, trust_path) = isolated_trust_path();
        let workspace = tmp.path().join("ws");
        let other = tmp.path().join("data/notes");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&other).unwrap();

        let stored = add_at(&workspace, &other, &trust_path).expect("add");
        // On macOS, /var/folders is a symlink to /private/var/folders so the
        // canonical form may live under that prefix. Compare using
        // canonicalize on both ends.
        let canonical_other = other.canonicalize().unwrap_or(other.clone());
        assert_eq!(stored, canonical_other);

        let trust = WorkspaceTrust::load_from_file(&workspace, &trust_path);
        assert_eq!(trust.paths().len(), 1);
        // Create the file so canonicalize resolves through any symlinks; the
        // stored trust path uses the canonical form.
        let inner = other.join("file.md");
        std::fs::write(&inner, "x").unwrap();
        assert!(trust.permits(&inner));
        assert!(!trust.permits(Path::new("/etc/passwd")));
    }

    #[test]
    fn add_is_idempotent() {
        let (tmp, trust_path) = isolated_trust_path();
        let workspace = tmp.path().join("ws");
        let other = tmp.path().join("data/notes");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&other).unwrap();

        let _ = add_at(&workspace, &other, &trust_path).unwrap();
        let _ = add_at(&workspace, &other, &trust_path).unwrap();
        let trust = WorkspaceTrust::load_from_file(&workspace, &trust_path);
        assert_eq!(trust.paths().len(), 1);
    }

    #[test]
    fn trust_is_workspace_scoped() {
        let (tmp, trust_path) = isolated_trust_path();
        let ws_a = tmp.path().join("ws-a");
        let ws_b = tmp.path().join("ws-b");
        let other = tmp.path().join("data/notes");
        std::fs::create_dir_all(&ws_a).unwrap();
        std::fs::create_dir_all(&ws_b).unwrap();
        std::fs::create_dir_all(&other).unwrap();

        add_at(&ws_a, &other, &trust_path).unwrap();
        assert_eq!(
            WorkspaceTrust::load_from_file(&ws_a, &trust_path)
                .paths()
                .len(),
            1
        );
        assert_eq!(
            WorkspaceTrust::load_from_file(&ws_b, &trust_path)
                .paths()
                .len(),
            0
        );
    }

    #[test]
    fn remove_deletes_path() {
        let (tmp, trust_path) = isolated_trust_path();
        let workspace = tmp.path().join("ws");
        let other = tmp.path().join("data/notes");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&other).unwrap();

        add_at(&workspace, &other, &trust_path).unwrap();
        let removed = remove_at(&workspace, &other, &trust_path).unwrap();
        assert!(removed);

        let trust = WorkspaceTrust::load_from_file(&workspace, &trust_path);
        assert!(trust.paths().is_empty());
    }
}
