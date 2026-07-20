//! Hotbar 动作注册基础。
//!
//! 配置、侧边栏渲染和键分发使用此动作表面以及在此定义的内置动作。

pub mod actions;
pub mod setup;

pub use actions::HotbarActionRegistry;
