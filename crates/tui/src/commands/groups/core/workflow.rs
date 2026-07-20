//! `/workflow` 命令——用户选择使用工作流编排。
//!
//! 调用携带的是授权而非负载：裸 `/workflow` 让模型从对话上下文中综合目标
//! 并通过 `workflow` 工具进行编排（与 goal 模式的 `/goal` 合约相同：
//! 依赖上下文，无需参数）。`/workflow <objective>` 将运行限定到显式目标，
//! `/workflow status` 则将在不启动新任务的情况下中继类型化运行回执。

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "workflow",
    aliases: &["workflows", "wf"],
    usage: "/workflow [objective|status|cancel <run_id>]",
    description_id: MessageId::CmdWorkflowDescription,
};

pub(in crate::commands) struct WorkflowCmd;

impl RegisterCommand for WorkflowCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        workflow(app, arg)
    }
}

/// 共享编排合约，附加到每条启动指令中。反映了选择式编排的良好实践：
/// 用户的调用就是授权，扇出规模随任务需求伸缩，回执闭环。
const ORCHESTRATION_CONTRACT: &str = "Author a workflow script for the `workflow` tool (task()/parallel()/pipeline()/phase()/log()); \
     you are the fan-in owner — fan out, wait for receipts, aggregate, verify, and synthesize one result. \
     scale the fan-out to the size of the ask — a quick check gets a few tasks, an audit gets a wider sweep. \
     Prefer pipeline() over barriers so items flow stage-to-stage without waiting. \
     Use responseSchema on task() when you need structured child output; schema mismatches fail loudly in the run receipt. \
     parallel() turns child failures into null — filter those slots and treat them as failures, not results. \
     Run it with the `workflow` tool (`run` to block, or `start` then `status` for long runs), \
     narrate phases as they complete, verify findings before reporting them as facts, \
     and end with a compact receipt summary: run_id, status, and per-leaf outcomes.";

pub fn workflow(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let arg = arg.map(str::trim).filter(|value| !value.is_empty());

    if let Some(action) = parse_workflow_control_action(arg) {
        return action;
    }

    match arg {
        // 显式目标：参数限定了运行范围。
        Some(objective) => {
            let message = format!(
                "The user invoked /workflow with an explicit objective — this is authorization to \
                 orchestrate it with the `workflow` tool. Objective: {objective:?}. \
                 Use the conversation context to ground the work (files discussed, prior findings). \
                 {ORCHESTRATION_CONTRACT}"
            );
            CommandResult::with_message_and_action(
                format!("Orchestrating as a workflow: {objective}"),
                AppAction::SendMessage(message),
            )
        }
        // 裸调用：依赖上下文。模型从会话当前工作内容推导目标——无需重复说明。
        None => {
            let message = format!(
                "The user invoked /workflow with no argument — this is authorization to orchestrate \
                 the CURRENT work as a workflow. Synthesize the objective from the conversation \
                 context: the task in flight, recent findings, and open items. Do not ask the user \
                 to restate it unless the conversation genuinely contains no work yet. \
                 {ORCHESTRATION_CONTRACT}"
            );
            CommandResult::with_message_and_action(
                "Orchestrating the current work as a workflow...",
                AppAction::SendMessage(message),
            )
        }
    }
}

/// 通过 `workflow` 工具路由 `status`/`cancel` 操作，不启动新运行。
fn parse_workflow_control_action(arg: Option<&str>) -> Option<CommandResult> {
    let arg = arg?;
    let (verb, rest) = match arg.split_once(char::is_whitespace) {
        Some((verb, rest)) => (verb, rest.trim()),
        None => (arg, ""),
    };
    match verb {
        "status" | "runs" | "list" | "inspect" => {
            let target = if rest.is_empty() {
                "all runs".to_string()
            } else {
                format!("run_id `{rest}`")
            };
            let message = format!(
                "Call the `workflow` tool with action `status`{} and summarize the receipts for \
                 the user: run_id, status, phase progress, per-leaf outcomes, and any errors. \
                 Keep it compact. Do not start a new workflow.",
                if rest.is_empty() {
                    String::new()
                } else {
                    format!(" and run_id `{rest}`")
                }
            );
            Some(CommandResult::with_message_and_action(
                format!("Fetching workflow status for {target}..."),
                AppAction::SendMessage(message),
            ))
        }
        "cancel" | "stop" | "abort" => {
            if rest.is_empty() || rest.contains(char::is_whitespace) {
                return Some(CommandResult::error(
                    "Usage: /workflow cancel <run_id>\n\nUse /workflow status to list run ids.",
                ));
            }
            let message = format!(
                "Call the `workflow` tool with action `cancel` and run_id `{rest}`, then report \
                 the final run status to the user. Do not start a new workflow."
            );
            Some(CommandResult::with_message_and_action(
                format!("Cancelling workflow {rest}..."),
                AppAction::SendMessage(message),
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::tui::app::TuiOptions;

    fn test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &crate::config::Config::default())
    }

    #[test]
    fn bare_workflow_is_context_dependent_opt_in() {
        let mut app = test_app();
        let result = workflow(&mut app, None);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        // 裸形式不得要求用户提供目标。
        assert!(message.contains("Synthesize the objective from the conversation"));
        assert!(message.contains("authorization to orchestrate"));
        assert!(message.contains("`workflow` tool"));

        // 仅空白字符的表现与裸调用相同。
        let result = workflow(&mut app, Some("   "));
        assert!(matches!(result.action, Some(AppAction::SendMessage(_))));
    }

    #[test]
    fn workflow_with_objective_forwards_it() {
        let mut app = test_app();
        let result = workflow(&mut app, Some("audit provider error handling"));
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("audit provider error handling"));
        assert!(message.contains("authorization"));
    }

    #[test]
    fn workflow_status_and_cancel_route_to_tool_without_new_runs() {
        let mut app = test_app();
        let result = workflow(&mut app, Some("status"));
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("action `status`"));
        assert!(message.contains("Do not start a new workflow"));

        let result = workflow(&mut app, Some("status wf_run_1"));
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("run_id `wf_run_1`"));

        let result = workflow(&mut app, Some("cancel wf_run_1"));
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("action `cancel`"));
        assert!(message.contains("run_id `wf_run_1`"));

        let result = workflow(&mut app, Some("cancel"));
        assert!(result.is_error, "cancel without a run id is a usage error");
    }
}
