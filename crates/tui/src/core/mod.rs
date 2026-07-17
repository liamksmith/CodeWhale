//! DeepSeek CLI 的核心引擎模块。
//!
//! 本模块提供事件驱动的架构，将 UI 与 AI 交互逻辑分离：
//!
//! - `engine`: 处理操作的主引擎
//! - `events`: 引擎向 UI 发出的事件
//! - `ops`: UI 向引擎提交的操作
//! - `session`: 会话状态管理
//! - `turn`: 轮次上下文与跟踪

// 引擎代码运行在 TUI 备用屏幕（alt-screen）内部 —— 关于为什么原始 stdio 打印
// 不能出现在此处，请参见 `runtime_log`。请使用 `tracing::*` 替代。
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

pub mod authority;
pub mod engine;
pub mod events;
pub mod ops;
pub mod session;
pub mod tool_parser;
pub mod turn;

// Re-exports
