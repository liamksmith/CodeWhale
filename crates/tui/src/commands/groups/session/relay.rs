//! `/relay` 命令。

use std::fmt::Write as _;

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "relay",
    aliases: &["batonpass", "接力"],
    usage: "/relay [focus]",
    description_id: MessageId::CmdRelayDescription,
};

pub(in crate::commands) struct RelayCmd;

impl RegisterCommand for RelayCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        relay(app, arg)
    }
}

/// 要求当前模型为下一个线程编写紧凑的接力文档。
///
/// 可见命令为 `/relay`（中文用户可使用 `/接力`），
/// 但持久文件路径保持为 `.deepseek/handoff.md` 以兼容
/// 现有会话和启动提示词加载。
pub fn relay(app: &mut App, arg: Option<&str>) -> CommandResult {
    let focus = arg.map(str::trim).filter(|value| !value.is_empty());
    let message = build_relay_instruction(app, focus);
    CommandResult::with_message_and_action(
        "正在准备会话接力文档（.deepseek/handoff.md）...",
        AppAction::SendMessage(message),
    )
}

fn build_relay_instruction(app: &App, focus: Option<&str>) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "为未来的 CodeWhale 线程创建一个紧凑的会话接力文档。"
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "编写或更新 `.deepseek/handoff.md`。");
    let _ = writeln!(
        out,
        "保持现有文件路径以兼容，但将文档标题命名为 `# Session relay`。"
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "当前会话快照：");
    let _ = writeln!(out, "- 工作区：{}", app.workspace.display());
    let _ = writeln!(out, "- 模式：{}", app.mode.label());
    let _ = writeln!(out, "- 模型：{}", app.model_display_label());
    if let Some(focus) = focus {
        let _ = writeln!(out, "- 请求的接力焦点：{focus}");
    }
    if let Some(quarry) = app.hunt.quarry.as_deref() {
        let _ = writeln!(out, "- 目标：{quarry}");
    }
    if let Some(budget) = app.hunt.token_budget {
        let _ = writeln!(out, "- 目标 token 预算：{budget}");
    }
    if let Ok(todos) = app.todos.try_lock() {
        let snapshot = todos.snapshot();
        if !snapshot.items.is_empty() {
            let _ = writeln!(
                out,
                "\n待办事项（主要进度表面，已完成 {}%）：",
                snapshot.completion_pct
            );
            for item in snapshot.items {
                let _ = writeln!(
                    out,
                    "- #{} [{}] {}",
                    item.id,
                    item.status.as_str(),
                    item.content
                );
            }
        }
    } else {
        let _ = writeln!(out, "\n待办事项：由于列表正忙，无法获取。");
    }

    if let Ok(plan) = app.plan_state.try_lock() {
        let snapshot = plan.snapshot();
        if !snapshot.is_empty() {
            let _ = writeln!(out, "\n来自 update_plan 的可选策略元数据：");
            write_plan_field(&mut out, "标题", snapshot.title.as_deref());
            write_plan_field(&mut out, "目标", snapshot.objective.as_deref());
            write_plan_field(&mut out, "上下文", snapshot.context_summary.as_deref());
            write_plan_field(&mut out, "说明", snapshot.explanation.as_deref());
            write_plan_list(&mut out, "来源", &snapshot.sources_used);
            write_plan_list(&mut out, "关键文件", &snapshot.critical_files);
            write_plan_list(&mut out, "约束条件", &snapshot.constraints);
            write_plan_field(
                &mut out,
                "推荐方法",
                snapshot.recommended_approach.as_deref(),
            );
            write_plan_field(
                &mut out,
                "验证计划",
                snapshot.verification_plan.as_deref(),
            );
            write_plan_field(
                &mut out,
                "风险和未知项",
                snapshot.risks_and_unknowns.as_deref(),
            );
            write_plan_field(
                &mut out,
                "交接包",
                snapshot.handoff_packet.as_deref(),
            );
            for item in snapshot.items {
                let _ = writeln!(out, "- [{}] {}", plan_status_label(&item.status), item.step);
            }
        }
    } else {
        let _ = writeln!(
            out,
            "\n策略元数据：由于计划状态正忙，无法获取。"
        );
    }

    let _ = writeln!(
        out,
        "\n写入之前，检查当前对话上下文和所需的实时工具证据。不要编造测试结果、文件变更、阻塞项或决策。"
    );
    let _ = writeln!(
        out,
        "\n使用以下紧凑结构：\n\
         # Session relay\n\
         \n\
         ## Goal\n\
         [用户的目标和任何明确的约束]\n\
         \n\
         ## Current work\n\
         [当前的待办项、进度以及进行中的工作]\n\
         \n\
         ## Files and state\n\
         [变更的文件、重要路径、子代理/RLM 会话、已运行的命令]\n\
         \n\
         ## Decisions\n\
         [关键选择的原因]\n\
         \n\
         ## Verification\n\
         [哪些测试通过、哪些失败、哪些未运行]\n\
         \n\
         ## Next action\n\
         [下一个线程的一个具体行动]"
    );
    let _ = writeln!(
        out,
        "\n除非会话确实需要更多，否则保持在约 900 词以内。写入后，报告路径和下一步行动。"
    );
    out
}

fn write_plan_field(out: &mut String, label: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        let _ = writeln!(out, "- {label}: {value}");
    }
}

fn write_plan_list(out: &mut String, label: &str, values: &[String]) {
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            let _ = writeln!(out, "- {label}: {value}");
        }
    }
}

fn plan_status_label(status: &crate::tools::plan::StepStatus) -> &'static str {
    match status {
        crate::tools::plan::StepStatus::Pending => "待处理",
        crate::tools::plan::StepStatus::InProgress => "进行中",
        crate::tools::plan::StepStatus::Completed => "已完成",
    }
}
