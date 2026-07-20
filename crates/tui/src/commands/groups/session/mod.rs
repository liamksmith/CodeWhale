//! 会话命令区域：保存、复刻、恢复、导出以及 `/relay` 会话交接产物。

#[cfg(all(test, feature = "long-running-tests"))]
mod acceptance;
mod compact;
mod export;
mod fork;
mod load;
mod new;
mod purge;
mod relay;
mod rename;
mod save;
mod sessions;
// 此组目录有意包含一个同名的 `session.rs` 子模块。module_inception 允许是永久性的结构设计考量，而非迁移脚手架；参见 docs/architecture/command-dispatch.md。
#[allow(clippy::module_inception)]
mod session;

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandGroup, FunctionCommand, RegisterCommand};

pub struct SessionCommands;

impl CommandGroup for SessionCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(FunctionCommand::new(
                rename::RenameCmd::info(),
                rename::RenameCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                save::SaveCmd::info(),
                save::SaveCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                fork::ForkCmd::info(),
                fork::ForkCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                new::NewCmd::info(),
                new::NewCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                sessions::SessionsCmd::info(),
                sessions::SessionsCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                load::LoadCmd::info(),
                load::LoadCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                compact::CompactCmd::info(),
                compact::CompactCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                purge::PurgeCmd::info(),
                purge::PurgeCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                relay::RelayCmd::info(),
                relay::RelayCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                export::ExportCmd::info(),
                export::ExportCmd::execute,
            )),
        ])
    }
}
