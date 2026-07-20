//! 工作区快照 — 轮次前后的安全网。
//!
//! 每个轮次引擎将用户工作区的 `pre-turn:<seq>` 快照保存到侧边 git 仓库中，
//! 路径为 `~/.deepseek/snapshots/<project_hash>/<worktree_hash>/.git`，
//! 然后在轮次完成时保存对应的 `post-turn:<seq>` 快照。用户可以通过
//! `/restore N`（斜杠命令）回滚，或者当模型识别到"撤销我的最后一次编辑"意图时，
//! 使用 `revert_turn` 工具。
//!
//! ## 为什么使用侧边仓库？
//!
//! - 用户的 `.git` 永远不会被触及。当调用 git 时，`--git-dir` 和
//!   `--work-tree` *始终*一起设置；这一不变性确保快照和用户仓库完全独立。
//! - 没有 git 的工作区仍然可以获得快照。
//! - `git` 自身的去重（对象包文件）使磁盘占用保持在可控范围 — 典型 100 MB 工作区 × 12 轮次 ≈
//!   1.2 GB 未压缩，但 git 的内容寻址存储通常能将此降低 10-30 倍。我们进一步通过以下方式缓解：
//!     - 7 天默认保留期（`session_manager` 在会话启动时通过 [`prune::prune_older_than`] 清理）。
//!     - 侧边仓库上设置 `gc.auto = 0`（我们不希望在轮次中途触发后台 gc）以及在清理后执行
//!       显式的 `git gc --prune=now`。
//!     - 启动时清理中断的 git 打包操作留下的过期 `tmp_pack_*` 文件。
//!
//! ## 失败模型
//!
//! 轮次前后快照调用是 **非致命的**。如果 `git` 缺失、磁盘已满或工作区位于只读文件系统上，
//! 轮次继续执行，引擎记录警告。快照是安全网，而非正确性门控。

pub mod paths;
pub mod prune;
pub mod repo;

#[allow(unused_imports)]
pub use paths::{snapshot_dir_for, snapshot_git_dir};
pub use prune::{DEFAULT_MAX_AGE, prune_older_than};

/// 每个工作区侧边仓库保留的最大快照数。每次新快照后清理最旧的快照以限制磁盘使用（#1112）。
pub const DEFAULT_MAX_SNAPSHOTS: usize = 50;
#[allow(unused_imports)]
pub use repo::{
    DEFAULT_MAX_WORKSPACE_BYTES_FOR_SNAPSHOT, Snapshot, SnapshotId, SnapshotRepo,
    estimate_workspace_size_bounded,
};
