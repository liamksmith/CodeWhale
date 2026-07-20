//! 审批风险和影响程度策略。
//!
//! 本模块刻意不涉及 UI：它对工具调用进行分类，使审批和升级视图能够呈现决策，而无需拥有策略本身。

use crate::command_safety::is_parallel_readonly_command;
use serde_json::Value;

/// 按成本/风险级别对工具进行分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    /// 免费、只读操作（`list_dir`、`read_file`、todo_*）
    Safe,
    /// 文件修改（`write_file`、`edit_file`）
    FileWrite,
    /// Shell 执行（`exec_shell`）
    Shell,
    /// 面向网络的内置工具
    Network,
    /// 只读 MCP 发现和资源访问
    McpRead,
    /// 可能更改远程状态的 MCP 操作
    McpAction,
    /// 子代理生命周期（`agent` start/status/peek/cancel）；子代理自身的工具门控决定它实际能做什么。
    Agent,
    /// 未知或未分类的工具表面
    Unknown,
}

/// 基于影响程度的接管模态框变体。
///
/// `RiskLevel::Benign` 允许单次按键确认审批。
/// `RiskLevel::Destructive` 对可能触及文件、shell 或远程状态的审批保持更强的警告文案和样式。
///
/// 路由规则位于 [`classify_risk`] 中——有疑问时路由到 `Destructive`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Benign,
    Destructive,
}

/// 审批提示的展示级影响程度（#3883 后续）。
///
/// `RiskLevel` 驱动快捷键映射，并保持保守（"不能证明是只读的"即为 `Destructive`），
/// 但将该桶中的所有内容渲染为红色的 DESTRUCTIVE 接管，使得常规的文件编辑和构建命令
/// 看起来像是紧急情况。Stakes 将展示分为三个级别：
///
/// - `Routine` - 可证明是只读的；最小化界面元素。
/// - `Elevated` - 普通的状态触碰工作（编辑、构建、MCP 操作）；平静的审批，而非警告。
/// - `Critical` - 真正具有破坏性、类似发布或触及秘密的操作，依据 `ToolActionKind`；
///   保持强样式和策略语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStakes {
    Routine,
    Elevated,
    Critical,
}

/// 按名称获取工具的分类。
pub fn get_tool_category(name: &str) -> ToolCategory {
    if name == "agent" || name == "workflow" {
        // Workflow 是多代理编排；复用 Agent 的影响程度/路由
        // 并通过 build_impact_summary (#4126) 定制影响卡片。
        ToolCategory::Agent
    } else if matches!(name, "write_file" | "edit_file" | "apply_patch") {
        ToolCategory::FileWrite
    } else if matches!(
        name,
        "web_run" | "web_search" | "fetch_url" | "wait_for_dev_server"
    ) {
        ToolCategory::Network
    } else if matches!(
        name,
        "exec_shell"
            | "task_shell_start"
            | "task_shell_wait"
            | "exec_shell_wait"
            | "exec_shell_interact"
            | "exec_wait"
            | "exec_interact"
    ) {
        ToolCategory::Shell
    } else if name.starts_with("list_mcp_")
        || name.starts_with("read_mcp_")
        || name.starts_with("get_mcp_")
    {
        ToolCategory::McpRead
    } else if name.starts_with("mcp_") {
        ToolCategory::McpAction
    } else if matches!(
        name,
        "read_file"
            | "list_dir"
            | "work_update"
            | "todo_write"
            | "todo_read"
            | "checklist_write"
            | "note"
            | "update_plan"
            | "search"
            | "file_search"
            | "project"
            | "diagnostics"
    ) || name.starts_with("read_")
        || name.starts_with("list_")
        || name.starts_with("get_")
    {
        ToolCategory::Safe
    } else if name == "start_mcp_server" {
        // 启动 MCP 服务器会产生子进程或打开网络连接——归类为 McpAction 以触发适当的审批提示。
        ToolCategory::McpAction
    } else {
        ToolCategory::Unknown
    }
}

#[must_use]
pub fn classify_stakes(
    tool_name: &str,
    category: ToolCategory,
    risk: RiskLevel,
    params: &Value,
) -> ApprovalStakes {
    if matches!(risk, RiskLevel::Benign) {
        return ApprovalStakes::Routine;
    }
    match crate::tui::auto_review::ToolActionKind::from_tool_call(tool_name, params, category) {
        crate::tui::auto_review::ToolActionKind::Publish
        | crate::tui::auto_review::ToolActionKind::Destructive
        | crate::tui::auto_review::ToolActionKind::Secret => ApprovalStakes::Critical,
        _ => ApprovalStakes::Elevated,
    }
}

/// 决定审批请求的影响程度变体。
///
/// 偏向保守：我们不识别的分类路由到 `Destructive`，且任何被 `command_safety` 标记为
/// `Dangerous` 的 shell 命令即使请求其余部分看起来平静，也被强制为 `Destructive`。
/// 这种区分让模态框能够在可能触及本轮次之外状态的内容上渲染更强的警告文案。
#[must_use]
pub fn classify_risk(tool_name: &str, category: ToolCategory, params: &Value) -> RiskLevel {
    match category {
        // 读取路径和发现。
        ToolCategory::Safe | ToolCategory::McpRead => RiskLevel::Benign,
        // 仅查询的网络是良性的；打开 URL 会拉取任意的远程内容，因此保持破坏性。
        ToolCategory::Network => match tool_name {
            "web_search" | "wait_for_dev_server" => RiskLevel::Benign,
            // web_run 用于搜索/查询时是良性的，但其 `open`/`click` 操作会获取模型提供的 URL（任意远程内容）——破坏性，与 fetch_url 一致。
            "web_run" => {
                let fetches_url = params
                    .get("open")
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty())
                    || params
                        .get("click")
                        .and_then(Value::as_array)
                        .is_some_and(|a| !a.is_empty());
                if fetches_url {
                    RiskLevel::Destructive
                } else {
                    RiskLevel::Benign
                }
            }
            _ => RiskLevel::Destructive,
        },
        // Shell 保持破坏性，除非现有的命令安全分析器能证明具体命令是只读的。
        ToolCategory::Shell => {
            if let Some(cmd) = params.get("command").and_then(Value::as_str)
                && is_parallel_readonly_command(cmd)
            {
                return RiskLevel::Benign;
            }
            RiskLevel::Destructive
        }
        // 子代理生命周期：status/peek 仅用于检查。启动和其他操作保持显式选项的快捷键映射（子代理自身的门控在其运行后决定它能做什么）。
        ToolCategory::Agent => match params.get("action").and_then(Value::as_str) {
            Some("status" | "peek" | "list") => RiskLevel::Benign,
            _ => RiskLevel::Destructive,
        },
        // 文件写入、MCP 操作、未分类的表面——全部需要显式确认。
        ToolCategory::FileWrite | ToolCategory::McpAction | ToolCategory::Unknown => {
            RiskLevel::Destructive
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_read_only_surfaces_as_benign() {
        for name in ["read_file", "list_dir", "list_mcp_tools", "web_search"] {
            let category = get_tool_category(name);
            assert_eq!(
                classify_risk(name, category, &json!({})),
                RiskLevel::Benign,
                "{name}"
            );
        }
    }

    #[test]
    fn classifies_stateful_or_unknown_surfaces_as_destructive() {
        for name in [
            "write_file",
            "edit_file",
            "apply_patch",
            "mcp_linear_save_issue",
            "fetch_url",
            "unknown_tool",
        ] {
            let category = get_tool_category(name);
            assert_eq!(
                classify_risk(name, category, &json!({})),
                RiskLevel::Destructive,
                "{name}"
            );
        }
    }

    #[test]
    fn shell_risk_uses_command_safety_analysis() {
        let category = get_tool_category("exec_shell");
        assert_eq!(
            classify_risk(
                "exec_shell",
                category,
                &json!({"command": "git status --short"})
            ),
            RiskLevel::Benign
        );
        assert_eq!(
            classify_risk(
                "exec_shell",
                category,
                &json!({"command": "rm -rf /tmp/example"})
            ),
            RiskLevel::Destructive
        );
    }

    #[test]
    fn web_run_open_and_click_fetch_remote_content() {
        let category = get_tool_category("web_run");
        assert_eq!(
            classify_risk(
                "web_run",
                category,
                &json!({"search_query": [{"q": "rust"}]})
            ),
            RiskLevel::Benign
        );
        assert_eq!(
            classify_risk("web_run", category, &json!({"open": [{"ref_id": "x"}]})),
            RiskLevel::Destructive
        );
        assert_eq!(
            classify_risk(
                "web_run",
                category,
                &json!({"click": [{"ref_id": "x", "id": 1}]})
            ),
            RiskLevel::Destructive
        );
    }
}
