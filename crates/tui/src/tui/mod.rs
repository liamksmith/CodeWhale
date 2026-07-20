//! `DeepSeek` CLI 的终端 UI（TUI）模块。

// 渲染层在备用屏幕内运行。原始的 stdio 打印会产生滚动恶魔（详见 `runtime_log`）。
// 请使用 `tracing::*` 进行诊断 — `runtime_log` 将其捕获到磁盘。
// `ui::run_event_loop` 在 `LeaveAlternateScreen` 之后合法地打印退出后的恢复提示；
// 该单一位置局部使用了 `#[allow(clippy::print_stdout)]`。
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

// === 子模块 ===

pub mod active_cell;
pub mod app;
pub mod approval;
pub mod auto_review;
pub mod auto_router;
pub mod backtrack;
pub mod clipboard;
pub mod color_compat;
pub mod command_palette;
pub mod composer_ui;
pub mod context_inspector;
pub mod context_menu;
pub mod diff_render;
pub mod event_broker;
pub mod external_editor;
pub mod feedback_picker;
pub mod file_frecency;
pub mod file_mention;
pub mod file_picker;
pub mod file_picker_relevance;
pub mod file_tree;
pub mod footer_ui;
pub mod format_helpers;
pub mod frame_rate_limiter;
pub mod history;
pub mod hotbar;
pub mod key_actions;
pub mod key_shortcuts;
pub mod keybindings;
pub mod live_transcript;
pub mod markdown_render;
mod mcp_routing;
pub mod model_picker;
pub mod mouse_ui;
pub mod notifications;
pub mod onboarding;
pub mod osc8;
pub mod output_rows_cache;
pub mod pager;
pub mod paste;
pub mod paste_burst;
pub mod persistence_actor;
pub mod plan_prompt;
pub mod prompt_suggestion;
pub mod provider_picker;
pub mod scrolling;
pub mod selection;
pub mod session_picker;
pub mod setup;
mod shell_job_routing;
pub mod sidebar;
pub mod slash_menu;
pub mod spinner;
pub mod streaming;
pub mod streaming_thinking;
mod subagent_routing;
pub mod theme_picker;
mod tool_routing;
pub mod transcript;
pub mod transcript_cache;
pub mod translation;
pub mod ui;
mod ui_text;
pub mod user_input;
pub mod views;
pub mod vim_mode;
pub mod widgets;
pub mod workspace_context;

// === 重导出 ===

pub use app::{InitialInput, TuiOptions};
pub use ui::run_tui;
