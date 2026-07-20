//! 工具系统模块与重导出。

// 工具运行在 TUI 备用屏幕运行时内部。此模块树内的原始 `print!` / `eprintln!` 会泄漏到 ratatui 的 diff 渲染缓冲区，
// 导致"滚动恶魔"回归问题（#1085 / v0.8.27 后续）。
// 请改用 `tracing::*` 路由状态/错误报告 — `runtime_log` 订阅器将其捕获到 `~/.deepseek/logs/`。
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

pub mod apply_patch;
pub mod approval_cache;
pub mod arg_repair;
pub mod automation;
pub mod cargo_failure_summary;
pub mod dev_server_readiness;
pub mod diagnostics;
pub mod diff_format;
pub mod dynamic;
pub mod file;
pub mod file_search;
pub mod finance;

pub mod fetch_url;
pub mod fim;
pub mod git;
pub mod git_history;
pub mod github;
pub mod goal;
pub mod handle;
pub mod image_ocr;
pub mod js_execution;
pub mod large_output_router;
pub mod notify;
pub mod pandoc;
pub mod parallel;
pub mod plan;
pub mod plugin;
pub mod project;
pub mod registry;
pub mod remember;
pub mod revert_turn;
pub mod review;
pub mod rlm;
pub mod runtime_mcp;
pub mod schema_canonicalize;
pub mod schema_sanitize;
pub mod search;
pub mod shell;
mod shell_output;
pub mod skill;
pub mod spec;
pub mod speech;
pub mod subagent;
pub mod tasks;
pub mod test_runner;
pub mod todo;
pub mod tool_result_retrieval;
pub mod truncate;
pub mod user_input;
pub mod validate_data;
pub mod verifier;
pub mod web_run;
pub mod web_search;
pub mod workflow;
pub mod workflow_plan_approval;
pub mod workflow_trigger;

pub use registry::{AgentToolSurfaceOptions, ToolRegistry, ToolRegistryBuilder};
pub use review::ReviewOutput;
pub use spec::ToolContext;
pub use user_input::UserInputResponse;
