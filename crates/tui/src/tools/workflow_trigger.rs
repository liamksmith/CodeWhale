//! 自动工作流触发和抑制启发式方法（#4127）。
//!
//! 软自动模型：**代理**决定使用工作流，而无需操作员
//! 说出"工作流"这个词。策略在此回答"是否应编排？"——
//! 父提示词仍然**告知操作员**预期的形状，并可能在调用
//! `workflow` / `plan` 之前通过 `request_user_input`（TUI 模态框）询问设置问题。
//!
//! 纯决策辅助；自身不启动工作流。公共 API 对
//! 单元测试和即将到来的运行时连接是实时的 —— 通过
//! [`soft_auto_policy_is_linked`] 保持从非测试构建可达。

// 软自动策略主要由模型（提示词）和今天测试使用；
// 运行时自动启动将在下一步调用 [`evaluate_workflow_trigger`]。在此之前，
// 私有辅助方法会在 `-D warnings` 下的二进制构建中触发 `dead_code`。
#![allow(dead_code)]

/// 父级可以在无需完整对话回放的情况下提供的信号。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowTriggerSignals {
    /// 当前请求的大致打开文件/编辑范围数量。
    pub distinct_file_scopes: usize,
    /// 当操作员处于交互式多轮设计/聊天中时为 true。
    pub highly_interactive: bool,
    /// 当请求需要写入但没有清晰阶段/子任务分解时为 true。
    pub risky_writes_unclear_decomposition: bool,
    /// 如果现在启动工作流，估计的子任务数量。
    pub estimated_children: usize,
    /// 来自 `[workflow].auto_start_child_limit` 的软限制（默认 8）。
    pub auto_start_child_limit: usize,
    /// 正在使用的近似父上下文 token 数（用于高容量信号）。
    pub context_tokens: usize,
    /// 高上下文量有利于工作流的阈值。
    pub high_context_token_threshold: usize,
}

impl WorkflowTriggerSignals {
    #[must_use]
    pub fn product_defaults() -> Self {
        Self {
            distinct_file_scopes: 0,
            highly_interactive: false,
            risky_writes_unclear_decomposition: false,
            estimated_children: 0,
            auto_start_child_limit: 8,
            context_tokens: 0,
            high_context_token_threshold: 80_000,
        }
    }
}

/// 自动工作流启动/推荐的决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowTriggerDecision {
    /// 启动或推荐工作流。
    Trigger { reason: &'static str },
    /// 抑制自动工作流；首选直接工具/单代理。
    Suppress { reason: &'static str },
}

impl WorkflowTriggerDecision {
    #[must_use]
    pub fn should_trigger(&self) -> bool {
        matches!(self, Self::Trigger { .. })
    }

    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Trigger { reason } | Self::Suppress { reason } => reason,
        }
    }
}

/// 评估自动工作流是否适合此用户请求。
///
/// 当两者都可能适用时，抑制优先于触发（嘈杂的自动编排
/// 比错过扇出更糟糕）。Agent/Operate 模式中的提示词指导
/// 应与这些规则保持一致。
#[must_use]
pub fn evaluate_workflow_trigger(
    user_text: &str,
    signals: &WorkflowTriggerSignals,
) -> WorkflowTriggerDecision {
    let text = user_text.trim();
    let lower = text.to_ascii_lowercase();

    // --- 硬抑制（AC） ---
    if signals.highly_interactive {
        return WorkflowTriggerDecision::Suppress {
            reason: "highly interactive task — keep turn-by-turn",
        };
    }
    if signals.risky_writes_unclear_decomposition {
        return WorkflowTriggerDecision::Suppress {
            reason: "risky writes without clear decomposition",
        };
    }
    if signals.estimated_children > 0
        && signals.auto_start_child_limit > 0
        && signals.estimated_children > signals.auto_start_child_limit
    {
        return WorkflowTriggerDecision::Suppress {
            reason: "estimated children exceed auto_start_child_limit",
        };
    }
    if child_overhead_exceeds_benefit(&lower, signals) {
        return WorkflowTriggerDecision::Suppress {
            reason: "child overhead greater than benefit",
        };
    }
    if is_simple_command_or_factual_question(&lower, text) {
        return WorkflowTriggerDecision::Suppress {
            reason: "simple command or factual question",
        };
    }
    if is_one_file_edit(&lower, signals) {
        return WorkflowTriggerDecision::Suppress {
            reason: "one-file edit — use direct tools",
        };
    }

    // --- 触发条件（AC） ---
    if signals.distinct_file_scopes >= 3 {
        return WorkflowTriggerDecision::Trigger {
            reason: "independent scopes across multiple files",
        };
    }
    if signals.context_tokens >= signals.high_context_token_threshold {
        return WorkflowTriggerDecision::Trigger {
            reason: "high context volume favors staged Workflow",
        };
    }
    if has_fanout_language(&lower) {
        return WorkflowTriggerDecision::Trigger {
            reason: "audit/sweep/compare/fan-out language",
        };
    }
    if has_staged_work_language(&lower) {
        return WorkflowTriggerDecision::Trigger {
            reason: "staged multi-phase work",
        };
    }
    if has_independent_verification_language(&lower) {
        return WorkflowTriggerDecision::Trigger {
            reason: "independent verification pass",
        };
    }

    WorkflowTriggerDecision::Suppress {
        reason: "no automatic Workflow trigger matched",
    }
}

fn child_overhead_exceeds_benefit(lower: &str, signals: &WorkflowTriggerSignals) -> bool {
    // 微小的请求或明确的单步语言 —— 启动成本占主导地位。
    if signals.estimated_children == 1 {
        return true;
    }
    if lower.len() < 24 && !has_fanout_language(lower) && !has_staged_work_language(lower) {
        return true;
    }
    let tiny = [
        "fix typo",
        "rename variable",
        "one liner",
        "one-liner",
        "quick peek",
        "just check",
    ];
    tiny.iter().any(|needle| lower.contains(needle))
}

fn is_simple_command_or_factual_question(lower: &str, original: &str) -> bool {
    if lower.starts_with('/') {
        // 斜杠命令是 UI 路由，不是编排。
        return true;
    }
    let factual_prefixes = [
        "what is ",
        "what's ",
        "whats ",
        "who is ",
        "when is ",
        "where is ",
        "how many ",
        "which ",
        "define ",
        "explain ",
    ];
    if factual_prefixes.iter().any(|p| lower.starts_with(p)) && original.len() < 160 {
        return true;
    }
    let simple_cmds = [
        "run tests",
        "run the tests",
        "cargo test",
        "cargo check",
        "git status",
        "git log",
        "git diff",
        "ls ",
        "pwd",
        "show version",
        "print version",
    ];
    if simple_cmds
        .iter()
        .any(|c| lower == *c || lower.starts_with(&format!("{c} ")))
    {
        return true;
    }
    // 简短的是/否或状态 ping。
    matches!(
        lower.trim_end_matches(['?', '.', '!']),
        "ok" | "thanks" | "thank you" | "status" | "ping" | "hello" | "hi"
    )
}

fn is_one_file_edit(lower: &str, signals: &WorkflowTriggerSignals) -> bool {
    if signals.distinct_file_scopes == 1 {
        let editish = [
            "edit ",
            "fix ",
            "patch ",
            "update ",
            "change ",
            "rewrite ",
            "in this file",
            "this file",
            "only this file",
            "single file",
            "one file",
        ];
        return editish.iter().any(|n| lower.contains(n));
    }
    // 没有范围信号的显式单文件措辞。
    lower.contains("only this file")
        || lower.contains("just this file")
        || lower.contains("single file")
        || (lower.contains("one file") && !has_fanout_language(lower))
}

fn has_fanout_language(lower: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "audit",
        "sweep",
        "compare",
        "fan-out",
        "fan out",
        "fanout",
        "in parallel",
        "parallel across",
        "across the codebase",
        "across packages",
        "across crates",
        "every crate",
        "all packages",
        "all modules",
        "multi-repo",
        "multi repo",
    ];
    NEEDLES.iter().any(|n| lower.contains(n))
}

fn has_staged_work_language(lower: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "phase 1",
        "phase 2",
        "first implement",
        "then verify",
        "implement then",
        "staged",
        "multi-phase",
        "multi phase",
        "plan then execute",
        "explore then implement",
        "scout then",
    ];
    NEEDLES.iter().any(|n| lower.contains(n))
}

fn has_independent_verification_language(lower: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "independent verification",
        "verify independently",
        "separate verifier",
        "second pair of eyes",
        "review in parallel",
        "verify in parallel",
        "independent review",
    ];
    NEEDLES.iter().any(|n| lower.contains(n))
}

/// 可达性探测，以便软自动表面在发布构建中保持链接。
///
/// 当典型扇出请求在产品默认值下触发工作流时返回 `true`
///（由注册表/工具连接冒烟测试使用）。
#[must_use]
pub fn soft_auto_policy_is_linked() -> bool {
    evaluate_workflow_trigger(
        "audit every crate for unsafe blocks",
        &WorkflowTriggerSignals::product_defaults(),
    )
    .should_trigger()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals() -> WorkflowTriggerSignals {
        WorkflowTriggerSignals::product_defaults()
    }

    #[test]
    fn suppresses_one_file_edits() {
        let mut s = signals();
        s.distinct_file_scopes = 1;
        let d = evaluate_workflow_trigger("fix the typo in this file", &s);
        assert!(!d.should_trigger(), "{d:?}");
        assert!(d.reason().contains("one-file"));
    }

    #[test]
    fn suppresses_simple_commands_and_factual_questions() {
        let s = signals();
        for ask in [
            "cargo test",
            "git status",
            "what is a worktree?",
            "how many crates are there?",
            "/help",
            "thanks",
        ] {
            let d = evaluate_workflow_trigger(ask, &s);
            assert!(!d.should_trigger(), "expected suppress for {ask:?}: {d:?}");
        }
    }

    #[test]
    fn suppresses_highly_interactive_and_unclear_risky_writes() {
        let mut s = signals();
        s.highly_interactive = true;
        assert!(!evaluate_workflow_trigger("redesign the product with me", &s).should_trigger());

        s = signals();
        s.risky_writes_unclear_decomposition = true;
        assert!(!evaluate_workflow_trigger("make it better somehow", &s).should_trigger());
    }

    #[test]
    fn suppresses_when_child_overhead_dominates() {
        let mut s = signals();
        s.estimated_children = 1;
        assert!(!evaluate_workflow_trigger("quick peek at main.rs", &s).should_trigger());

        s = signals();
        s.estimated_children = 20;
        s.auto_start_child_limit = 8;
        let d = evaluate_workflow_trigger("audit the whole monorepo", &s);
        assert!(!d.should_trigger(), "{d:?}");
        assert!(d.reason().contains("auto_start_child_limit"));
    }

    #[test]
    fn triggers_on_fanout_and_staged_language() {
        let s = signals();
        for ask in [
            "audit every crate for unsafe blocks",
            "sweep the codebase for TODO debt",
            "compare the two provider implementations in parallel",
            "phase 1 explore then phase 2 implement",
            "run an independent verification of the release notes",
        ] {
            let d = evaluate_workflow_trigger(ask, &s);
            assert!(d.should_trigger(), "expected trigger for {ask:?}: {d:?}");
        }
    }

    #[test]
    fn triggers_on_independent_scopes_and_high_context() {
        let mut s = signals();
        s.distinct_file_scopes = 5;
        assert!(
            evaluate_workflow_trigger("touch the related modules carefully", &s).should_trigger()
        );

        s = signals();
        s.context_tokens = 120_000;
        assert!(evaluate_workflow_trigger("continue the migration plan", &s).should_trigger());
    }

    #[test]
    fn suppression_wins_over_fanout_language_when_interactive() {
        let mut s = signals();
        s.highly_interactive = true;
        let d = evaluate_workflow_trigger("let's design an audit sweep together", &s);
        assert!(!d.should_trigger(), "{d:?}");
        assert!(d.reason().contains("interactive"));
    }
}
