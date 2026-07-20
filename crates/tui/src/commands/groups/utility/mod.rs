//! 实用命令区域：附件、后台任务、作业、MCP 和网络检查。

mod attachment;
mod jobs;
mod mcp;
mod network;
mod task;

use crate::commands::traits::{Command, CommandGroup, FunctionCommand, RegisterCommand};

pub struct UtilityCommands;

impl CommandGroup for UtilityCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(FunctionCommand::new(
                attachment::AttachCmd::info(),
                attachment::AttachCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                task::TaskCmd::info(),
                task::TaskCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                jobs::JobsCmd::info(),
                jobs::JobsCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                mcp::McpCmd::info(),
                mcp::McpCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                network::NetworkCmd::info(),
                network::NetworkCmd::execute,
            )),
        ])
    }
}
