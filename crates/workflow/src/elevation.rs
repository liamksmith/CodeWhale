//! 升级工作流计划评估，用于审批卡（#4126）。
//!
//! 纯的、无 UI 的 [`WorkflowSpec`]（以及可选的规划器风险字符串）分析，
//! 以便调用方可以决定是否需要操作员审批卡以及该卡片应显示哪些字段。

use serde::{Deserialize, Serialize};

use crate::{
    IsolationMode, LeafSpec, PermissionSpec, TaskMode, WorkflowNode, WorkflowSpec,
    leaf_is_write_capable, leaf_wants_worktree,
};

/// 产品配置中的默认软 token 预算（`[workflow].default_token_budget`）。
/// 请求超过此值的计划被视为高预算。
pub const DEFAULT_HIGH_BUDGET_THRESHOLD: u64 = 120_000;

/// 在 IR 之外细化升级评估的选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElevationOptions {
    /// 在工具调用上声明的 token 预算（可能超过 `spec.budget`）。
    pub token_budget: Option<u64>,
    /// token 预算被视为高的阈值。
    pub high_budget_threshold: u64,
    /// 父会话当前是否允许写入。
    pub parent_allows_write: bool,
    /// 父会话当前是否允许网络。
    pub parent_allows_network: bool,
}

impl Default for ElevationOptions {
    fn default() -> Self {
        Self {
            token_budget: None,
            high_budget_threshold: DEFAULT_HIGH_BUDGET_THRESHOLD,
            // 除非调用方缩小姿态，否则假设 Act/读写父级。
            parent_allows_write: true,
            parent_allows_network: true,
        }
    }
}

/// 工作流计划为何需要（或不需要）升级审批的摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowPlanElevation {
    pub elevated: bool,
    pub goal: String,
    pub child_count: usize,
    pub child_summary: String,
    pub writes: bool,
    pub shell: bool,
    pub network: bool,
    pub secrets: bool,
    pub worktree: bool,
    pub high_budget: bool,
    pub broader_authority: bool,
    /// 审批卡的人类可读预算行。
    pub budget_label: String,
    /// 区别性的升级原因（用于审计/影响行）。
    pub reasons: Vec<String>,
}

impl WorkflowPlanElevation {
    /// TUI 审批模态框使用的卡片字段标签/值（#4126）。
    #[must_use]
    pub fn card_fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("Goal", self.goal.clone()),
            ("Children", self.child_summary.clone()),
            ("Writes", yes_no(self.writes)),
            ("Shell", yes_no(self.shell)),
            ("Network", yes_no(self.network)),
            ("Budget", self.budget_label.clone()),
        ]
    }

    /// 计划是否完全在只读封装内。
    #[must_use]
    pub fn is_read_only_envelope(&self) -> bool {
        !self.elevated
            && !self.writes
            && !self.shell
            && !self.network
            && !self.secrets
            && !self.worktree
            && !self.high_budget
            && !self.broader_authority
    }
}

fn yes_no(flag: bool) -> String {
    if flag {
        "yes".to_string()
    } else {
        "no".to_string()
    }
}

/// 评估编译后的 [`WorkflowSpec`]。
#[must_use]
pub fn assess_workflow_elevation(
    spec: &WorkflowSpec,
    options: ElevationOptions,
) -> WorkflowPlanElevation {
    let mut child_ids = Vec::new();
    let mut writes = false;
    let mut shell = false;
    let mut network = false;
    let mut secrets = false;
    let mut worktree = false;

    walk_nodes(
        &spec.nodes,
        /* parallel */ false,
        &mut child_ids,
        &mut writes,
        &mut shell,
        &mut network,
        &mut secrets,
        &mut worktree,
    );

    // spec 级权限也会提升级别。
    merge_permissions(
        &spec.permissions,
        &mut writes,
        &mut shell,
        &mut network,
        &mut secrets,
    );

    // 规划器风险字符串在结构化计划降低器处理时存储在 `description` 上
    //（当存在时：`risk: elevated|writes|shell|network|…`）。
    apply_plan_risk_hint(
        spec.description.as_deref(),
        &mut writes,
        &mut shell,
        &mut network,
    );

    let effective_tokens = options
        .token_budget
        .or(spec.budget.max_tokens)
        .filter(|n| *n > 0);
    let high_budget = effective_tokens.is_some_and(|n| n > options.high_budget_threshold);

    let broader_authority =
        (!options.parent_allows_write && writes) || (!options.parent_allows_network && network);

    let mut reasons = Vec::new();
    if writes {
        reasons.push("writes".to_string());
    }
    if shell {
        reasons.push("shell".to_string());
    }
    if network {
        reasons.push("network".to_string());
    }
    if secrets {
        reasons.push("secrets".to_string());
    }
    if worktree {
        reasons.push("worktree".to_string());
    }
    if high_budget {
        reasons.push("high_budget".to_string());
    }
    if broader_authority {
        reasons.push("broader_authority".to_string());
    }

    let elevated = !reasons.is_empty();
    let child_count = child_ids.len();
    let child_summary = if child_ids.is_empty() {
        "0 children".to_string()
    } else if child_ids.len() <= 4 {
        format!(
            "{} child{}: {}",
            child_ids.len(),
            if child_ids.len() == 1 { "" } else { "ren" },
            child_ids.join(", ")
        )
    } else {
        format!(
            "{} children: {}, {}… (+{})",
            child_ids.len(),
            child_ids[0],
            child_ids[1],
            child_ids.len() - 2
        )
    };

    let budget_label = format_budget_label(effective_tokens, &spec.budget, high_budget);

    WorkflowPlanElevation {
        elevated,
        goal: spec.goal.clone(),
        child_count,
        child_summary,
        writes,
        shell,
        network,
        secrets,
        worktree,
        high_budget,
        broader_authority,
        budget_label,
        reasons,
    }
}

/// 仅从规划器 `risk` 字符串进行的轻量级评估（在 IR 之前）。
#[must_use]
pub fn assess_plan_risk_string(risk: Option<&str>) -> PlanRiskHint {
    match risk.map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("read_only") | Some("readonly") | Some("low") | Some("safe") => {
            PlanRiskHint::ReadOnly
        }
        Some("writes") | Some("write") | Some("read_write") | Some("readwrite")
        | Some("medium") => PlanRiskHint::Writes,
        Some("shell") => PlanRiskHint::Shell,
        Some("network") => PlanRiskHint::Network,
        Some("elevated") | Some("high") => PlanRiskHint::Elevated,
        Some(_) => PlanRiskHint::Elevated,
    }
}

/// 从结构化计划 `risk` 字段得到的粗略风险分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanRiskHint {
    ReadOnly,
    Writes,
    Shell,
    Network,
    Elevated,
}

impl PlanRiskHint {
    #[must_use]
    pub fn elevates(self) -> bool {
        !matches!(self, Self::ReadOnly)
    }
}

fn apply_plan_risk_hint(
    risk: Option<&str>,
    writes: &mut bool,
    shell: &mut bool,
    network: &mut bool,
) {
    match assess_plan_risk_string(risk) {
        PlanRiskHint::ReadOnly => {}
        PlanRiskHint::Writes => *writes = true,
        PlanRiskHint::Shell => {
            *shell = true;
            *writes = true;
        }
        PlanRiskHint::Network => {
            *network = true;
        }
        PlanRiskHint::Elevated => {
            *writes = true;
            *shell = true;
            *network = true;
        }
    }
}

fn format_budget_label(
    effective_tokens: Option<u64>,
    budget: &crate::BudgetSpec,
    high_budget: bool,
) -> String {
    let mut parts = Vec::new();
    if let Some(tokens) = effective_tokens {
        parts.push(format!("{tokens} tokens"));
    }
    if let Some(steps) = budget.max_steps {
        parts.push(format!("max_steps={steps}"));
    }
    if let Some(timeout) = budget.timeout_secs {
        parts.push(format!("timeout={timeout}s"));
    }
    if let Some(parallel) = budget.max_parallel {
        parts.push(format!("max_parallel={parallel}"));
    }
    if parts.is_empty() {
        "default".to_string()
    } else if high_budget {
        format!("{} (high)", parts.join(", "))
    } else {
        parts.join(", ")
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_nodes(
    nodes: &[WorkflowNode],
    parallel: bool,
    child_ids: &mut Vec<String>,
    writes: &mut bool,
    shell: &mut bool,
    network: &mut bool,
    secrets: &mut bool,
    worktree: &mut bool,
) {
    for node in nodes {
        match node {
            WorkflowNode::Leaf(leaf) => {
                inspect_leaf(
                    leaf, parallel, child_ids, writes, shell, network, secrets, worktree,
                );
            }
            WorkflowNode::BranchSet(branch) => {
                merge_permissions(&branch.permissions, writes, shell, network, secrets);
                walk_nodes(
                    &branch.children,
                    branch.parallel || parallel,
                    child_ids,
                    writes,
                    shell,
                    network,
                    secrets,
                    worktree,
                );
            }
            WorkflowNode::Sequence(seq) => {
                walk_nodes(
                    &seq.children,
                    parallel,
                    child_ids,
                    writes,
                    shell,
                    network,
                    secrets,
                    worktree,
                );
            }
            WorkflowNode::LoopUntil(loop_spec) => {
                walk_nodes(
                    &loop_spec.children,
                    parallel,
                    child_ids,
                    writes,
                    shell,
                    network,
                    secrets,
                    worktree,
                );
            }
            WorkflowNode::Cond(cond) => {
                walk_nodes(
                    &cond.then_nodes,
                    parallel,
                    child_ids,
                    writes,
                    shell,
                    network,
                    secrets,
                    worktree,
                );
                walk_nodes(
                    &cond.else_nodes,
                    parallel,
                    child_ids,
                    writes,
                    shell,
                    network,
                    secrets,
                    worktree,
                );
            }
            WorkflowNode::Expand(expand) => {
                if let Some(template) = expand.template.as_deref() {
                    walk_nodes(
                        std::slice::from_ref(template),
                        parallel,
                        child_ids,
                        writes,
                        shell,
                        network,
                        secrets,
                        worktree,
                    );
                }
            }
            WorkflowNode::Reduce(_) | WorkflowNode::TeacherReview(_) => {
                // 控制/归约节点本身不会产生支持写入的叶子。
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn inspect_leaf(
    leaf: &LeafSpec,
    parallel: bool,
    child_ids: &mut Vec<String>,
    writes: &mut bool,
    shell: &mut bool,
    network: &mut bool,
    secrets: &mut bool,
    worktree: &mut bool,
) {
    child_ids.push(leaf.id.clone());
    if leaf_is_write_capable(leaf) {
        *writes = true;
    }
    merge_permissions(&leaf.permissions, writes, shell, network, secrets);
    if leaf_wants_worktree(leaf, parallel) || matches!(leaf.isolation, IsolationMode::Worktree) {
        *worktree = true;
    }
    // 显式的 read_write 模式与 shell 工具已处理；没有工具拒绝列表的
    // 实现者可以运行 shell。
    if leaf.mode == TaskMode::ReadWrite
        && leaf.permissions.allowed_tools.is_empty()
        && matches!(
            leaf.agent_type,
            crate::AgentType::Implementer | crate::AgentType::General
        )
    {
        // 支持写入的实现者/通用代理可能超越只读运行 shell——
        // 在审批卡上将 shell 标记为升级项。
        *shell = true;
    }
}

fn merge_permissions(
    permissions: &PermissionSpec,
    writes: &mut bool,
    shell: &mut bool,
    network: &mut bool,
    secrets: &mut bool,
) {
    if permissions.allow_write {
        *writes = true;
    }
    if permissions.allow_network {
        *network = true;
    }
    for tool in &permissions.allowed_tools {
        let name = tool.trim();
        if is_write_tool(name) {
            *writes = true;
        }
        if is_shell_tool(name) {
            *shell = true;
        }
        if is_network_tool(name) {
            *network = true;
        }
        if is_secret_tool(name) {
            *secrets = true;
        }
    }
}

fn is_write_tool(tool: &str) -> bool {
    matches!(
        tool,
        "write_file" | "edit_file" | "apply_patch" | "checklist_write" | "todo_write"
    )
}

fn is_shell_tool(tool: &str) -> bool {
    matches!(
        tool,
        "exec_shell"
            | "exec_shell_wait"
            | "exec_shell_interact"
            | "exec_wait"
            | "exec_interact"
            | "task_shell_start"
            | "task_shell_wait"
    )
}

fn is_network_tool(tool: &str) -> bool {
    matches!(
        tool,
        "web_search" | "web_run" | "fetch_url" | "wait_for_dev_server"
    ) || tool.starts_with("mcp_")
}

fn is_secret_tool(tool: &str) -> bool {
    let lower = tool.to_ascii_lowercase();
    lower.contains("secret")
        || lower.contains("credential")
        || lower.contains("password")
        || lower == "read_env"
        || lower == "env"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentType, BranchSpec, BudgetSpec, LeafSpec, ModelPolicy, PermissionSpec, PromotionPolicy,
        SequenceSpec, TaskMode,
    };

    fn leaf(id: &str, mode: TaskMode) -> LeafSpec {
        LeafSpec {
            id: id.to_string(),
            prompt: format!("do {id}"),
            agent_type: if mode == TaskMode::ReadWrite {
                AgentType::Implementer
            } else {
                AgentType::Explore
            },
            profile: None,
            role: None,
            mode,
            isolation: IsolationMode::Auto,
            file_scope: Vec::new(),
            depends_on_results: Vec::new(),
            budget: BudgetSpec::default(),
            permissions: PermissionSpec::default(),
            model_policy: ModelPolicy::default(),
        }
    }

    fn spec_with(nodes: Vec<WorkflowNode>, risk: Option<&str>) -> WorkflowSpec {
        WorkflowSpec {
            id: Some("test".to_string()),
            goal: "ship feature".to_string(),
            description: risk.map(str::to_string),
            budget: BudgetSpec::default(),
            permissions: PermissionSpec::default(),
            model_policy: ModelPolicy::default(),
            promotion_policy: PromotionPolicy::default(),
            gates: Vec::new(),
            nodes,
        }
    }

    #[test]
    fn read_only_plan_is_not_elevated() {
        let spec = spec_with(
            vec![WorkflowNode::Leaf(leaf("scan", TaskMode::ReadOnly))],
            Some("read_only"),
        );
        let elevation = assess_workflow_elevation(&spec, ElevationOptions::default());
        assert!(!elevation.elevated, "{elevation:?}");
        assert!(elevation.is_read_only_envelope());
        assert_eq!(elevation.goal, "ship feature");
        assert!(elevation.child_summary.contains("scan"));
        assert!(!elevation.writes);
        assert!(!elevation.shell);
        assert!(!elevation.network);
        let fields = elevation.card_fields();
        assert_eq!(fields.len(), 6);
        assert!(
            fields
                .iter()
                .any(|(k, v)| *k == "Goal" && v == "ship feature")
        );
        assert!(fields.iter().any(|(k, v)| *k == "Writes" && v == "no"));
    }

    #[test]
    fn write_plan_elevates_and_flags_shell_for_implementer() {
        let spec = spec_with(
            vec![WorkflowNode::Leaf(leaf("impl", TaskMode::ReadWrite))],
            Some("writes"),
        );
        let elevation = assess_workflow_elevation(&spec, ElevationOptions::default());
        assert!(elevation.elevated);
        assert!(elevation.writes);
        assert!(elevation.shell);
        assert!(elevation.reasons.iter().any(|r| r == "writes"));
    }

    #[test]
    fn network_and_secrets_tools_elevate() {
        let mut network_leaf = leaf("fetch", TaskMode::ReadOnly);
        network_leaf.permissions.allow_network = true;
        network_leaf.permissions.allowed_tools = vec!["fetch_url".to_string()];

        let mut secret_leaf = leaf("creds", TaskMode::ReadOnly);
        secret_leaf.permissions.allowed_tools = vec!["read_secret".to_string()];

        let spec = spec_with(
            vec![WorkflowNode::Sequence(SequenceSpec {
                id: "seq".to_string(),
                children: vec![
                    WorkflowNode::Leaf(network_leaf),
                    WorkflowNode::Leaf(secret_leaf),
                ],
            })],
            None,
        );
        let elevation = assess_workflow_elevation(&spec, ElevationOptions::default());
        assert!(elevation.elevated);
        assert!(elevation.network);
        assert!(elevation.secrets);
        assert!(elevation.reasons.iter().any(|r| r == "network"));
        assert!(elevation.reasons.iter().any(|r| r == "secrets"));
    }

    #[test]
    fn parallel_write_children_flag_worktree() {
        let left = leaf("left", TaskMode::ReadWrite);
        let right = leaf("right", TaskMode::ReadWrite);
        let spec = spec_with(
            vec![WorkflowNode::BranchSet(BranchSpec {
                id: "parallel".to_string(),
                description: None,
                parallel: true,
                budget: BudgetSpec::default(),
                permissions: PermissionSpec::default(),
                model_policy: ModelPolicy::default(),
                children: vec![WorkflowNode::Leaf(left), WorkflowNode::Leaf(right)],
            })],
            Some("writes"),
        );
        let elevation = assess_workflow_elevation(&spec, ElevationOptions::default());
        assert!(elevation.worktree, "{elevation:?}");
        assert!(elevation.writes);
    }

    #[test]
    fn high_budget_elevates() {
        let mut spec = spec_with(
            vec![WorkflowNode::Leaf(leaf("scan", TaskMode::ReadOnly))],
            Some("read_only"),
        );
        spec.budget.max_tokens = Some(250_000);
        let elevation = assess_workflow_elevation(&spec, ElevationOptions::default());
        assert!(elevation.high_budget);
        assert!(elevation.elevated);
        assert!(elevation.budget_label.contains("high"));
    }

    #[test]
    fn broader_authority_when_parent_is_read_only() {
        let spec = spec_with(
            vec![WorkflowNode::Leaf(leaf("impl", TaskMode::ReadWrite))],
            Some("writes"),
        );
        let elevation = assess_workflow_elevation(
            &spec,
            ElevationOptions {
                parent_allows_write: false,
                parent_allows_network: false,
                ..ElevationOptions::default()
            },
        );
        assert!(elevation.broader_authority);
        assert!(elevation.reasons.iter().any(|r| r == "broader_authority"));
    }

    #[test]
    fn plan_risk_string_classifies_elevated_variants() {
        assert_eq!(
            assess_plan_risk_string(Some("read_only")),
            PlanRiskHint::ReadOnly
        );
        assert_eq!(
            assess_plan_risk_string(Some("writes")),
            PlanRiskHint::Writes
        );
        assert_eq!(assess_plan_risk_string(Some("shell")), PlanRiskHint::Shell);
        assert_eq!(
            assess_plan_risk_string(Some("network")),
            PlanRiskHint::Network
        );
        assert_eq!(
            assess_plan_risk_string(Some("elevated")),
            PlanRiskHint::Elevated
        );
        assert!(assess_plan_risk_string(Some("elevated")).elevates());
        assert!(!assess_plan_risk_string(Some("read_only")).elevates());
    }

    #[test]
    fn card_fields_always_include_required_labels() {
        let spec = spec_with(
            vec![WorkflowNode::Leaf(leaf("a", TaskMode::ReadOnly))],
            None,
        );
        let fields = assess_workflow_elevation(&spec, ElevationOptions::default()).card_fields();
        let labels: Vec<_> = fields.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            labels,
            vec!["Goal", "Children", "Writes", "Shell", "Network", "Budget"]
        );
    }
}
