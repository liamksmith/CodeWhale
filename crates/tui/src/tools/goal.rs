//! 用于模型可见的“LLM 即评判者”循环的目标工具。
//!
//! TUI 已经有一个 `/goal` 命令，并将其目标（objective）传递到引擎提示词中。
//! 本模块将运行时切片（runtime slice）单独分离出来：一个小的会话级状态对象，
//! 外加模型可用于检查和结束该状态的工具。

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str,
};

/// 单个引擎轮次（turn）内，自动目标续传提示词注入的最大次数。
/// 这仅仅是轮次内的粒度限制——用于防止单个轮次内因卡住而空转、毫无进展。
/// 跨轮次的循环**没有上限**：目标会一直运行，直到完成、阻塞、暂停，
/// 或可选的预算耗尽为止。参见 `goal_loop::decide_continuation`。
pub const MAX_GOAL_CONTINUATIONS_PER_TURN: u32 = 3;

/// 当前运行时目标的共享引用。
pub type SharedGoalState = Arc<Mutex<GoalState>>;

/// 创建一个空的共享目标状态。
#[must_use]
pub fn new_shared_goal_state() -> SharedGoalState {
    Arc::new(Mutex::new(GoalState::default()))
}

/// 基于宿主目标表面（host goal surface）的内容，以一个显式状态（explicit status）来创建共享状态。
#[must_use]
pub fn new_shared_goal_state_from_host_status(
    objective: Option<String>,
    token_budget: Option<u32>,
    status: GoalStatus,
) -> SharedGoalState {
    let mut state = GoalState::default();
    state.sync_from_host_status(objective.as_deref(), token_budget, status);
    Arc::new(Mutex::new(state))
}

/// 目标的运行时状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    Active,
    Paused,
    Complete,
    Blocked,
}

impl GoalStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Complete => "complete",
            Self::Blocked => "blocked",
        }
    }
}

/// 会话本地的目标状态。`Instant` 仅保留在运行时；快照会暴露经过的秒数，
/// 以确保工具输出保持可序列化和稳定。
#[derive(Debug, Clone, Default)]
pub struct GoalState {
    /// 目标
    objective: Option<String>,
    /// token预算
    token_budget: Option<u32>,
    /// 状态
    status: Option<GoalStatus>,
    /// 已用 token
    tokens_used: u64,
    /// 已用时间
    time_used_seconds: u64,
    /// 继续次数
    continuation_count: u32,
    /// 开始时间
    started_at: Option<Instant>,
    /// 完成时间
    finished_at: Option<Instant>,
    /// 完成证据
    evidence: Option<String>,
    /// 阻塞原因
    blocker: Option<String>,
    /// 验证信息
    completion_verification: Option<GoalCompletionVerification>,
}

impl GoalState {
    #[must_use]
    pub fn objective(&self) -> Option<&str> {
        self.objective.as_deref()
    }

    #[must_use]
    pub fn token_budget(&self) -> Option<u32> {
        self.token_budget
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.objective.is_some() && self.status == Some(GoalStatus::Active)
    }

    /// 它的作用是把"宿主进程"（TUI/CLI）设定的目标信息注入到引擎内部的`SharedGoalState`中值
    /// ————是一个"被动接收"的同步桥，引擎自己不会修改目标的语义内容，只从外部权威源拉取最新
    /// 的目标描述、token预算和状态。如果目标变了，重置计数器；如果目标被清空，清除所有状态；如果只是调预算，
    /// 只改预算不动其他。
    /// 它在引擎中的两个调用点
    /// 调用点 1：引擎启动时（engine.rs 第 874 行）Engine::new() 中
    /// 通俗理解： 引擎刚创建时，如果用户配置中设了目标（例如 codewhale --goal "重构 user   
    /// 模块" --goal-budget 100000），就把这个初始目标写入共享状态。
    /// 调用点 2：每轮对话开始时（engine.rs 第 2559 行）
    /// 通俗理解： 每次 AI开始新一轮对话前，如果目标参数发生了变化（用户改目标了、暂停了、完成了），就把最新值
    /// 同步到共享状态。这确保了后续的 goal_continuation_if_active() 能读到正确的目标信息。
    pub fn sync_from_host_status(
        &mut self,
        objective: Option<&str>,
        token_budget: Option<u32>,
        status: GoalStatus,
    ) {
        let objective = objective.map(str::trim).filter(|value| !value.is_empty());
        match objective {
            // ── 情况 A：传入了一个非空目标 ── 
            Some(objective) => {
                let changed = self.objective.as_deref() != Some(objective);
                let status_changed = self.status != Some(status);
                if changed {
                    // 目标内容变了 → 全新的目标，重置一切统计
                    self.objective = Some(objective.to_string());
                    self.token_budget = token_budget;
                    self.tokens_used = 0;             // 已用 token 归零
                    self.time_used_seconds = 0;       // 已用时间归零
                    self.continuation_count = 0;      // 继续次数归零
                    self.started_at = Some(Instant::now());   // 记录开始时间
                    self.evidence = None;             // 完成证据清空
                    self.blocker = None;              // 阻塞原因清空
                    self.completion_verification = None;   // 验证信息清空 
                } else if self.token_budget != token_budget {
                    self.token_budget = token_budget;   // 目标没变，只是调整了预算 → 只更新预算
                }
                
                if changed || status_changed || self.status.is_none() {
                    self.status = Some(status);
                    self.finished_at = if status == GoalStatus::Active {
                        None   // Active → 没有结束时间
                    } else {
                        Some(Instant::now())  // 非 Active → 记录结束时间
                    };
                }
            }
            // ── 情况 B：没传目标 → 清空整个目标状态 ──
            None => self.clear(),
        }
    }

    pub fn create(&mut self, objective: String, token_budget: Option<u32>) {
        self.objective = Some(objective);
        self.token_budget = token_budget;
        self.status = Some(GoalStatus::Active);
        self.tokens_used = 0;
        self.time_used_seconds = 0;
        self.continuation_count = 0;
        self.started_at = Some(Instant::now());
        self.finished_at = None;
        self.evidence = None;
        self.blocker = None;
        self.completion_verification = None;
    }

    pub fn record_usage(&mut self, token_delta: u64, time_delta_seconds: u64) {
        if self.is_active() {
            self.tokens_used = self.tokens_used.saturating_add(token_delta);
            self.time_used_seconds = self.time_used_seconds.saturating_add(time_delta_seconds);
        }
    }

    /// 继续次数加一
    pub fn record_continuation(&mut self) {
        if self.is_active() {
            self.continuation_count = self.continuation_count.saturating_add(1);
        }
    }

    pub fn mark_complete(
        &mut self,
        evidence: String,
        verification: GoalCompletionVerification,
    ) -> Result<(), &'static str> {
        if self.objective.is_none() {
            return Err("No active goal exists to complete.");
        }
        self.status = Some(GoalStatus::Complete);
        self.finished_at = Some(Instant::now());
        self.evidence = Some(evidence);
        self.blocker = None;
        self.completion_verification = Some(verification);
        Ok(())
    }

    pub fn mark_blocked(&mut self, blocker: String) -> Result<(), &'static str> {
        if self.objective.is_none() {
            return Err("No active goal exists to block.");
        }
        self.status = Some(GoalStatus::Blocked);
        self.finished_at = Some(Instant::now());
        self.blocker = Some(blocker);
        self.evidence = None;
        self.completion_verification = None;
        Ok(())
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn snapshot(&self) -> GoalSnapshot {
        // 一旦目标进入终端状态，就将已耗时冻结在结束时刻，
        // 这样侧边栏计时器（以及任何工具快照）在完成后就不会再继续增长。
        let elapsed_seconds = match (self.started_at, self.finished_at) {
            (Some(started), Some(finished)) => {
                Some(finished.saturating_duration_since(started).as_secs())
            }
            (Some(started), None) => Some(started.elapsed().as_secs()),
            (None, _) => None,
        };
        GoalSnapshot {
            objective: self.objective.clone(),
            status: self
                .status
                .map(GoalStatus::as_str)
                .unwrap_or("none")
                .to_string(),
            token_budget: self.token_budget,
            tokens_used: self.tokens_used,
            time_used_seconds: self.time_used_seconds,
            continuation_count: self.continuation_count,
            elapsed_seconds,
            evidence: self.evidence.clone(),
            blocker: self.blocker.clone(),
            completion_verification: self.completion_verification.clone(),
        }
    }
}

/// 当前目标的可序列化工具输出和提示词输入。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoalSnapshot {
    pub objective: Option<String>,
    pub status: String,
    pub token_budget: Option<u32>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub continuation_count: u32,
    pub elapsed_seconds: Option<u64>,
    pub evidence: Option<String>,
    pub blocker: Option<String>,
    pub completion_verification: Option<GoalCompletionVerification>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalCompletionVerification {
    pub status: String,
    pub check: String,
    pub summary: String,
}

impl GoalSnapshot {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.objective.is_some() && self.status == GoalStatus::Active.as_str()
    }

    #[must_use]
    pub fn from_thread_goal(goal: &codewhale_protocol::ThreadGoal) -> Self {
        Self {
            objective: Some(goal.objective.clone()),
            status: thread_goal_status_as_goal_status(goal.status.clone())
                .as_str()
                .to_string(),
            token_budget: goal
                .token_budget
                .and_then(|value| u32::try_from(value.max(0)).ok()),
            tokens_used: u64::try_from(goal.tokens_used.max(0)).unwrap_or(u64::MAX),
            time_used_seconds: u64::try_from(goal.time_used_seconds.max(0)).unwrap_or(u64::MAX),
            continuation_count: u32::try_from(goal.continuation_count.max(0)).unwrap_or(u32::MAX),
            elapsed_seconds: None,
            evidence: None,
            blocker: None,
            completion_verification: None,
        }
    }
}

#[must_use]
pub fn thread_goal_status_as_goal_status(
    status: codewhale_protocol::ThreadGoalStatus,
) -> GoalStatus {
    match status {
        codewhale_protocol::ThreadGoalStatus::Active => GoalStatus::Active,
        codewhale_protocol::ThreadGoalStatus::Paused => GoalStatus::Paused,
        codewhale_protocol::ThreadGoalStatus::Complete => GoalStatus::Complete,
        codewhale_protocol::ThreadGoalStatus::Blocked
        | codewhale_protocol::ThreadGoalStatus::UsageLimited
        | codewhale_protocol::ThreadGoalStatus::BudgetLimited => GoalStatus::Blocked,
    }
}

/// 当目标在一轮之后仍处于活跃状态时，渲染注入的继续提示词。
/// 没有运行层级的上限，因此显示进度（轮次计数、token 数）
/// 而非"N/max"计量器——循环会一直运行，直到完成、阻塞或暂停。
#[must_use]
pub fn render_continuation_prompt(snapshot: &GoalSnapshot, continuation_index: u32) -> String {
    let goal_json = serde_json::to_string_pretty(snapshot).unwrap_or_else(|_| "{}".to_string());
    format!(
        "{}\n\n## Active Goal State\n\n```json\n{}\n```\n\nContinuation pass #{}.\nIf the goal is complete, first run or cite a concrete verifier/check when one applies, then call `update_goal` with `status: \"complete\"`, concrete evidence, and `verification: {{\"status\":\"passed\",\"check\":\"...\",\"summary\":\"...\"}}`. For non-verifiable work (docs, research, writing), use `verification: {{\"status\":\"not_applicable\",\"check\":\"...\",\"summary\":\"...\"}}` with a clear rationale instead of fabricating a verifier receipt. If it is blocked, call `update_goal` with `status: \"blocked\"` and the blocker. Otherwise continue making progress toward the objective.",
        crate::prompts::GOAL_CONTINUATION_PROMPT.trim(),
        goal_json,
        continuation_index,
    )
}

fn lock_goal_state(
    state: &SharedGoalState,
) -> Result<std::sync::MutexGuard<'_, GoalState>, ToolError> {
    state
        .lock()
        .map_err(|_| ToolError::execution_failed("goal state lock poisoned"))
}

fn parse_token_budget(input: &Value) -> Result<Option<u32>, ToolError> {
    let Some(raw) = input.get("token_budget") else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let Some(value) = raw.as_u64() else {
        return Err(ToolError::invalid_input(
            "token_budget must be a non-negative integer",
        ));
    };
    u32::try_from(value)
        .map(Some)
        .map_err(|_| ToolError::invalid_input("token_budget is too large"))
}

fn parse_completion_verification(input: &Value) -> Result<GoalCompletionVerification, ToolError> {
    let Some(raw) = input.get("verification") else {
        return Err(ToolError::invalid_input(
            "verification is required when status is complete; run a verifier/check and pass verification: {status, check, summary}",
        ));
    };
    let verification: GoalCompletionVerification = serde_json::from_value(raw.clone())
        .map_err(|err| ToolError::invalid_input(format!("invalid verification: {err}")))?;
    let status = verification.status.trim();
    let normalized_status = match status {
        "passed" | "not_applicable" => status,
        other => {
            return Err(ToolError::invalid_input(format!(
                "verification.status must be 'passed' or 'not_applicable' before update_goal can mark a goal complete; got '{other}'"
            )));
        }
    };
    if verification.check.trim().is_empty() {
        return Err(ToolError::invalid_input("verification.check is required"));
    }
    if verification.summary.trim().is_empty() {
        return Err(ToolError::invalid_input("verification.summary is required"));
    }
    Ok(GoalCompletionVerification {
        status: normalized_status.to_string(),
        check: verification.check.trim().to_string(),
        summary: verification.summary.trim().to_string(),
    })
}

fn json_result(snapshot: &GoalSnapshot) -> Result<ToolResult, ToolError> {
    ToolResult::json(snapshot).map_err(|err| ToolError::execution_failed(err.to_string()))
}

// 三个不同的工具结构体，都实现了ToolSpec trait。它们共同构成 AI 在对话中操作"运行时目标"的接口。
// |-----------------------------------------------------------------|
// |结构体            │ 工具名             │ 作用                     |
// |-----------------------------------------------------------------|
// |CreateGoalTool    │ create_goal       │ AI 创建一个新目标         |
// |GetGoalTool       │ get_goal          │ AI 查看当前目标状态        | 
// |UpdateGoalTool    │ update_goal       │ AI 标记目标已完成或被阻塞  |
// |-----------------------------------------------------------------|
// 工具注册的位置:
// 三个 goal 工具在 每轮对话开始时 被注册到工具注册表：
// `tool_setup.rs`
//     let builder = ToolRegistryBuilder::new()
//                   .with_read_only_file_tools()
//                   .with_search_tools()
//                   // ... 其他工具 ... 
//                   .with_goal_tools(self.config.goal_state.clone());  // ← 这里！
// `registry.rs`：
//  pub fn with_goal_tools(self, goal_state: SharedGoalState) -> Self {
//      use super::goal::{CreateGoalTool, GetGoalTool, UpdateGoalTool};
//      self.with_tool(Arc::new(CreateGoalTool::new(goal_state.clone())))
//          .with_tool(Arc::new(GetGoalTool::new(goal_state.clone())))
//          .with_tool(Arc::new(UpdateGoalTool::new(goal_state)))
//  } 
// with_tool 把工具包装成 Arc<dyn ToolSpec> 存入注册表。当 LLM 在对话中决定调用
// create_goal 时，引擎从注册表找到对应的 Arc<CreateGoalTool>，在它上面调用 execute。
// 总结:
// `execute` 是 AI 操作"运行时目标"的唯一入口 —— create_goal 建目标并重置统计，
// get_goal 查看目标状态，update_goal 标记完成或阻塞。它在引擎的回合循环中被调用（
// turn_loop → execute_tool_with_lock → registry.execute_full_with_context →
// tool.execute），正是整个代理对话中 "AI 决定调用工具 → 引擎找到工具 → 执行" 
// 这一标准流程的具体体现。

/// 通俗理解： AI 在对话中决定"我理解了，用户的目标是 XXX"，然后调用 create_goal 正式记录这个目标。
/// state.create() 会重置所有统计数据（token用量、耗时、继续次数），并标记开始时间。
pub struct CreateGoalTool {
    goal_state: SharedGoalState,
}

impl CreateGoalTool {
    #[must_use]
    pub fn new(goal_state: SharedGoalState) -> Self {
        Self { goal_state }
    }
}

#[async_trait]
impl ToolSpec for CreateGoalTool {
    fn name(&self) -> &'static str {
        "create_goal"
    }

    fn description(&self) -> &'static str {
        "Create the current runtime goal. Use this only when the user explicitly asks to pursue a persistent objective."
        // 创建当前运行时目标。仅当用户明确要求追求持久目标时使用此功能。
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "objective": {
                    "type": "string",
                    // "要追求的完整目标。保留用户的完整目标，而非缩短的单轮版本。"
                    "description": "The full objective to pursue. Keep the complete user goal, not a shortened one-turn version."
                },
                "token_budget": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Optional soft token budget for the goal."
                    // 目标的可选软令牌预算。
                }
            },
            "required": ["objective"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        Vec::new()
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    /// 1. 从 JSON 输入中提取 "objective" 字段
    /// 2. 解析可选的 token 预算
    /// 3. 写入共享状态,重置计数器，记录开始时间，拍快照作为返回值
    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let objective = required_str(&input, "objective")?.trim().to_string();
        if objective.is_empty() {
            return Err(ToolError::invalid_input("objective cannot be empty"));
        }
        let token_budget = parse_token_budget(&input)?;
        let snapshot = {
            let mut state = lock_goal_state(&self.goal_state)?;
            state.create(objective, token_budget);
            state.snapshot()
        };
        json_result(&snapshot)
    }
}

/// 通俗理解： AI 在干活中途想看看"我现在还剩多少预算？离目标还有多远？"，
/// 调用 get_goal查询当前目标的状态（目标内容、状态、token使用量、耗时等）。
/// 注意它的特性：
/// - ToolCapability::ReadOnly —— 读操作
/// - ApprovalRequirement::Auto —— 需用户审批
/// - supports_parallel: true —— 以和其他工具并行执行
pub struct GetGoalTool {
    goal_state: SharedGoalState,
}

impl GetGoalTool {
    #[must_use]
    pub fn new(goal_state: SharedGoalState) -> Self {
        Self { goal_state }
    }
}

#[async_trait]
impl ToolSpec for GetGoalTool {
    fn name(&self) -> &'static str {
        "get_goal"
    }

    fn description(&self) -> &'static str {
        "Inspect the current runtime goal state, including objective, status, token budget, elapsed time, evidence, and blocker."
        // 检查当前运行时目标状态，包括目标、状态、令牌预算、已用时间、证据和阻碍因素。
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _input: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let snapshot = {
            let state = lock_goal_state(&self.goal_state)?;
            state.snapshot()   // 只读：拍一张当前目标的快照
        };
        json_result(&snapshot)
    }
}

/// 通俗理解： 当 AI认为目标已完成（有证据、有验证），或者发现无法继续（有阻塞原因），
/// 调用update_goal来改变目标状态。这会触发GoalUpdated事件，更新TUI侧边栏。
pub struct UpdateGoalTool {
    goal_state: SharedGoalState,
}

impl UpdateGoalTool {
    #[must_use]
    pub fn new(goal_state: SharedGoalState) -> Self {
        Self { goal_state }
    }
}

#[async_trait]
impl ToolSpec for UpdateGoalTool {
    fn name(&self) -> &'static str {
        "update_goal"
    }

    fn description(&self) -> &'static str {
        "Update the runtime goal completion gate. Only mark complete when the objective has verified evidence; mark blocked only after a real blocker prevents progress."
        // “更新运行时目标完成门控。仅在目标有经核实的证据时才标记为已完成；仅在真正的阻碍因素阻止进展时才标记为受阻。”
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["complete", "blocked"],
                    "description": "Use complete only when the goal is fully satisfied; blocked when meaningful progress cannot continue. Pause, resume, and budget-limit states are controlled by the user or system."
                    // “仅在目标完全达成时使用‘已完成’；在有意义的进展无法继续时使用‘受阻’。暂停、恢复和预算限制状态由用户或系统控制。”
                },
                "evidence": {
                    "type": "string",
                    "description": "Required when status is complete. Briefly cite the proof that the goal is done."
                    // “当状态为‘已完成’时必填。简要引用证明目标已达成的依据。”
                },
                "verification": {
                    "type": "object",
                    // “当状态为‘已完成’时必填。来自具体检查（例如 run_verifiers 或等效的项目特定门控）的验证者评判收据。”
                    "description": "Required when status is complete. A verifier-as-judge receipt from a concrete check, such as run_verifiers or an equivalent project-specific gate.",
                    "properties": {
                        "status": {
                            "type": "string",
                            "enum": ["passed", "not_applicable"],
                            // “当具体验证器/检查通过时使用 passed；当没有适用的自动化验证器时使用 not_applicable。”
                            "description": "Use passed when a concrete verifier/check succeeded; not_applicable when no automated verifier applies."
                        },
                        "check": {
                            "type": "string",
                            // “已通过的验证器/检查。”
                            "description": "The verifier/check that passed."
                        },
                        "summary": {
                            "type": "string",
                            // “来自验证器/检查的简要结果摘要。”
                            "description": "Brief result summary from the verifier/check."
                        }
                    },
                    "required": ["status", "check", "summary"],
                    "additionalProperties": false
                },
                "blocker": {
                    "type": "string",
                    // “当状态为‘受阻’时必填。说明阻碍进展的条件。”
                    "description": "Required when status is blocked. Explain the condition preventing progress."
                },
                "objective": {
                    "type": "string",
                    // “保留供未来主机控制的目标编辑使用；被 update_goal 忽略。”
                    "description": "Reserved for future host-controlled goal edits; ignored by update_goal."
                }
            },
            "required": ["status"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        Vec::new()
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        // status 只能是 "complete" 或 "blocked"
        // "complete" 时必须带 evidence（完成证据）和 verification（验证结果）
        // "blocked" 时必须带 blocker（阻塞原因）
        let status = required_str(&input, "status")?.trim().to_ascii_lowercase();
        let snapshot = {
            let mut state = lock_goal_state(&self.goal_state)?;
            match status.as_str() {
                "complete" => {
                    let evidence = input
                        .get("evidence")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_string();
                    if evidence.is_empty() {
                        return Err(ToolError::invalid_input(
                            "evidence is required when status is complete",
                        ));
                    }
                    let verification = parse_completion_verification(&input)?;
                    state
                        .mark_complete(evidence, verification)
                        .map_err(ToolError::invalid_input)?;
                }
                "blocked" => {
                    let blocker = input
                        .get("blocker")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_string();
                    if blocker.is_empty() {
                        return Err(ToolError::invalid_input(
                            "blocker is required when status is blocked",
                        ));
                    }
                    state
                        .mark_blocked(blocker)
                        .map_err(ToolError::invalid_input)?;
                }
                other => {
                    return Err(ToolError::invalid_input(format!(
                        "unsupported goal status '{other}'; update_goal can only mark complete or blocked"
                    )));
                }
            }
            state.snapshot()
        };
        json_result(&snapshot)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[tokio::test]
    async fn create_get_and_complete_goal() {
        let state = new_shared_goal_state();
        let ctx = ToolContext::new(".");

        let create = CreateGoalTool::new(state.clone());
        let created = create
            .execute(
                json!({
                    "objective": "ship the runtime slice",
                    "token_budget": 1200
                }),
                &ctx,
            )
            .await
            .expect("create goal");
        assert!(created.success);
        let created_json: Value = serde_json::from_str(&created.content).expect("created json");
        assert_eq!(
            created_json.get("status").and_then(Value::as_str),
            Some("active")
        );

        let get = GetGoalTool::new(state.clone());
        let current = get.execute(json!({}), &ctx).await.expect("get goal");
        assert!(current.content.contains("ship the runtime slice"));
        let current_json: Value = serde_json::from_str(&current.content).expect("current json");
        assert_eq!(
            current_json.get("token_budget").and_then(Value::as_u64),
            Some(1200)
        );

        let update = UpdateGoalTool::new(state.clone());
        let completed = update
            .execute(
                json!({
                    "status": "complete",
                    "evidence": "focused tests passed",
                    "verification": {
                        "status": "passed",
                        "check": "cargo test -p codewhale-tui goal_loop",
                        "summary": "focused tests passed"
                    }
                }),
                &ctx,
            )
            .await
            .expect("complete goal");
        let completed_json: Value =
            serde_json::from_str(&completed.content).expect("completed json");
        assert_eq!(
            completed_json.get("status").and_then(Value::as_str),
            Some("complete")
        );
        assert!(completed.content.contains("focused tests passed"));
        assert!(!state.lock().expect("goal lock").is_active());
    }

    #[tokio::test]
    async fn update_goal_requires_completion_evidence() {
        let state = new_shared_goal_state_from_host_status(
            Some("prove completion".to_string()),
            None,
            GoalStatus::Active,
        );
        let update = UpdateGoalTool::new(state);
        let err = update
            .execute(json!({"status": "complete"}), &ToolContext::new("."))
            .await
            .expect_err("missing evidence should fail");

        assert!(err.to_string().contains("evidence is required"));
    }

    #[tokio::test]
    async fn update_goal_accepts_not_applicable_verification_for_non_verifiable_goals() {
        let state = new_shared_goal_state_from_host_status(
            Some("write the release notes".to_string()),
            None,
            GoalStatus::Active,
        );
        let update = UpdateGoalTool::new(state.clone());
        let completed = update
            .execute(
                json!({
                    "status": "complete",
                    "evidence": "release notes drafted and reviewed in thread",
                    "verification": {
                        "status": "not_applicable",
                        "check": "no automated verifier applies",
                        "summary": "writing task completed with evidence in thread"
                    }
                }),
                &ToolContext::new("."),
            )
            .await
            .expect("non-verifiable goal should complete");

        let completed_json: Value =
            serde_json::from_str(&completed.content).expect("completed json");
        assert_eq!(
            completed_json.get("status").and_then(Value::as_str),
            Some("complete")
        );
        assert_eq!(
            completed_json
                .get("completion_verification")
                .and_then(|verification| verification.get("status"))
                .and_then(Value::as_str),
            Some("not_applicable")
        );
        assert!(!state.lock().expect("goal lock").is_active());
    }

    #[tokio::test]
    async fn update_goal_requires_passed_verification_to_complete() {
        let state = new_shared_goal_state_from_host_status(
            Some("prove completion".to_string()),
            None,
            GoalStatus::Active,
        );
        let update = UpdateGoalTool::new(state.clone());
        let err = update
            .execute(
                json!({
                    "status": "complete",
                    "evidence": "all checks look good"
                }),
                &ToolContext::new("."),
            )
            .await
            .expect_err("missing verifier gate should fail");

        assert!(err.to_string().contains("verification is required"));
        assert!(state.lock().expect("goal lock").is_active());
    }

    #[tokio::test]
    async fn update_goal_rejects_model_resume() {
        let state = new_shared_goal_state_from_host_status(
            Some("pause remains host controlled".to_string()),
            None,
            GoalStatus::Paused,
        );
        let update = UpdateGoalTool::new(state);
        let err = update
            .execute(json!({"status": "active"}), &ToolContext::new("."))
            .await
            .expect_err("model resume should fail");

        assert!(err.to_string().contains("complete or blocked"));
    }

    #[test]
    fn paused_host_goal_is_not_active() {
        let state = new_shared_goal_state_from_host_status(
            Some("wait for user".to_string()),
            Some(42),
            GoalStatus::Paused,
        );
        let snapshot = state.lock().expect("goal lock").snapshot();

        assert_eq!(snapshot.status, "paused");
        assert_eq!(snapshot.token_budget, Some(42));
        assert!(!snapshot.is_active());
    }

    #[test]
    fn goal_state_projects_usage_and_continuations() {
        let state = new_shared_goal_state_from_host_status(
            Some("persist accounting".to_string()),
            Some(1_000),
            GoalStatus::Active,
        );
        {
            let mut goal = state.lock().expect("goal lock");
            goal.record_usage(300, 12);
            goal.record_continuation();
        }

        let snapshot = state.lock().expect("goal lock").snapshot();
        assert_eq!(snapshot.tokens_used, 300);
        assert_eq!(snapshot.time_used_seconds, 12);
        assert_eq!(snapshot.continuation_count, 1);
    }

    #[test]
    fn completed_goal_snapshot_freezes_elapsed() {
        // 回归测试：已完成目标的快照 elapsed_seconds 不得继续增长。
        // 修复前，snapshot() 始终使用 started_at.elapsed()，
        // 因此已完成目标的已用时间在侧边栏/工具输出中持续跳动。
        let state = new_shared_goal_state_from_host_status(
            Some("freeze on completion".to_string()),
            None,
            GoalStatus::Active,
        );
        let first = {
            let mut goal = state.lock().expect("goal lock");
            goal.mark_complete(
                "evidence".to_string(),
                GoalCompletionVerification {
                    status: "passed".to_string(),
                    check: "cargo test".to_string(),
                    summary: "ok".to_string(),
                },
            )
            .expect("mark complete");
            goal.snapshot()
        };
        let elapsed_at_completion = first.elapsed_seconds.expect("elapsed present");

        // 休眠跨越整秒边界。在旧的（有 bug 的）代码下，
        // snapshot() 返回 started_at.elapsed().as_secs()，因此会
        // 至少增加一秒，导致下方的断言失败。
        // 有了冻结机制，完成的快照保持捕获时的值不变。
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        let second = state.lock().expect("goal lock").snapshot();
        assert_eq!(second.status, "complete");
        assert_eq!(
            second.elapsed_seconds,
            Some(elapsed_at_completion),
            "completed goal elapsed must be frozen, not keep ticking"
        );
    }

    #[test]
    fn protocol_thread_goal_converts_to_runtime_snapshot() {
        let snapshot = GoalSnapshot::from_thread_goal(&codewhale_protocol::ThreadGoal {
            thread_id: "thread-1".to_string(),
            goal_id: "goal-1".to_string(),
            objective: "Bridge the goal models".to_string(),
            status: codewhale_protocol::ThreadGoalStatus::Active,
            token_budget: Some(2_000),
            tokens_used: 750,
            time_used_seconds: 44,
            continuation_count: 3,
            created_at: 1,
            updated_at: 2,
        });

        assert_eq!(
            snapshot.objective.as_deref(),
            Some("Bridge the goal models")
        );
        assert_eq!(snapshot.status, "active");
        assert_eq!(snapshot.token_budget, Some(2_000));
        assert_eq!(snapshot.tokens_used, 750);
        assert_eq!(snapshot.time_used_seconds, 44);
        assert_eq!(snapshot.continuation_count, 3);
    }

    #[test]
    fn continuation_prompt_includes_bound_and_goal_state() {
        let snapshot = GoalSnapshot {
            objective: Some("finish issue 2199".to_string()),
            status: "active".to_string(),
            token_budget: None,
            tokens_used: 0,
            time_used_seconds: 0,
            continuation_count: 0,
            elapsed_seconds: Some(5),
            evidence: None,
            blocker: None,
            completion_verification: None,
        };

        let prompt = render_continuation_prompt(&snapshot, 2);
        assert!(prompt.contains("Goal Continuation"));
        assert!(prompt.contains("finish issue 2199"));
        assert!(prompt.contains("Continuation pass #2"));
    }
}
