//! 核心命令区域：模型/提供者选择、帮助、导航以及持久的 RLM/子代理入口点。

#[cfg(all(test, feature = "long-running-tests"))]
mod acceptance;
mod agent;
mod anchor;
mod clear;
mod constitution;
// 此组目录故意包含一个同名的 `core.rs` 子模块。
// `module_inception` 允许是永久性的结构设计理由，而非迁移临时方案；
// 参见 docs/architecture/command-dispatch.md。
#[allow(clippy::module_inception)]
mod core;
mod exit;
mod feedback;
mod fleet;
mod help;
mod hf;
mod home;
mod hooks;
mod hotbar;
mod links;
mod model;
mod modeldb;
mod models;
mod profile;
mod provider;
mod queue;
mod rlm;
mod setup;
mod stash;
mod subagents;
mod translate;
pub mod util;
pub mod voice;
mod workflow;
mod workspace;

pub(in crate::commands) use self::core::reset_conversation_state;

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandGroup, FunctionCommand, RegisterCommand};

pub struct CoreCommands;

impl CommandGroup for CoreCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(FunctionCommand::new(
                anchor::AnchorCmd::info(),
                anchor::AnchorCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                help::HelpCmd::info(),
                help::HelpCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                clear::ClearCmd::info(),
                clear::ClearCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                exit::ExitCmd::info(),
                exit::ExitCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                model::ModelCmd::info(),
                model::ModelCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                models::ModelsCmd::info(),
                models::ModelsCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                modeldb::ModelDbCmd::info(),
                modeldb::ModelDbCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                provider::ProviderCmd::info(),
                provider::ProviderCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                queue::QueueCmd::info(),
                queue::QueueCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                stash::StashCmd::info(),
                stash::StashCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                hooks::HooksCmd::info(),
                hooks::HooksCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                subagents::SubagentsCmd::info(),
                subagents::SubagentsCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                fleet::FleetCmd::info(),
                fleet::FleetCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                workflow::WorkflowCmd::info(),
                workflow::WorkflowCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                hotbar::HotbarCmd::info(),
                hotbar::HotbarCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                setup::SetupCmd::info(),
                setup::SetupCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                constitution::ConstitutionCmd::info(),
                constitution::ConstitutionCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                agent::AgentCmd::info(),
                agent::AgentCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                links::LinksCmd::info(),
                links::LinksCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                feedback::FeedbackCmd::info(),
                feedback::FeedbackCmd::execute,
            )),
            Box::new(FunctionCommand::new(hf::HfCmd::info(), hf::HfCmd::execute)),
            Box::new(FunctionCommand::new(
                home::HomeCmd::info(),
                home::HomeCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                workspace::WorkspaceCmd::info(),
                workspace::WorkspaceCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                profile::ProfileCmd::info(),
                profile::ProfileCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                rlm::RlmCmd::info(),
                rlm::RlmCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                translate::TranslateCmd::info(),
                translate::TranslateCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                voice::VoiceCmd::info(),
                voice::VoiceCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                voice::VoiceSendCmd::info(),
                voice::VoiceSendCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                voice::VoiceControlCmd::info(),
                voice::VoiceControlCmd::execute,
            )),
        ])
    }
}
