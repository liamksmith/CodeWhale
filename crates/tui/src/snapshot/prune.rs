//! 启动时快照清理。
//!
//! 每次会话启动时从 `session_manager` 调用。失败永远不会致命——旧快照占用磁盘空间很烦人，但不会破坏正确性，因此我们记录日志并继续。

use std::io;
use std::path::Path;
use std::time::Duration;

use super::paths::snapshot_git_dir;
use super::repo::SnapshotRepo;

/// 默认快照保留期限：7 天。
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// 清理给定工作区中早于 `max_age` 的快照。
///
/// 如果快照仓库尚不存在（首次运行），这是一个低成本的无操作。
/// 返回已移除的快照数量。
pub fn prune_older_than(workspace: &Path, max_age: Duration) -> io::Result<usize> {
    let git_dir = snapshot_git_dir(workspace);
    if !git_dir.exists() {
        return Ok(0);
    }
    let repo = SnapshotRepo::open_or_init(workspace)?;
    let removed = repo.prune_older_than(max_age)?;
    // `git prune --expire=now` 遍历整个对象存储——在大型快照仓库上需要数秒。
    // 没有移除意味着没有新的不可达对象，因此在常见的启动路径上完全跳过此遍历（#3757）。
    if removed > 0 {
        repo.prune_unreachable_objects()?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lock_test_env;
    use std::sync::MutexGuard;
    use tempfile::tempdir;

    /// 与 `repo::tests` 中相同的守卫形式——在进程级环境互斥锁下，将一个测试的生命周期内固定 HOME。
    struct ScopedHome {
        prev: Option<std::ffi::OsString>,
        _guard: MutexGuard<'static, ()>,
    }
    impl Drop for ScopedHome {
        fn drop(&mut self) {
            // 安全性：进程级锁仍然持有。
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }
    fn scoped_home(home: &std::path::Path) -> ScopedHome {
        let guard = lock_test_env();
        let prev = std::env::var_os("HOME");
        // 安全性：由全局环境锁序列化。
        unsafe {
            std::env::set_var("HOME", home);
        }
        ScopedHome {
            prev,
            _guard: guard,
        }
    }

    #[test]
    fn prune_no_repo_returns_zero() {
        let tmp = tempdir().unwrap();
        let _home = scoped_home(tmp.path());
        let removed = prune_older_than(tmp.path(), DEFAULT_MAX_AGE).unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn prune_with_existing_repo_zero_age_clears_all() {
        let tmp = tempdir().unwrap();
        let _home = scoped_home(tmp.path());
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
        std::fs::write(workspace.join("f.txt"), "x").unwrap();
        repo.snapshot("turn:0").unwrap();

        // 同秒抖动防护：参见 `repo::tests`。
        std::thread::sleep(Duration::from_millis(1100));

        let removed = prune_older_than(&workspace, Duration::from_secs(0)).unwrap();
        assert!(removed >= 1);
    }
}
