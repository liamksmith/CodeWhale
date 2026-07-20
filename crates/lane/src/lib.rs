//! Lane 注册表 + 运行时后端（#4176）。
//!
//! **Lane** 是一个正在运行的工作流实例（一个 issue/目标）。**Runtime** 控制其在何处以及如何执行（tmux、inline、vm、ci）——与 Fleet 无关。
//!
//! 持久化：`$CODEWHALE_HOME/lanes/<lane-id>.json` 以及位于 `$CODEWHALE_HOME/lanes/logs/<lane-id>.ndjson` 下的流式 json 日志。

mod registry;
mod runtime;
mod worktree;

pub use registry::{LaneRecord, LaneRegistry, LaneStatus, lanes_dir};
pub use runtime::{
    InlineRuntime, LaneStartSpec, RuntimeBackend, RuntimeBackendKind, TmuxRuntime, backend_for,
    resolve_backend,
};
pub use worktree::{WorktreeProvision, provision_worktree, remove_worktree_if_expired};
