//! 记忆命令区域：持久化记忆与快速笔记。

// 本 group 目录有意包含一个同名的 `memory.rs` 子模块。允许 module_inception 是永久性的结构设计决策，
// 而非迁移脚手架；详见 docs/architecture/command-dispatch.md。
#[allow(clippy::module_inception)]
mod memory;
mod note;

use crate::commands::traits::{Command, CommandGroup, FunctionCommand, RegisterCommand};

pub struct MemoryCommands;

impl CommandGroup for MemoryCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(FunctionCommand::new(
                note::NoteCmd::info(),
                note::NoteCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                memory::MemoryCmd::info(),
                memory::MemoryCmd::execute,
            )),
        ])
    }
}
