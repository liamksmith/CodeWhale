//! 每个工作空间快照副仓库的路径解析。
//!
//! 快照存放在已解析的状态目录
//!（`~/.codewhale/snapshots` 或旧版 `~/.deepseek/snapshots`）下，
//! 采用两级哈希拆分，以便我们可以独立快照同一项目的多个工作树——
//! `git worktree list` 用户在特性分支之间不会发生串扰。

use std::io;
use std::path::{Path, PathBuf};

/// 计算给定工作空间路径的快照目录。
///
/// 返回 `$STATE_DIR/snapshots/<project_hash>/<worktree_hash>/`，
/// 其中 `$STATE_DIR` 通过 `codewhale_config::resolve_state_dir` 解析。
/// 调用方负责在磁盘上创建它；我们特意不在此处访问文件系统，
/// 以便可以廉价地重复调用。
///
/// `project_hash` 基于规范化后的工作空间路径推导，去除任何 `.worktrees/<name>` 后缀——
/// 同一仓库的多个工作树共享相同的 `project_hash`，以便用户如果需要可以跨工作树浏览快照，
/// 但 `worktree_hash` 默认保持提交隔离。
pub fn snapshot_dir_for(workspace: &Path) -> PathBuf {
    snapshot_dir_with_home(workspace, dirs::home_dir())
}

/// 与 [`snapshot_dir_for`] 相同，但可注入主目录。
/// 由测试使用，以便它们永远不会触及用户的真实状态目录。
pub fn snapshot_dir_with_home(workspace: &Path, home: Option<PathBuf>) -> PathBuf {
    let home = home.unwrap_or_else(|| PathBuf::from("."));
    let canonical = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let project_root = strip_worktree_suffix(&canonical);
    let project_hash = stable_hex(&project_root);
    let worktree_hash = stable_hex(&canonical);
    snapshot_base_with_home(Some(home))
        .join(project_hash)
        .join(worktree_hash)
}

fn snapshot_base_with_home(home: Option<PathBuf>) -> PathBuf {
    let home = home.unwrap_or_else(|| PathBuf::from("."));
    // 优先使用 .codewhale，回退到 .deepseek
    let primary = home.join(".codewhale").join("snapshots");
    if primary.exists() {
        return primary;
    }
    home.join(".deepseek").join("snapshots")
}

/// 解析快照目录内的 `.git` 目录。
pub fn snapshot_git_dir(workspace: &Path) -> PathBuf {
    snapshot_dir_for(workspace).join(".git")
}

/// 确保快照目录在磁盘上存在并返回其路径。
pub fn ensure_snapshot_dir(workspace: &Path) -> io::Result<PathBuf> {
    let dir = snapshot_dir_for(workspace);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 去除末尾的 `.worktrees/<name>` 段，以便同一检出中的所有工作树共享一个 `project_hash`。
/// 如果路径看起来不像工作树，则原样返回。
fn strip_worktree_suffix(path: &Path) -> PathBuf {
    let mut components: Vec<_> = path.components().collect();
    if components.len() >= 2
        && let Some(parent) = components.get(components.len() - 2)
        && parent.as_os_str() == ".worktrees"
    {
        components.truncate(components.len() - 2);
        let mut p = PathBuf::new();
        for c in components {
            p.push(c.as_os_str());
        }
        return p;
    }
    path.to_path_buf()
}

/// 十六进制编码的确定性 FNV-1a 摘要。这只是目录标签，不是安全边界，
/// 但它必须在进程启动之间保持稳定。
fn stable_hex(path: &Path) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn snapshot_dir_layout_two_levels_under_deepseek() {
        let tmp = tempdir().expect("tempdir");
        let dir = snapshot_dir_with_home(tmp.path(), Some(tmp.path().to_path_buf()));
        let mut iter = dir.strip_prefix(tmp.path()).unwrap().components();
        assert_eq!(iter.next().unwrap().as_os_str(), ".deepseek");
        assert_eq!(iter.next().unwrap().as_os_str(), "snapshots");
        assert!(iter.next().is_some()); // project_hash
        assert!(iter.next().is_some()); // worktree_hash
        assert!(iter.next().is_none());
    }

    #[test]
    fn worktree_suffix_stripped_for_project_hash() {
        let tmp = tempdir().expect("tempdir");
        let main_path = tmp.path().join("repo");
        let wt_path = tmp.path().join("repo").join(".worktrees").join("featX");
        std::fs::create_dir_all(&main_path).unwrap();
        std::fs::create_dir_all(&wt_path).unwrap();

        let main_dir = snapshot_dir_with_home(&main_path, Some(tmp.path().to_path_buf()));
        let wt_dir = snapshot_dir_with_home(&wt_path, Some(tmp.path().to_path_buf()));

        // 相同的 project_hash（工作树特定尾部之前的父组件）。
        let main_components: Vec<_> = main_dir.components().collect();
        let wt_components: Vec<_> = wt_dir.components().collect();
        assert_eq!(
            main_components[main_components.len() - 2],
            wt_components[wt_components.len() - 2],
            "worktrees should share project_hash",
        );
        // 但不同的 worktree_hash（尾部）。
        assert_ne!(main_components.last(), wt_components.last());
    }

    #[test]
    fn ensure_snapshot_dir_creates_path() {
        let tmp = tempdir().expect("tempdir");
        // 使用限定范围的 HOME，这样就不会污染真实的 HOME。
        let dir = snapshot_dir_with_home(tmp.path(), Some(tmp.path().to_path_buf()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(dir.exists());
    }

    #[test]
    fn snapshot_git_dir_appends_dot_git() {
        let tmp = tempdir().expect("tempdir");
        let git_dir = snapshot_git_dir(tmp.path());
        assert_eq!(git_dir.file_name().unwrap(), ".git");
    }
}
