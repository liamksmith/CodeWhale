//! 工作树配置，由 Runtime（而非 Fleet）拥有 — #4176 / #4016。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use chrono::DateTime;

/// 车道（lane）的隔离工作树 + 分支规格。
#[derive(Debug, Clone)]
pub struct WorktreeProvision {
    /// Git 仓库根目录（必须包含 `.git`）。
    pub repo_root: PathBuf,
    /// 要创建的分支（从 `base_ref` 分出）。
    pub branch: String,
    /// 新工作树的目录（由 `git worktree add` 创建）。
    pub path: PathBuf,
    /// 分支的基准引用（默认为 `HEAD`）。
    pub base_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProvisionedWorktree {
    pub path: PathBuf,
    pub branch: String,
}

/// 为车道创建一个 git 工作树 + 分支。
pub fn provision_worktree(spec: &WorktreeProvision) -> Result<ProvisionedWorktree> {
    if spec.branch.trim().is_empty() {
        bail!("工作树分支不能为空");
    }
    if !spec.repo_root.exists() {
        bail!("仓库根目录不存在：{}", spec.repo_root.display());
    }
    if let Some(parent) = spec.path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建工作树父目录 {}", parent.display()))?;
    }
    let base = spec.base_ref.as_deref().unwrap_or("HEAD");
    let status = Command::new("git")
        .current_dir(&spec.repo_root)
        .args([
            "worktree",
            "add",
            "-b",
            &spec.branch,
            &spec.path.to_string_lossy(),
            base,
        ])
        .status()
        .context("git worktree add")?;
    if !status.success() {
        bail!(
            "git worktree add 失败：分支 {}，路径 {}",
            spec.branch,
            spec.path.display()
        );
    }
    Ok(ProvisionedWorktree {
        path: spec.path.clone(),
        branch: spec.branch.clone(),
    })
}

/// 当 TTL 已过期（或 TTL 为 0 时立即）移除工作树。
///
/// `stopped_at` 是 RFC3339 格式。当 `ttl_secs` 为 `None` 时不执行清理。
pub fn remove_worktree_if_expired(
    worktree_path: &Path,
    ttl_secs: Option<u64>,
    stopped_at: Option<&str>,
) -> Result<()> {
    let Some(ttl) = ttl_secs else {
        return Ok(());
    };
    if !worktree_path.exists() {
        return Ok(());
    }
    if ttl > 0 {
        let Some(stopped) = stopped_at else {
            return Ok(());
        };
        let stopped_ts = DateTime::parse_from_rfc3339(stopped)
            .with_context(|| format!("解析 stopped_at {stopped}"))?
            .timestamp() as u64;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if now.saturating_sub(stopped_ts) < ttl {
            return Ok(());
        }
    }

    // 尽力而为：git worktree remove --force，然后 rm -rf。
    let _ = Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            &worktree_path.to_string_lossy(),
        ])
        .status();
    if worktree_path.exists() {
        fs::remove_dir_all(worktree_path)
            .with_context(|| format!("移除工作树 {}", worktree_path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn init_repo(root: &Path) {
        assert!(
            Command::new("git")
                .args(["init", "-b", "main"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["config", "user.email", "lane@test"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["config", "user.name", "lane"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join("README"), "lane").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "README"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["commit", "-m", "init"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn provision_and_ttl_zero_cleanup() {
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        let wt_path = dir.path().join("wt-lane");
        let provisioned = provision_worktree(&WorktreeProvision {
            repo_root: repo,
            branch: "codex/lane-test".into(),
            path: wt_path.clone(),
            base_ref: Some("main".into()),
        })
        .unwrap();
        assert!(provisioned.path.is_dir());
        assert!(wt_path.join("README").is_file());

        remove_worktree_if_expired(&wt_path, Some(0), Some("2020-01-01T00:00:00Z")).unwrap();
        assert!(
            !wt_path.exists(),
            "TTL 为 0 应立即移除工作树"
        );
    }
}
