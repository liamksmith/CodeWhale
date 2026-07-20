//! 技能命令区域：列出和运行技能、回顾和恢复。

mod restore;
mod review;
    // 此组目录特意包含一个同名的 `skills.rs` 子模块。
    // module_inception 允许是永久性结构设计，而非迁移脚手架；参见 docs/architecture/command-dispatch.md。
#[allow(clippy::module_inception)]
mod skills;

pub(in crate::commands) use self::skills::run_skill_by_name;

use crate::commands::traits::{Command, CommandGroup, FunctionCommand, RegisterCommand};

pub struct SkillsCommands;

impl CommandGroup for SkillsCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(FunctionCommand::new(
                skills::SkillsCmd::info(),
                skills::SkillsCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                skills::SkillCmd::info(),
                skills::SkillCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                review::ReviewCmd::info(),
                review::ReviewCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                restore::RestoreCmd::info(),
                restore::RestoreCmd::execute,
            )),
        ])
    }
}
