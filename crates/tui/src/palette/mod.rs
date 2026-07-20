//! DeepSeek 颜色调色板与语义角色。
//!
//! 本模块定义 TUI 的三层颜色系统：
//!
//! 1. **RGB 元组**（`*_RGB` 常量）——主题生成和运行时调色板构建所使用的原始颜色值。
//! 2. **语义 `Color` 常量**——预计算的 `ratatui::style::Color` 值，映射到 UI 角色（表面、文本、强调、状态、模式）。
//! 3. **向后兼容的别名**（`DEEPSEEK_*`）——委托给当前 Whale 调色板常量的旧名称。

mod adapt;
mod detect;
mod themes;
mod tokens;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use adapt::*;
#[allow(unused_imports)]
pub use detect::*;
#[allow(unused_imports)]
pub use themes::*;
pub use tokens::*;
