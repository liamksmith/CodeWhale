//! 历史记录文本渲染的共享常量。

pub(super) const TOOL_COMMAND_LINE_LIMIT: usize = 3;
pub(super) const TOOL_OUTPUT_LINE_LIMIT: usize = 6;
pub(super) const TOOL_TEXT_LIMIT: usize = 300;
pub(super) const TOOL_HEADER_SUMMARY_LIMIT: usize = 56;
pub(super) const TOOL_OUTPUT_HEAD_LINES: usize = 2;
pub(super) const TOOL_OUTPUT_TAIL_LINES: usize = 2;
#[cfg(test)]
pub(super) const TOOL_RUNNING_SYMBOLS: [&str; 12] = crate::tui::spinner::BRAILLE_SPINNER_FRAMES;
#[cfg(test)]
pub(super) const TOOL_STATUS_SYMBOL_MS: u64 = crate::tui::spinner::BRAILLE_SPINNER_FRAME_MS;
/// 用户角色在消息行首的视觉标记。实心竖线 — 无动画；用户输入已完成。
pub(super) const USER_GLYPH: &str = "\u{258E}"; // ▎
/// 助手角色的视觉标记。实心圆点，在响应流式传输时以 2 秒周期脉冲，空闲时保持全亮。
pub(super) const ASSISTANT_GLYPH: &str = "\u{25CF}"; // ●
/// 记录正文左侧轨道。实心 1/8 块（`▏`）后跟一个空格 —
/// 用作续行、工具卡片详情行和提示行的视觉左边距锚点。
/// 暗淡显示，引导视线而不与内容竞争。
pub(super) const TRANSCRIPT_RAIL: &str = "\u{258F} "; // ▏ + space
pub(super) const TOOL_CARD_SUMMARY_LINES: usize = 4;
pub(super) const TOOL_DONE_SYMBOL: &str = "•";
pub(super) const TOOL_FAILED_SYMBOL: &str = "•";
/// 现场记录中前台 shell 等待的紧凑 Ctrl+B 提示。
pub(super) const FOREGROUND_SHELL_WAIT_HINT: &str = "Ctrl+B → /jobs";
