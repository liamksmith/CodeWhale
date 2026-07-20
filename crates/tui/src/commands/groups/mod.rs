//! 组拥有的内置命令区域。
//!
//! 每个组模块将命令对象注册到中央命令注册表中。命令实现函数仍归属于各自的组，
//! 而调度、面板元数据和帮助查找都从同一注册表接口读取。

macro_rules! cached_command_list {
    ($commands:expr) => {{
        static COMMANDS: std::sync::OnceLock<Vec<Box<dyn crate::commands::traits::Command>>> =
            std::sync::OnceLock::new();
        COMMANDS.get_or_init(|| $commands).as_slice()
    }};
}

pub mod config;
pub mod core;
pub mod debug;
pub mod memory;
pub mod plugins;
pub mod project;
pub mod session;
pub mod skills;
pub mod utility;

use std::sync::OnceLock;

use crate::commands::traits::CommandGroup;

pub fn all_command_groups() -> &'static [&'static dyn CommandGroup] {
    static GROUPS: OnceLock<Vec<&'static dyn CommandGroup>> = OnceLock::new();
    GROUPS
        .get_or_init(|| {
            vec![
                &core::CoreCommands,
                &session::SessionCommands,
                &config::ConfigCommands,
                &debug::DebugCommands,
                &project::ProjectCommands,
                &skills::SkillsCommands,
                &memory::MemoryCommands,
                &plugins::PluginsCommands,
                &utility::UtilityCommands,
            ]
        })
        .as_slice()
}
