//! 子代理（Sub-agent）生成系统。
//!
//! 提供用于生成后台子代理、查询其状态以及获取结果的工具。
//! 子代理使用经过筛选的工具集运行，并继承主会话的工作区配置。
//!
//! 面向模型的接口是单一的 `agent` 工具。旧的生命周期结构体和管理器辅助函数
//! 仍然可执行，用于持久化记录和内部恢复，而持久化运行时则被新接口复用。

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock, Semaphore};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::client::DeepSeekClient;
use crate::config::MAX_SUBAGENTS;
use crate::core::events::Event;
use crate::dependencies::{ExternalTool, Git};
use crate::llm_client::{LlmClient, LlmError};
use crate::models::{
    ContentBlock, Message, MessageRequest, MessageResponse, SystemPrompt, Tool, Usage,
};
use crate::request_tuning::RequestTuning;
use crate::tools::handle::VarHandle;
use crate::tools::plan::{PlanState, SharedPlanState};
use crate::tools::registry::{AgentToolSurfaceOptions, ToolRegistry, ToolRegistryBuilder};
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};
use crate::tools::todo::SharedTodoList;
#[cfg(test)]
use crate::tools::todo::TodoList;
use crate::tools::truncate::{SPILLOVER_HEAD_BYTES, SPILLOVER_THRESHOLD_BYTES, maybe_spillover};
use crate::tui::app::AppMode;
use crate::tui::app::ReasoningEffort;
use crate::utils::spawn_supervised;
use crate::worker_profile::{ModelRoute, ShellPolicy, ToolScope, WorkerRuntimeProfile};

pub mod mailbox;
#[allow(unused_imports)]
pub use mailbox::{Mailbox, MailboxEnvelope, MailboxMessage, MailboxReceiver};

// === 常量 ===

/// 缓存感知常驻文件子代理的全局所有权表（#529）。
/// 映射文件路径 → 代理 ID。代理在运行时持有文件的租约；
/// 当代理进入终止状态时，租约被释放。
static RESIDENT_LEASES: std::sync::OnceLock<
    parking_lot::Mutex<std::collections::HashMap<String, String>>,
> = std::sync::OnceLock::new();

/// 释放 `agent_id` 持有的所有常驻文件租约。当代理
/// 进入终止状态（已完成、失败、已取消）时调用。
fn release_resident_leases_for(agent_id: &str) {
    if let Some(lock) = RESIDENT_LEASES.get() {
        let mut guard = lock.lock();
        guard.retain(|_, owner| owner != agent_id);
    }
}

/// 子代理循环的默认最大步数。设置为 `u32::MAX` 以移除
/// 任意的固定上限（#2034）。子代理会一直运行，直到生成最终的文本
/// 响应（无工具调用）、被父级取消或达到配置的显式预算。
/// 想要硬性限制的调用方可以覆盖 `SubAgentManager` 上的 `max_steps`。
const DEFAULT_MAX_STEPS: u32 = u32::MAX;
/// 单次子代理工具执行的默认挂钟预算。实际值
/// 通过 `SubAgentRuntime::tool_timeout` 传递，以便一个耗时但合法的
/// 工具（大型构建、慢速 shell 命令、深度搜索）不会在执行中途被杀死。
/// 保持非零值，以便 `timeout(Duration::ZERO, ...)` 永远不会立即触发。
/// 每步 API 超时、流式看门狗和心跳下限仍然是独立的停滞检测器。
const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(300);
const MIN_SUBAGENT_SPAWN_TOKEN_RESERVE: u64 = 1;
const MIN_EVENT_CHANNEL_HEADROOM_FOR_ROUTINE_PROGRESS: usize = 32;

/// 格式化子代理进度消息的步数计数器。
///
/// 当 `max_steps == u32::MAX`（默认值）时，分母是一个表示"无限制"的标记——
/// 仅渲染 `step N`，而不是 `step N/4294967295`。
fn format_step_counter(steps: u32, max_steps: u32) -> String {
    if max_steps == u32::MAX {
        format!("step {steps}")
    } else {
        format!("step {steps}/{max_steps}")
    }
}
// 非流式子代理需要足够的响应预算来承载大型工具调用
// 参数，尤其是 write_file 内容。API 按生成的 token 计费，而非请求的上限。
const SUBAGENT_RESPONSE_MAX_TOKENS: u32 = 16_384;
const MAX_CONSECUTIVE_TRUNCATED_SUBAGENT_RESPONSES: u32 = 5;
const SUBAGENT_TRANSIENT_PROVIDER_MAX_RETRIES: u32 = 2;
const SUBAGENT_TRANSIENT_PROVIDER_INITIAL_BACKOFF: Duration = Duration::from_millis(250);
/// 每步 LLM API 调用超时。每个 `create_message` 请求必须在此窗口内完成，
/// 否则该步将被视为超时。防止单个卡住的 API 调用无限期阻塞子代理。
/// 每步 DeepSeek API 超时的旧有回退值。当前超时值通过
/// `SubAgentRuntime::step_api_timeout` 传递，以便用户可以通过
/// `[subagents] api_timeout_secs` 在配置中覆盖它。
/// 该常量仅存在于需要硬编码默认值的测试/桩运行时；
/// 生产运行时显式设置该字段（#1806, #1808）。
const DEFAULT_STEP_API_TIMEOUT: Duration =
    Duration::from_secs(crate::config::DEFAULT_SUBAGENT_API_TIMEOUT_SECS);
const COMPLETED_AGENT_RETENTION: Duration = Duration::from_secs(60 * 60);
const MAX_AGENT_WORKER_RECORDS: usize = 256;
const MAX_AGENT_WORKER_EVENTS_PER_RECORD: usize = 128;
/// [`SubAgentCheckpoint`] 中保留的消息尾部的字节预算(#3882)。
/// 检查点在每个工作者的每一步触发，并被克隆到快照、投影和 `subagents.v1.json` 中；
/// 无界的 `messages` 克隆会将一个大型工具输出在 Fleet 扇出下变成多个常驻副本。
/// 检查点在此预算内保留最近的消息（始终至少保留最后一条，因此可持续性得以保留），
/// 并记录跳过了多少条旧消息。完整的工具输出仍然可以从磁盘上的溢出文件中恢复。
const SUBAGENT_CHECKPOINT_MESSAGE_BUDGET_BYTES: usize = 256 * 1024;
/// 嵌入在 `subagent_full_transcript` 句柄中的消息尾部的字节预算(#3882)。
/// 每个代理在内存中保留一个句柄；有效载荷保留有界的尾部加上真实的 `message_count`，
/// 这样检查仍然有用，而无需在 RAM 中固定整个无界转录。
const SUBAGENT_TRANSCRIPT_MESSAGE_BUDGET_BYTES: usize = 1024 * 1024;
const SUBAGENT_STATE_SCHEMA_VERSION: u32 = 1;
const SUBAGENT_STATE_FILE: &str = "subagents.v1.json";
const SUBAGENT_WORKTREE_ROOT_DIR: &str = ".codewhale-worktrees";
const SUBAGENT_RESTART_REASON: &str = "Interrupted by process restart";
const SUBAGENT_QUEUED_LAUNCH_REASON: &str = "queued: waiting for a sub-agent launch slot";
const SUBAGENT_MODEL_WAIT_REASON: &str = "waiting for model response";
/// #freeze: 热路径（每步检查点）状态持久化的最小间隔。
/// `update_checkpoint` 在每个代理的每一步触发；在高扇出时，
/// 在管理器写锁下无条件地全舰队重写会导致 UI 卡死。
/// 热路径写入在此间隔内最多合并为一次；终止/结构性变更仍然立即持久化，
/// 任何终止写入都会将完整的在内存舰队（包括其他代理的待处理检查点）刷新到磁盘。
const SUBAGENT_PERSIST_DEBOUNCE: Duration = Duration::from_millis(1500);

/// #3803: 由侧边栏刷新（`Op::ListSubAgents`）触发的写锁定 `cleanup` 运行之间的最小间隔。
/// Cleanup 自动取消过时的代理（心跳超时，默认 300 秒）并丢弃旧的已完成记录，
/// 因此 2 秒的下限使其保持响应性，同时防止在高扇出爆发期间每次刷新都发生写锁争用。
pub const SUBAGENT_LIST_CLEANUP_MIN_INTERVAL: Duration = Duration::from_secs(2);

/// #freeze: 子代理持久化热路径的轻量级性能计数器，
/// 由 `CODEWHALE_SUBAGENT_PERF_TRACE=1` 控制开启。原子递增总是廉价的；
/// 只有结构化的 `subagent_perf` 日志行才被门控。
static SUBAGENT_PERSIST_WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SUBAGENT_PERSIST_SKIPPED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn subagent_perf_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("CODEWHALE_SUBAGENT_PERF_TRACE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

const VALID_SUBAGENT_TYPES: &str = "general (aliases: general-purpose, general_purpose, worker, default), \
     explore (aliases: exploration, explorer), plan (aliases: planning, planner, awaiter), \
     review (aliases: code-review, code_review, reviewer), implementer (aliases: implement, implementation, builder), \
     verifier (aliases: verify, verification, validator, tester), custom";
/// `normalize_role_alias` 接受的角色别名。与下面的匹配分支保持同步，
/// 以便 `SubAgentType::from_str` 接受的每个输入也能解析为规范角色（避免 #2649 中的双重验证拒绝）。
const VALID_ROLE_ALIASES: &str = "default; worker (aliases: general, general-purpose, general_purpose); \
     explorer (aliases: explore, exploration); awaiter (aliases: plan, planning, planner); \
     reviewer (aliases: review, code-review, code_review); implementer (aliases: implement, implementation, builder); \
     verifier (aliases: verify, verification, validator, tester); custom";
const SUBAGENT_TYPE_DESCRIPTION: &str = "Sub-agent type. Accepted vocabulary: general (aliases: general-purpose, general_purpose, worker, default), \
     explore (aliases: exploration, explorer), plan (aliases: planning, planner, awaiter), \
     review (aliases: code-review, code_review, reviewer), implementer (aliases: implement, implementation, builder), \
     verifier (aliases: verify, verification, validator, tester), custom.";
/// UI 中用作子代理友好名称的鲸类物种。完整的鲸目下目——
/// 须鲸（Mysticeti）、齿鲸（Odontoceti），加上选定的海豚科物种，
/// 这些物种不会与现有的代理类型标签混淆。鼠海豚科（Phocoenidae）
/// 被排除，因为其名称不太适合作为友好标签。
///
/// 英文和简体中文名称交替排列，以便任何新生成的代理
/// 有大致相等的机会获得其中一种——目标是友好的多样性，
/// 而非严格的语言环境匹配。
///
/// 分类来源：海洋哺乳动物学会（2025）。
pub const WHALE_NICKNAMES: &[&str] = &[
    "Blue",
    "蓝鲸",
    "Humpback",
    "座头鲸",
    "Sperm",
    "抹香鲸",
    "Fin",
    "长须鲸",
    "Sei",
    "塞鲸",
    "Bryde's",
    "布氏鲸",
    "Minke",
    "小须鲸",
    "Antarctic Minke",
    "南极小须鲸",
    "Pygmy Right",
    "小露脊鲸",
    "Omura's",
    "大村鲸",
    "Eden's",
    "艾氏鲸",
    "Rice's",
    "赖斯鲸",
    "Gray",
    "灰鲸",
    "Bowhead",
    "弓头鲸",
    "North Atlantic Right",
    "北大西洋露脊鲸",
    "North Pacific Right",
    "北太平洋露脊鲸",
    "Southern Right",
    "南露脊鲸",
    "Beluga",
    "白鲸",
    "Narwhal",
    "独角鲸",
    "Orca",
    "虎鲸",
    "Pilot",
    "领航鲸",
    "False Killer",
    "伪虎鲸",
    "Pygmy Killer",
    "小虎鲸",
    "Melon-headed",
    "瓜头鲸",
    "Beaked",
    "喙鲸",
    "Cuvier's Beaked",
    "柯氏喙鲸",
    "Baird's Beaked",
    "贝氏喙鲸",
    "Blainville's Beaked",
    "柏氏喙鲸",
    "Ginkgo-toothed Beaked",
    "银杏齿喙鲸",
    "Strap-toothed",
    "带齿喙鲸",
    "Stejneger's Beaked",
    "斯氏喙鲸",
    "Dwarf Sperm",
    "小抹香鲸",
    "Pygmy Sperm",
    "侏儒抹香鲸",
    "Rough-toothed",
    "糙齿海豚",
    "Atlantic Spotted",
    "大西洋斑海豚",
    "Pantropical Spotted",
    "热带斑海豚",
    "Spinner",
    "长吻飞旋海豚",
    "Clymene",
    "短吻飞旋海豚",
    "Striped",
    "条纹海豚",
    "Common Bottlenose",
    "宽吻海豚",
    "Indo-Pacific Bottlenose",
    "印太瓶鼻海豚",
    "Risso's",
    "灰海豚",
    "Commerson's",
    "花斑海豚",
    "Chilean",
    "智利海豚",
    "Heaviside's",
    "海氏矮海豚",
    "Hector's",
    "赫氏矮海豚",
    "Amazon River",
    "亚马逊河豚",
    "Ganges River",
    "恒河豚",
    "Indus River",
    "印度河豚",
    "La Plata",
    "拉普拉塔河豚",
    "Franciscana",
    "拉河豚",
];

/// 使用 ID 字符串的哈希为给定的代理 ID 返回确定性的鲸鱼名称。
/// 相同的 ID 始终获得相同的名称——对于持久化代理，跨会话重启保持稳定。
#[must_use]
pub fn whale_name_for_id(id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    let idx = (hasher.finish() as usize) % WHALE_NICKNAMES.len();
    WHALE_NICKNAMES[idx].to_string()
}

/// 为代理 ID 分配唯一的鲸鱼名称，避免与 `active_names` 中已有的名称冲突。
/// 如果确定性名称已被占用，则附加数字后缀（例如 "Orca (2)"）。
#[must_use]
pub fn assign_unique_whale_name(
    id: &str,
    active_names: &std::collections::HashSet<String>,
) -> String {
    let base = whale_name_for_id(id);
    if !active_names.contains(&base) {
        return base;
    }
    // 使用相同哈希的确定性后缀以保持稳定
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    let suffix_seed = hasher.finish();
    for i in 2.. {
        let candidate = format!("{base} ({i})");
        if !active_names.contains(&candidate) {
            return candidate;
        }
        // 使用种子变化探测值
        let probe = (suffix_seed.wrapping_add(i as u64)) % 100;
        let candidate2 = format!("{base} ({probe})");
        if !active_names.contains(&candidate2) {
            return candidate2;
        }
    }
    // 回退（理论上不应到达此处）
    format!("{base} ({})", id.get(..4).unwrap_or("?"))
}

// === 类型 ===

/// 子代理编排的分配元数据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubAgentAssignment {
    pub objective: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

impl SubAgentAssignment {
    fn new(objective: String, role: Option<String>) -> Self {
        Self { objective, role }
    }
}

/// 具有专业化行为和工具访问权限的子代理执行类型。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentType {
    /// 通用目的——对多步骤任务具有完整工具访问权限。
    #[default]
    General,
    /// 快速探索——用于代码库搜索的只读工具。
    Explore,
    /// 规划——仅用于架构规划的分析工具。
    Plan,
    /// 代码审查——读取 + 分析工具。
    Review,
    /// 实现——专注于编写/修补代码以满足特定的变更。
    /// 与 `General` 的不同之处在于，提示词姿态强调以最小的附带编辑干净地落地变更（#404）。
    Implementer,
    /// 验证——专注于运行测试套件或其他验证门控，并报告通过/失败及证据。
    /// 与 `Review` 的不同之处在于，Review 读取代码并评分；
    /// Verifier *运行*测试并报告结果（#404）。
    Verifier,
    /// 在生成时定义的自定义工具访问权限。
    Custom,
}

impl SubAgentType {
    /// 从用户输入解析子代理类型。
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "general" | "general-purpose" | "general_purpose" | "worker" | "default" => {
                Some(Self::General)
            }
            "explore" | "exploration" | "explorer" => Some(Self::Explore),
            "plan" | "planning" | "planner" | "awaiter" => Some(Self::Plan),
            "review" | "code-review" | "code_review" | "reviewer" => Some(Self::Review),
            "implementer" | "implement" | "implementation" | "builder" => Some(Self::Implementer),
            "verifier" | "verify" | "verification" | "validator" | "tester" => Some(Self::Verifier),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Explore => "explore",
            Self::Plan => "plan",
            Self::Review => "review",
            Self::Implementer => "implementer",
            Self::Verifier => "verifier",
            Self::Custom => "custom",
        }
    }

    /// 获取此代理类型的系统提示词。
    #[must_use]
    pub fn system_prompt(&self) -> String {
        let role_intro = match self {
            Self::General => GENERAL_AGENT_INTRO,
            Self::Explore => EXPLORE_AGENT_INTRO,
            Self::Plan => PLAN_AGENT_INTRO,
            Self::Review => REVIEW_AGENT_INTRO,
            Self::Implementer => IMPLEMENTER_AGENT_INTRO,
            Self::Verifier => VERIFIER_AGENT_INTRO,
            Self::Custom => CUSTOM_AGENT_INTRO,
        };
        format!("{role_intro}{SUBAGENT_OUTPUT_FORMAT}")
    }
}

/// 子代理执行的状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SubAgentStatus {
    Running,
    Completed,
    Interrupted(String),
    Failed(String),
    Cancelled,
    /// 工作者因超过自身的每工作者 token 预算而停止。
    /// 与作用域级别的准入门控不同（#3319）：此限制针对单个失控工作者，
    /// 而作用域门控限制整个根运行及其后代的扇出总量。
    BudgetExhausted,
}

/// 非运行中的子代理需要父级操作的结构化原因。
///
/// 这是有意与 `SubAgentStatus` 分开的：旧有接口继续看到 `Interrupted`，
/// 而父级可见的投影获得具体的问题/操作，而非一个停滞的子任务。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubAgentNeedsInput {
    pub question: String,
}

/// 用于工具结果的子代理状态快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentResult {
    pub name: String,
    pub agent_id: String,
    pub context_mode: String,
    pub fork_context: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    pub agent_type: SubAgentType,
    pub assignment: SubAgentAssignment,
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    pub status: SubAgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_status: Option<AgentWorkerStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    #[serde(default)]
    pub spawn_depth: u32,
    pub result: Option<String>,
    pub steps_taken: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<SubAgentCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_input: Option<SubAgentNeedsInput>,
    pub duration_ms: u64,
    /// `true` 表示此代理是从先前会话的持久化状态文件加载的，
    /// 而非在当前会话中生成的（#405）。
    /// 允许列表默认过滤掉历史噪音，同时通过 `include_archived=true` 保持记录可访问。
    #[serde(default, skip_serializing_if = "is_false")]
    pub from_prior_session: bool,
}

/// 子代理执行的无头工作者生命周期状态。
///
/// 这是独立于 TUI 的状态机，未来的 CLI/API/工作流接口应使用此状态机。
/// 旧有的 `SubAgentStatus` 仍然是子代理运行返回的兼容性投影。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentWorkerStatus {
    Queued,
    Starting,
    Running,
    WaitingForUser,
    ModelWait,
    RunningTool,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl AgentWorkerStatus {
    /// 终止状态的工作者可能会按时间从运行分类账中逐出（#4217）。
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

/// 为无头工作者请求的工具能力配置文件。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentWorkerToolProfile {
    /// 继承父运行时的注册表以保持兼容性。
    Inherited,
    /// 仅使用列出的工具。
    Explicit(Vec<String>),
}

/// 从 `agent` 派生的声明式无头工作者请求。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentWorkerSpec {
    pub worker_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub agent_type: SubAgentType,
    pub model: String,
    pub workspace: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    pub context_mode: String,
    pub fork_context: bool,
    pub tool_profile: AgentWorkerToolProfile,
    #[serde(default)]
    pub runtime_profile: WorkerRuntimeProfile,
    pub max_steps: u32,
    pub spawn_depth: u32,
    pub max_spawn_depth: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunFollowUpDelivery {
    pub delivered: bool,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub interrupt: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub continued_from_checkpoint: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunFollowUpTarget {
    #[serde(default = "default_agent_inspect_tool")]
    pub tool: String,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(default)]
    pub accepted_statuses: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_delivery: Option<AgentRunFollowUpDelivery>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunTakeoverTarget {
    #[serde(default = "default_subagent_takeover_kind")]
    pub kind: String,
    #[serde(default)]
    pub supported: bool,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunArtifactRef {
    pub kind: String,
    pub name: String,
    pub target: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunUsage {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_spent_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_remaining_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_scope: Option<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunVerificationSummary {
    pub status: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunRecommendedAction {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub reason: String,
}

/// 结构化的无头工作者事件。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentWorkerEvent {
    pub seq: u64,
    pub worker_id: String,
    pub status: AgentWorkerStatus,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

/// `SubAgentManager` 保留的规范无头工作者记录。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentWorkerRecord {
    pub spec: AgentWorkerSpec,
    #[serde(default = "default_subagent_actor_kind")]
    pub actor_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    #[serde(default = "default_agent_run_follow_up")]
    pub follow_up: AgentRunFollowUpTarget,
    #[serde(default = "default_agent_run_takeover")]
    pub takeover: AgentRunTakeoverTarget,
    #[serde(default)]
    pub artifacts: Vec<AgentRunArtifactRef>,
    #[serde(default = "default_agent_run_usage")]
    pub usage: AgentRunUsage,
    #[serde(default = "default_agent_run_verification")]
    pub verification: AgentRunVerificationSummary,
    #[serde(default = "default_agent_run_recommended_action")]
    pub recommended_action: AgentRunRecommendedAction,
    pub status: AgentWorkerStatus,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub steps_taken: u32,
    #[serde(default)]
    pub events: VecDeque<AgentWorkerEvent>,
}

impl AgentWorkerRecord {
    fn new(spec: AgentWorkerSpec, now_ms: u64) -> Self {
        let run_id = agent_worker_run_id(&spec);
        let artifacts = default_subagent_artifacts(&run_id);
        let follow_up = follow_up_target_for_spec(&spec);
        let takeover = takeover_target_for_spec(&spec);
        let recommended_action =
            recommended_action_for_worker_status(AgentWorkerStatus::Starting, &spec);
        Self {
            parent_run_id: spec.parent_run_id.clone(),
            spec,
            actor_kind: default_subagent_actor_kind(),
            follow_up,
            takeover,
            artifacts,
            usage: default_agent_run_usage(),
            verification: default_agent_run_verification(),
            recommended_action,
            status: AgentWorkerStatus::Starting,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            started_at_ms: None,
            completed_at_ms: None,
            latest_message: None,
            result_summary: None,
            error: None,
            steps_taken: 0,
            events: VecDeque::new(),
        }
    }
}

fn default_subagent_actor_kind() -> String {
    "subagent".to_string()
}

fn default_agent_inspect_tool() -> String {
    "handle_read".to_string()
}

fn default_subagent_takeover_kind() -> String {
    "local_subagent_session".to_string()
}

fn default_agent_run_follow_up() -> AgentRunFollowUpTarget {
    AgentRunFollowUpTarget {
        tool: default_agent_inspect_tool(),
        agent_id: String::new(),
        session_name: None,
        accepted_statuses: vec!["running".to_string(), "interrupted_continuable".to_string()],
        latest_delivery: None,
    }
}

fn default_agent_run_takeover() -> AgentRunTakeoverTarget {
    AgentRunTakeoverTarget {
        kind: default_subagent_takeover_kind(),
        supported: false,
        agent_id: String::new(),
        session_name: None,
        instructions: "No takeover target is available for this older record.".to_string(),
        unsupported_reason: Some("legacy_record_missing_agent_id".to_string()),
    }
}

fn default_agent_run_usage() -> AgentRunUsage {
    AgentRunUsage {
        status: "unknown".to_string(),
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
        token_budget: None,
        budget_spent_tokens: None,
        budget_remaining_tokens: None,
        budget_scope: None,
        note: "Token usage is not yet reported by the sub-agent worker ledger.".to_string(),
    }
}

fn positive_token_budget(budget: Option<u64>) -> Option<u64> {
    budget.filter(|value| *value > 0)
}

fn usage_total_tokens(usage: &Usage) -> u64 {
    u64::from(usage.input_tokens).saturating_add(u64::from(usage.output_tokens))
}

fn refresh_usage_note(usage: &mut AgentRunUsage) {
    let worker_total = usage.total_tokens.unwrap_or(0);
    if let Some(limit) = usage.token_budget {
        let spent = usage.budget_spent_tokens.unwrap_or(worker_total);
        let remaining = usage
            .budget_remaining_tokens
            .unwrap_or_else(|| limit.saturating_sub(spent));
        usage.status = if remaining == 0 {
            "budget_exhausted".to_string()
        } else if worker_total > 0 {
            "reported".to_string()
        } else {
            "tracking".to_string()
        };
        usage.note = if worker_total > 0 {
            format!(
                "Token budget: {spent}/{limit} spent, {remaining} remaining. This worker reported {worker_total} tokens."
            )
        } else {
            format!("Token budget: {spent}/{limit} spent, {remaining} remaining.")
        };
    } else if worker_total > 0 {
        usage.status = "reported".to_string();
        usage.note = format!("Provider reported {worker_total} tokens for this worker.");
    } else if usage.status.is_empty() {
        *usage = default_agent_run_usage();
    }
}

fn default_agent_run_verification() -> AgentRunVerificationSummary {
    AgentRunVerificationSummary {
        status: "self_report_only".to_string(),
        summary:
            "No verified command or test receipt is attached; treat the result summary as a child self-report."
                .to_string(),
    }
}

fn default_agent_run_recommended_action() -> AgentRunRecommendedAction {
    AgentRunRecommendedAction {
        action: "inspect_transcript".to_string(),
        tool: Some(default_agent_inspect_tool()),
        reason: "Inspect the returned transcript handle if the child result needs audit detail."
            .to_string(),
    }
}

fn recommended_action_for_worker_status(
    status: AgentWorkerStatus,
    spec: &AgentWorkerSpec,
) -> AgentRunRecommendedAction {
    let agent_ref = spec
        .session_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(&spec.worker_id);
    match status {
        AgentWorkerStatus::Queued => AgentRunRecommendedAction {
            action: "continue_parent_work".to_string(),
            tool: None,
            reason: format!(
                "Worker {agent_ref} is queued in the background; continue coordinating and consume its completion event when it arrives."
            ),
        },
        AgentWorkerStatus::Starting
        | AgentWorkerStatus::Running
        | AgentWorkerStatus::ModelWait
        | AgentWorkerStatus::RunningTool => AgentRunRecommendedAction {
            action: "continue_parent_work".to_string(),
            tool: None,
            reason: format!(
                "Worker {agent_ref} is active in the background; continue parent work until its completion event arrives."
            ),
        },
        AgentWorkerStatus::WaitingForUser => AgentRunRecommendedAction {
            action: "inspect_or_replace".to_string(),
            tool: Some(default_agent_inspect_tool()),
            reason: format!(
                "Worker {agent_ref} needs parent action; inspect the transcript handle and open a replacement with agent if the task still matters."
            ),
        },
        AgentWorkerStatus::Completed => AgentRunRecommendedAction {
            action: "verify_self_report".to_string(),
            tool: Some("handle_read".to_string()),
            reason: format!(
                "Worker {agent_ref} completed; verify its self-report before treating side effects as fact."
            ),
        },
        AgentWorkerStatus::Failed => AgentRunRecommendedAction {
            action: "inspect_failure".to_string(),
            tool: Some(default_agent_inspect_tool()),
            reason: format!(
                "Worker {agent_ref} failed; inspect the transcript handle and decide whether to open a replacement."
            ),
        },
        AgentWorkerStatus::Cancelled => AgentRunRecommendedAction {
            action: "open_replacement_if_needed".to_string(),
            tool: Some("agent".to_string()),
            reason: format!(
                "Worker {agent_ref} was cancelled; open a replacement with agent only if the assignment still matters."
            ),
        },
        AgentWorkerStatus::Interrupted => AgentRunRecommendedAction {
            action: "inspect_or_replace".to_string(),
            tool: Some(default_agent_inspect_tool()),
            reason: format!(
                "Worker {agent_ref} was interrupted; inspect the transcript handle before deciding whether to re-dispatch."
            ),
        },
    }
}

fn agent_worker_run_id(spec: &AgentWorkerSpec) -> String {
    if spec.run_id.is_empty() {
        spec.worker_id.clone()
    } else {
        spec.run_id.clone()
    }
}

fn follow_up_target_for_spec(spec: &AgentWorkerSpec) -> AgentRunFollowUpTarget {
    AgentRunFollowUpTarget {
        tool: default_agent_inspect_tool(),
        agent_id: spec.worker_id.clone(),
        session_name: spec.session_name.clone(),
        accepted_statuses: vec!["running".to_string(), "interrupted_continuable".to_string()],
        latest_delivery: None,
    }
}

fn takeover_target_for_spec(spec: &AgentWorkerSpec) -> AgentRunTakeoverTarget {
    let agent_ref = spec
        .session_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(&spec.worker_id);
    AgentRunTakeoverTarget {
        kind: default_subagent_takeover_kind(),
        supported: true,
        agent_id: spec.worker_id.clone(),
        session_name: spec.session_name.clone(),
        instructions: format!(
            "Inspect agent '{agent_ref}' through the returned transcript_handle with handle_read; open a replacement with agent if the lane no longer fits."
        ),
        unsupported_reason: None,
    }
}

fn default_subagent_artifacts(run_id: &str) -> Vec<AgentRunArtifactRef> {
    vec![
        AgentRunArtifactRef {
            kind: "worker_events".to_string(),
            name: "worker_record.events".to_string(),
            target: run_id.to_string(),
            description: "Bounded structured lifecycle events retained on the worker record."
                .to_string(),
        },
        AgentRunArtifactRef {
            kind: "transcript".to_string(),
            name: "transcript_handle".to_string(),
            target: format!("agent:{run_id}"),
            description:
                "Use the projection transcript_handle with handle_read for the child transcript."
                    .to_string(),
        },
        AgentRunArtifactRef {
            kind: "receipt".to_string(),
            name: "result_summary".to_string(),
            target: run_id.to_string(),
            description: "Child final summary when present; verify before treating as fact."
                .to_string(),
        },
    ]
}

fn normalize_worker_spec(mut spec: AgentWorkerSpec) -> AgentWorkerSpec {
    if spec.run_id.is_empty() {
        spec.run_id = spec.worker_id.clone();
    }
    spec
}

fn worker_tool_scope(tool_profile: &AgentWorkerToolProfile) -> ToolScope {
    match tool_profile {
        AgentWorkerToolProfile::Inherited => ToolScope::Inherit,
        AgentWorkerToolProfile::Explicit(tools) => ToolScope::Explicit(tools.clone()),
    }
}

fn worker_profile_from_spec(spec: &AgentWorkerSpec) -> WorkerRuntimeProfile {
    let mut profile = WorkerRuntimeProfile::for_role(spec.agent_type.clone());
    profile.tools = worker_tool_scope(&spec.tool_profile);
    profile.model = ModelRoute::Fixed(spec.model.clone());
    profile.max_spawn_depth = spec.max_spawn_depth.saturating_sub(spec.spawn_depth);
    profile.background = true;
    profile
}

fn worker_profile_for_spawn(
    runtime: &SubAgentRuntime,
    agent_type: &SubAgentType,
    tool_profile: &AgentWorkerToolProfile,
    effective_model: &str,
    model_route: Option<ModelRoute>,
) -> WorkerRuntimeProfile {
    let mut requested = WorkerRuntimeProfile::for_role(agent_type.clone());
    requested.tools = worker_tool_scope(tool_profile);
    requested.model = model_route.unwrap_or_else(|| ModelRoute::Fixed(effective_model.to_string()));
    requested.provider = Some(runtime.client.api_provider().as_str().to_string());
    requested.max_spawn_depth = runtime.max_spawn_depth.saturating_sub(runtime.spawn_depth);
    requested.background = true;
    runtime.worker_profile.derive_child(&requested)
}

fn normalize_worker_record(mut record: AgentWorkerRecord) -> AgentWorkerRecord {
    record.spec = normalize_worker_spec(record.spec);
    if record.spec.runtime_profile == WorkerRuntimeProfile::default() {
        record.spec.runtime_profile = worker_profile_from_spec(&record.spec);
    }
    let run_id = agent_worker_run_id(&record.spec);
    if record.actor_kind.is_empty() {
        record.actor_kind = default_subagent_actor_kind();
    }
    if record.parent_run_id.is_none() {
        record.parent_run_id = record.spec.parent_run_id.clone();
    }
    if record.follow_up.agent_id.is_empty() {
        record.follow_up = follow_up_target_for_spec(&record.spec);
    } else if record.follow_up.tool != default_agent_inspect_tool() {
        record.follow_up.tool = default_agent_inspect_tool();
    }
    if record.takeover.agent_id.is_empty()
        || !record
            .takeover
            .instructions
            .contains(&default_agent_inspect_tool())
    {
        record.takeover = takeover_target_for_spec(&record.spec);
    }
    record.recommended_action = recommended_action_for_worker_status(record.status, &record.spec);
    if record.artifacts.is_empty() {
        record.artifacts = default_subagent_artifacts(&run_id);
    }
    if record.usage.status.is_empty() {
        record.usage = default_agent_run_usage();
    } else {
        refresh_usage_note(&mut record.usage);
    }
    if record.verification.status.is_empty() {
        record.verification = default_agent_run_verification();
    }
    record
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn current_git_branch(workspace: &Path) -> Option<String> {
    let branch = run_git(workspace, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = branch.trim();
    if branch.is_empty() {
        return None;
    }
    if branch != "HEAD" {
        return Some(branch.to_string());
    }

    let short_hash = run_git(workspace, &["rev-parse", "--short", "HEAD"])?;
    let short_hash = short_hash.trim();
    (!short_hash.is_empty()).then(|| format!("detached:{short_hash}"))
}

fn run_git(workspace: &Path, args: &[&str]) -> Option<String> {
    let output = Git::output(args, workspace).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SubAgentSpawnOptions {
    pub name: Option<String>,
    pub model: Option<String>,
    pub model_route: Option<ModelRoute>,
    pub nickname: Option<String>,
    pub fork_context: bool,
    pub token_budget: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkflowTaskSpawnResult {
    pub result: SubAgentResult,
    pub metadata: WorkflowTaskSpawnMetadata,
}

/// 通过 `spawn_workflow_task`（#4119）启动的子级被标记的工作流身份。
/// 让面板/历史渲染无需解析子级提示词。
#[derive(Debug, Clone)]
pub(crate) struct WorkflowTaskSpawnIdentity {
    pub workflow_run_id: String,
    pub workflow_phase_id: Option<String>,
    pub workflow_task_label: Option<String>,
    pub workflow_child_index: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkflowTaskSpawnMetadata {
    pub resolved_provider: String,
    pub resolved_model: String,
    pub route_source: String,
    /// 为此生成解析的舰队角色（如有，#4177）。
    pub resolved_role: Option<String>,
    /// 为此生成解析的 AgentProfile id（如有，#4177）。
    pub resolved_profile: Option<String>,
    pub parent_task_id: Option<String>,
    pub depth: u32,
    /// 启动此子级的工作流运行（直接 `agent` 生成为 `None`）。
    pub workflow_run_id: Option<String>,
    /// 子级被准入时的活跃阶段标题/id（工作流外部为 `None`）。
    pub workflow_phase_id: Option<String>,
    /// Workflow `task({ label })` 选项中的人类可读标签。
    pub workflow_task_label: Option<String>,
    /// 此工作流运行中各子级的准入顺序（从 0 开始）。
    pub workflow_child_index: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubAgentModelStrength {
    Same,
    Faster,
}

impl SubAgentModelStrength {
    fn parse(value: &str) -> Result<Self, ToolError> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "same" | "inherit" | "parent" | "current" => Ok(Self::Same),
            "faster" | "fast" | "smaller" | "small" | "lower" | "cheap" | "flash" => {
                Ok(Self::Faster)
            }
            _ => Err(ToolError::invalid_input(
                "model_strength must be one of: same, faster".to_string(),
            )),
        }
    }

    fn model_route(self) -> ModelRoute {
        match self {
            Self::Same => ModelRoute::Inherit,
            Self::Faster => ModelRoute::Faster,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubAgentThinking {
    Inherit,
    Auto,
    Effort(ReasoningEffort),
}

impl SubAgentThinking {
    fn parse(value: &str) -> Result<Self, ToolError> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "inherit" | "parent" | "same" | "current" => Ok(Self::Inherit),
            "auto" | "automatic" => Ok(Self::Auto),
            "off" | "disabled" | "none" | "false" => Ok(Self::Effort(ReasoningEffort::Off)),
            "low" | "minimal" => Ok(Self::Effort(ReasoningEffort::Low)),
            "medium" | "mid" => Ok(Self::Effort(ReasoningEffort::Medium)),
            "high" => Ok(Self::Effort(ReasoningEffort::High)),
            "max" | "maximum" | "xhigh" | "ultracode" => Ok(Self::Effort(ReasoningEffort::Max)),
            _ => Err(ToolError::invalid_input(
                "thinking must be one of: inherit, auto, off, low, medium, high, max".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone)]
struct SubAgentInput {
    text: String,
    interrupt: bool,
}

#[derive(Debug, Clone)]
struct SpawnRequest {
    session_name: Option<String>,
    prompt: String,
    agent_type: SubAgentType,
    /// 当调用方显式提供了 `type`/`agent_type` 或 `role` 时为 `true`
    /// （相对于 `General` 默认值）。舰队 `profile` 仅在调用方未提供时设置代理类型，
    /// 且仅在显式值时才拒绝冲突。
    agent_type_explicit: bool,
    /// 可选的舰队名册成员 id（已修剪、小写）。在生成时根据运行时名册解析——解析时无运行时访问权限。
    profile: Option<String>,
    assignment: SubAgentAssignment,
    allowed_tools: Option<Vec<String>>,
    model: Option<String>,
    model_strength: SubAgentModelStrength,
    /// 当调用方显式提供了 `model_strength` 时为 `true`。显式的强度
    /// 优先级高于舰队配置文件的模型固定/装载；解析时的默认值则不。
    model_strength_explicit: bool,
    thinking: SubAgentThinking,
    /// 子级的可选工作目录。必须规范化为父级工作区内的路径。
    /// 对于一流的 git worktree 隔离，请使用 `worktree` 而非手动预创建 cwd。
    cwd: Option<PathBuf>,
    /// 可选的一流 git worktree 隔离。设置时，CodeWhale
    /// 创建一个同级的 worktree/分支并从该检出运行子级。
    worktree: Option<SubAgentWorktreeRequest>,
    /// 缓存感知常驻模式的可选文件路径（#529）。设置时，
    /// 子级的提示词会前缀文件内容，以实现前缀缓存局部性。
    /// 全局所有权表防止两个代理同时持有同一文件的常驻租约。
    resident_file: Option<String>,
    /// 为 true 时，在附加子级任务之前，使用父级的系统提示词和消息前缀种子化子级。
    fork_context: bool,
    /// 后代的遗留递归预算。面向模型的子级工具接口仅为叶子节点；
    /// 此字段为持久化/内部记录保留。
    max_depth: Option<u32>,
    /// 此子级及其后代的可选聚合 token 预算。
    /// 未设置时，子级继承父级的预算池或配置的根默认值。
    token_budget: Option<u64>,
    /// 来自调用方的额外工具拒绝列表，与父运行时的继承拒绝列表合并。
    /// 拒绝始终优先于允许（#4042）。
    disallowed_tools: Option<Vec<String>>,
    /// 为 true（默认）时，子级继承父运行时的 `disallowed_tools`。
    /// 设置为 `false` 以让子级从干净的状态开始（仅应用上面显式的 `disallowed_tools`，如有）。
    inherit_disallowed_tools: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SubAgentWorktreeRequest {
    branch: Option<String>,
    path: Option<PathBuf>,
    base_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentUsageBudgetScope {
    scope_id: String,
    limit: u64,
    spent: u64,
    remaining: u64,
}

/// 中断子代理会话的持久恢复点。
///
/// `messages` 是字节有界的尾部（#3882），而非完整历史：
/// 检查点每步触发并克隆到快照/持久化中，因此无界克隆
/// 会在 Fleet 扇出下放大大型工具输出。
/// `message_count` 记录真实总数，`omitted_messages` 记录此快照中丢弃了多少条最旧消息；
/// 溢出的工具输出保留在磁盘上的溢出目录中。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubAgentCheckpoint {
    pub checkpoint_id: String,
    pub agent_id: String,
    pub continuation_handle: String,
    pub reason: String,
    pub continuable: bool,
    pub steps_taken: u32,
    pub message_count: usize,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<Message>,
    /// 为遵守检查点字节预算，从 `messages` 中省略的最旧消息数。
    /// 在 v0.8.67 之前写入的记录为 `0`（serde 默认值）。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub omitted_messages: usize,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSubAgent {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_name: Option<String>,
    #[serde(default)]
    fork_context: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace: Option<PathBuf>,
    agent_type: SubAgentType,
    prompt: String,
    assignment: SubAgentAssignment,
    #[serde(default)]
    model: String,
    #[serde(default)]
    nickname: Option<String>,
    status: SubAgentStatus,
    result: Option<String>,
    steps_taken: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint: Option<SubAgentCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    needs_input: Option<SubAgentNeedsInput>,
    duration_ms: u64,
    allowed_tools: Vec<String>,
    updated_at_ms: u64,
    /// 生成此代理的管理器/进程启动的稳定 id（#405）。
    /// 让新的管理器过滤出由先前会话持久化的代理。
    /// 使用 `#[serde(default)]` 可选，以保证向后兼容性——旧记录缺少此字段，
    /// 加载时为空字符串，管理器将其视为 "from_prior_session"，
    /// 因为它无法匹配任何当前 id。
    #[serde(default)]
    session_boot_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSubAgentState {
    schema_version: u32,
    agents: Vec<PersistedSubAgent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    workers: Vec<AgentWorkerRecord>,
}

impl Default for PersistedSubAgentState {
    fn default() -> Self {
        Self {
            schema_version: SUBAGENT_STATE_SCHEMA_VERSION,
            agents: Vec::new(),
            workers: Vec::new(),
        }
    }
}

/// 子代理递归深度的默认上限。通过配置中的 `[subagents] max_depth = N` 覆盖。
///
/// 来源于 [`codewhale_config::DEFAULT_SPAWN_DEPTH`]，以使独立子代理和舰队工作者共享一个递归轴（无"两个移动
/// 目标"）。配置/请求的深度限制在 [`codewhale_config::MAX_SPAWN_DEPTH_CEILING`] 内。
pub const DEFAULT_MAX_SPAWN_DEPTH: u32 = codewhale_config::DEFAULT_SPAWN_DEPTH;

/// 从（已递增的）`spawn_depth` 和模型提供的每次调用 `max_depth` 解析子运行时的 `max_spawn_depth`，
/// 限制到绝对的 [`codewhale_config::MAX_SPAWN_DEPTH_CEILING`]。
///
/// 没有绝对限制时，`max_spawn_depth = spawn_depth + max_depth`
/// 会使递归门控（`spawn_depth + 1 > max_spawn_depth`）在每一层简化为
/// `1 > max_depth`——当模型每次生成都重新提供 `max_depth >= 1` 时总是 false——
/// 因此环深度会增长到全局准入上限，而非预期的 8 环上限。
fn clamp_child_max_spawn_depth(child_spawn_depth: u32, requested_max_depth: u32) -> u32 {
    child_spawn_depth
        .saturating_add(requested_max_depth)
        .min(codewhale_config::MAX_SPAWN_DEPTH_CEILING)
}

/// 当一个子级完成时，向直接父级的完成收件箱发出的终止状态通知（issue #756）。
/// 对于根生成的代理，该收件箱是引擎轮次循环；对于嵌套代理，
/// 它是 `run_subagent` 内部的父级本地接收器。
/// 携带已经渲染的 `<codewhale:subagent.done>` 哨兵，
/// 模型按 `prompts/constitution.md` 期望在转录中收到该哨兵。
#[derive(Debug, Clone)]
pub struct SubAgentCompletion {
    /// 完成子级的代理 id。用于路由/日志记录——引擎的轮次循环目前并不以此作为键（它只注入有效载荷），
    /// 但下游工具和测试需要此字段。
    #[allow(dead_code)]
    pub agent_id: String,
    /// 第 1 行为人类可读摘要，第 2 行为哨兵。与 `Event::AgentComplete::result` 相同的有效载荷形状。
    pub payload: String,
}

/// 可供选择上下文分叉的子代理使用的父级转录快照。
/// 系统提示词和前置消息与父级请求保持字节一致，
/// 以便 DeepSeek 的前缀缓存可以复用已预热的前缀。
#[derive(Clone, Debug)]
pub struct SubAgentForkContext {
    pub system: Option<SystemPrompt>,
    pub messages: Vec<Message>,
    pub structured_state_block: Option<String>,
}

/// 生成子代理的运行时配置。
///
/// 携带子级所需的一切：(a) 构建自己的工具注册表——包括管理器以便孙级可以生成——
/// 以及 (b) 与生命周期取消和深度上限协同。`child_runtime()` 链接取消令牌，
/// 而 `background_runtime()` 有意将长期运行的 `agent` 会话与调用方的轮次令牌分离。
#[derive(Clone)]
pub struct SubAgentRuntime {
    pub client: DeepSeekClient,
    /// 会话 `Config` 快照，用于在舰队名册成员的配置文件固定了不同提供商时，
    /// 构建一个绑定到该提供商的*全新* LLM 客户端（#4193，#4181 中无头 `codewhale exec --provider`
    /// 路由的交互式 TUI 孪生版本）。引擎通过 [`SubAgentRuntime::with_api_config`] 传入它；
    /// `child_runtime`/`background_runtime` 克隆 `Arc`，以便每个后代都可以重新派生提供商 B 的客户端。
    ///
    /// 从未传入配置的遗留/测试运行时为 `None`。
    /// 当配置文件固定了与会话不同的提供商且此为 `None`
    /// （或固定提供商的凭据无法解析）时，生成失败而非静默复用会话客户端——
    /// 静默复用会将模型 B 的 id 发送到提供商 A 的端点，这正是 #4093 缺陷。
    pub api_config: Option<std::sync::Arc<crate::config::Config>>,
    pub model: String,
    pub auto_model: bool,
    pub reasoning_effort: Option<String>,
    pub reasoning_effort_auto: bool,
    pub role_models: HashMap<String, String>,
    /// 命名代理角色的共享舰队名册（#fleet-roster 切换 (v0.8.67)）。
    /// 默认仅内置；引擎安装合并的内置/配置/工作区名册，
    /// 以便模型生成的子代理和舰队调度解析同一方。克隆到子运行时。
    pub fleet_roster: std::sync::Arc<crate::fleet::roster::FleetRoster>,
    pub context: ToolContext,
    pub allow_shell: bool,
    /// 为 true 时，对于可写角色，Suggest 级别的文件写入自动接受，无需完整的父级自动批准。
    /// Shell/网络/MCP 仍然门控。为 Workflow 生成的子级设置。
    pub accept_edits: bool,
    /// 从父级轮次继承的原生 Agent 模式工具表面。携带依赖于特性/配置的工具族，
    /// 如 web 搜索、patch、memory、vision、notify 和 FIM，以使子级目录与父级保持对等。
    pub agent_tool_surface_options: AgentToolSurfaceOptions,
    /// 由后代继承的能力合约。`agent` 在注册工作者记录之前从此派生子级配置文件，
    /// 以便父级、子代理和舰队投影共享一个工作者合约。
    pub worker_profile: WorkerRuntimeProfile,
    pub event_tx: Option<mpsc::Sender<Event>>,
    /// 管理器句柄，以便子级可以通过 `agent` 递归。所有深度的所有代理共享同一个管理器。
    pub manager: SharedSubAgentManager,
    /// 生成树中的深度。0 = 顶级用户轮次；1 = 直接子级；依次类推。
    /// 子级克隆父运行时并在生成时递增此值。
    pub spawn_depth: u32,
    /// 应记录为通过此运行时的模型可见 `agent` 工具生成的任何子级的父级代理 id。
    /// 对于根引擎为 `None`；对于嵌套生成设置为正在运行的子代理 id，以便 UI 表面可以渲染树。
    pub parent_agent_id: Option<String>,
    /// 递归深度的硬上限。`spawn_depth + 1` 将超过此值的子级在生成入口处被拒绝。
    /// 使用 `>`（严格大于），以便相等是允许的——与 codex 的模式匹配。
    pub max_spawn_depth: u32,
    /// 协作取消令牌。直接调用 `child_runtime()` 的调用方从父级派生子令牌；
    /// 模型可见的 `agent` 使用 `background_runtime()` 将该令牌替换为分离的令牌。
    pub cancel_token: CancellationToken,
    /// 结构化进度/生命周期流。在子级间克隆，以便整个生成树发布到同一个有序、可扇出的邮箱中。
    /// 仅当没有消费者连接时（遗留入口点/测试）为 `None`。
    pub mailbox: Option<Mailbox>,
    /// 此运行时直接父级的唤醒通道（issue #756）。对于引擎的直接子级，
    /// 这指向引擎轮次循环。当子代理运行时，其工具注册表将此替换为本地收件箱，
    /// 以便嵌套子级向它们的编排子代理报告，而不是淹没根父级。
    /// 当没有消费者连接时（测试/遗留路径）为 `None`。
    pub parent_completion_tx: Option<mpsc::UnboundedSender<SubAgentCompletion>>,
    /// 可选分叉子级可见的请求前缀快照。
    pub fork_context: Option<SubAgentForkContext>,
    /// 父级的 MCP 池（如有）。
    pub mcp_pool: Option<std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpPool>>>,
    /// 子级 `create_message` 调用的每步 DeepSeek API 超时。
    /// 在引擎构建时从 `[subagents] api_timeout_secs`（限制在 1..=1800）解析，
    /// 以便缓慢但合法的模型轮次不会在子级思考过程中错误超时。
    /// `child_runtime()` 和 `background_runtime()` 保留父级的值（#1806, #1808）。
    pub step_api_timeout: Duration,
    /// 子代理步骤中单次工具执行的挂钟预算。
    /// 默认为 `DEFAULT_TOOL_TIMEOUT`；引擎可以覆盖它，以便长时间但合法
    /// 的工具运行不会在执行中途被杀死。`child_runtime()` 保留父级的值。
    pub tool_timeout: Duration,
    /// 子注册表继承的小米 MiMo 语音/TTS 工具输出的默认目录。
    /// 使父级和子代理的 `speech` / `tts` 工具共享相同的 `[speech].output_dir` / 环境变量覆盖。
    pub speech_output_dir: Option<PathBuf>,
    /// 共享待办列表——父级的 `SharedTodoList`，克隆到每个子级中，
    /// 以使子代理的 `checklist_update` 调用在 Work 侧边栏中实时可见。
    /// 没有这个，每个子级会获得一个全新的隔离列表，父级直到完成才能看到子级进度。
    pub todos: SharedTodoList,
    /// 生成时编排父级的会话模式（Wave 7 M4/M5）。
    pub parent_mode: AppMode,
}

impl SubAgentRuntime {
    /// 创建子代理执行的顶级运行时配置。
    /// 在引擎构建父级工具注册表将通过的运行时中使用此方法。
    /// 子级应通过 `Self::child_runtime` 派生其运行时。
    #[must_use]
    pub fn new(
        client: DeepSeekClient,
        model: String,
        context: ToolContext,
        allow_shell: bool,
        event_tx: Option<mpsc::Sender<Event>>,
        manager: SharedSubAgentManager,
    ) -> Self {
        Self {
            client,
            api_config: None,
            model,
            auto_model: false,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            role_models: HashMap::new(),
            fleet_roster: std::sync::Arc::new(crate::fleet::roster::FleetRoster::built_ins_only()),
            context,
            allow_shell,
            accept_edits: false,
            agent_tool_surface_options: AgentToolSurfaceOptions::new(
                ShellPolicy::from_legacy_allow_shell(allow_shell),
            ),
            worker_profile: WorkerRuntimeProfile::for_role(SubAgentType::General),
            event_tx,
            manager,
            spawn_depth: 0,
            parent_agent_id: None,
            max_spawn_depth: DEFAULT_MAX_SPAWN_DEPTH,
            cancel_token: CancellationToken::new(),
            mailbox: None,
            parent_completion_tx: None,
            fork_context: None,
            mcp_pool: None,
            step_api_timeout: DEFAULT_STEP_API_TIMEOUT,
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
            speech_output_dir: None,
            todos: crate::tools::todo::new_shared_todo_list(),
            parent_mode: AppMode::Agent,
        }
    }

    /// 保留父级会话模式，用于生成策略决策。
    #[must_use]
    pub fn with_parent_mode(mut self, mode: AppMode) -> Self {
        self.parent_mode = mode;
        self
    }

    /// 附加父级的共享待办列表，以便子代理的 `checklist_update` 调用在 Work 侧边栏中实时可见。
    /// 没有这个，子级会获得全新的隔离列表。
    #[must_use]
    pub fn with_todos(mut self, todos: SharedTodoList) -> Self {
        self.todos = todos;
        self
    }

    /// 保留父级 Agent 模式的原生工具表面，用于子级注册表。
    #[must_use]
    pub fn with_agent_tool_surface_options(mut self, options: AgentToolSurfaceOptions) -> Self {
        self.speech_output_dir = options.speech_output_dir.clone();
        self.agent_tool_surface_options = options;
        self
    }

    /// 附加 MCP 池，以便子代理可以执行 MCP 工具。
    #[must_use]
    pub fn with_mcp_pool(
        mut self,
        pool: Option<std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpPool>>>,
    ) -> Self {
        self.mcp_pool = pool;
        self
    }

    /// 覆盖每步 DeepSeek API 超时（默认 `DEFAULT_STEP_API_TIMEOUT`）。
    /// 由引擎在读取 `[subagents] api_timeout_secs` 后调用。
    /// 测试可以使用此方法快速失败，无需等待遗留的 120 秒（#1806, #1808）。
    #[must_use]
    pub fn with_step_api_timeout(mut self, timeout: Duration) -> Self {
        self.step_api_timeout = timeout;
        self
    }

    /// 保留为子代理工具配置的语音输出目录。
    #[must_use]
    pub fn with_speech_output_dir(mut self, output_dir: Option<PathBuf>) -> Self {
        self.speech_output_dir = output_dir.clone();
        self.agent_tool_surface_options.speech_output_dir = output_dir;
        self
    }

    /// 附加此运行时直接父级的唤醒通道。引擎将此用于直接子级；
    /// 正在运行的子代理在传递给其嵌套 `agent` 工具的运行时中替换它，
    /// 以便子级完成事件路由回生成它们的子代理。
    #[must_use]
    pub fn with_parent_completion_tx(
        mut self,
        tx: mpsc::UnboundedSender<SubAgentCompletion>,
    ) -> Self {
        self.parent_completion_tx = Some(tx);
        self
    }

    /// 附加当前父级请求前缀，用于 `fork_context` 生成。
    #[must_use]
    pub fn with_fork_context(mut self, context: SubAgentForkContext) -> Self {
        self.fork_context = Some(context);
        self
    }

    /// 附加一个 `Mailbox`，以便此运行时及其派生子级发布结构化的 `MailboxMessage` 信封，
    /// 与遗留的 `Event` 流并行。当邮箱关闭令牌应与此运行时的取消令牌匹配时，
    /// 与 [`Self::with_cancel_token`] 配对使用。
    #[must_use]
    #[allow(dead_code)] // wired by #128 (in-transcript cards) when it lands.
    pub fn with_mailbox(mut self, mailbox: Mailbox) -> Self {
        self.mailbox = Some(mailbox);
        self
    }

    /// 替换取消令牌（例如，当引擎构建运行时，同时有一个绑定到相同令牌的邮箱时）。
    #[must_use]
    #[allow(dead_code)] // wired by #128 alongside `with_mailbox`.
    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = token;
        self
    }

    /// 覆盖最大生成深度（默认 `DEFAULT_MAX_SPAWN_DEPTH`）。
    /// 由配置连接（`[subagents] max_depth = N`）和测试使用。
    #[must_use]
    #[allow(dead_code)]
    pub fn with_max_spawn_depth(mut self, max: u32) -> Self {
        self.max_spawn_depth = max;
        self
    }

    /// 附加原始角色/类型模型覆盖。值有意在生成时验证，以便错误配置在部分生成之前失败。
    #[must_use]
    pub fn with_role_models(mut self, role_models: HashMap<String, String>) -> Self {
        self.role_models = role_models;
        self
    }

    /// 附加会话 `Config`，以便生成可以为舰队配置文件的固定提供商构建全新的 LLM 客户端（#4193）。
    /// 没有它，跨提供商的进程内生成将静默失败而非错误路由
    /// （参见 [`api_config`](Self::api_config) 字段文档）。仅引擎连接；
    /// 测试和遗留运行时可以保持未设置。
    #[must_use]
    pub fn with_api_config(mut self, config: crate::config::Config) -> Self {
        self.api_config = Some(std::sync::Arc::new(config));
        self
    }

    /// 从传入的会话 `Config`（#4193）构建绑定到 `provider_id` 的 LLM 客户端。
    /// 镜像了经过验证的每提供商客户端工厂，该工厂被每轮自动路由（`model_routing`）
    /// 和引擎的提供商切换使用：克隆会话配置，仅覆盖其 `provider`，
    /// 让 [`DeepSeekClient::new`] 从配置/环境变量中重新解析该提供商的 base URL + 凭据。
    /// `provider_id` 可以是内置提供商 id 或用户命名的 `[providers.<id>] kind="openai-compatible"`
    /// 自定义提供商，如 `lm-studio`（#3965）。
    ///
    /// 当没有传入配置时，或当提供商的凭据/base URL 无法解析时返回 `Err`。
    /// 调用方必须暴露该错误，而不是回退到会话客户端：静默回退会将固定的模型 id
    /// 发送到会话提供商的端点（#4093）。
    fn client_for_provider_id(&self, provider_id: &str) -> Result<DeepSeekClient, String> {
        let Some(api_config) = self.api_config.as_ref() else {
            return Err(
                "session Config was not threaded into this runtime; cannot build a \
                 provider-pinned client"
                    .to_string(),
            );
        };
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            return Err("provider pin was blank".to_string());
        }
        let built_in = crate::config::ApiProvider::parse(provider_id);
        let custom = built_in.is_none()
            && api_config
                .providers
                .as_ref()
                .and_then(|providers| providers.custom_provider_config(provider_id))
                .is_some();
        if built_in.is_none() && !custom {
            return Err(format!(
                "provider '{provider_id}' is neither a built-in provider nor a configured \
                 [providers.{provider_id}] custom provider"
            ));
        }
        let mut provider_config = (**api_config).clone();
        // EPIC #2608:提供商从配置文件固定中原样获取（内置 id 或已配置的自定义 id），
        // 从不从模型 id 推断。仅覆盖 `provider` 使 `Config::api_provider`、
        // `deepseek_base_url` 和 `deepseek_api_key` 全部为固定的提供商重新解析。
        provider_config.provider = Some(
            built_in
                .map(|provider| provider.as_str().to_string())
                .unwrap_or_else(|| provider_id.to_string()),
        );
        DeepSeekClient::new(&provider_config).map_err(|err| err.to_string())
    }

    /// 安装合并的舰队名册（#fleet-roster 切换 (v0.8.67)）。
    /// 引擎为每个会话配置构建一次；子级继承它。
    #[must_use]
    pub fn with_fleet_roster(
        mut self,
        roster: std::sync::Arc<crate::fleet::roster::FleetRoster>,
    ) -> Self {
        self.fleet_roster = roster;
        self
    }

    /// 保留父级会话是否使用每轮模型路由。
    #[must_use]
    pub fn with_auto_model(mut self, auto_model: bool) -> Self {
        self.auto_model = auto_model;
        self
    }

    /// 保留父级的思考配置。子级模型强度在 `agent` 调用上是显式的；
    /// 此字段仅控制推理努力。
    #[must_use]
    pub fn with_reasoning_effort(
        mut self,
        reasoning_effort: Option<String>,
        reasoning_effort_auto: bool,
    ) -> Self {
        self.reasoning_effort = reasoning_effort;
        self.reasoning_effort_auto = reasoning_effort_auto;
        self
    }

    /// 返回一个有意与父级轮次取消令牌分离的子运行时。
    /// 后台子代理应在父级轮次被取消时继续运行；
    /// 显式的代理取消仍然通过管理器中止它们的任务句柄。
    #[must_use]
    pub fn background_runtime(&self) -> Self {
        let mut runtime = self.child_runtime();
        let token = CancellationToken::new();
        runtime.cancel_token = token.clone();
        runtime.context.cancel_token = Some(token);
        runtime
    }

    /// 构建一个子运行时，克隆当前运行时，递增 `spawn_depth`，
    /// 并派生子取消令牌。在生成入口处用于构建新子代理将看到的运行时。
    ///
    /// 子级继承父级的批准状态。非自动批准的父级仍然可以委托只读调查，
    /// 但需要批准的子级工具会被子代理注册表阻止，而不是在无提示的情况下静默运行。
    #[must_use]
    pub fn child_runtime(&self) -> Self {
        let mut child_context = self.context.clone();
        child_context.auto_approve = self.context.auto_approve;
        Self {
            client: self.client.clone(),
            api_config: self.api_config.clone(),
            model: self.model.clone(),
            auto_model: self.auto_model,
            reasoning_effort: self.reasoning_effort.clone(),
            reasoning_effort_auto: self.reasoning_effort_auto,
            role_models: self.role_models.clone(),
            fleet_roster: self.fleet_roster.clone(),
            context: child_context,
            allow_shell: self.allow_shell,
            accept_edits: self.accept_edits,
            agent_tool_surface_options: self.agent_tool_surface_options.clone(),
            worker_profile: self.worker_profile.clone(),
            event_tx: self.event_tx.clone(),
            manager: self.manager.clone(),
            spawn_depth: self.spawn_depth + 1,
            parent_agent_id: self.parent_agent_id.clone(),
            max_spawn_depth: self.max_spawn_depth,
            cancel_token: self.cancel_token.child_token(),
            mailbox: self.mailbox.clone(),
            parent_completion_tx: self.parent_completion_tx.clone(),
            fork_context: self.fork_context.clone(),
            mcp_pool: self.mcp_pool.clone(),
            step_api_timeout: self.step_api_timeout,
            tool_timeout: self.tool_timeout,
            speech_output_dir: self.speech_output_dir.clone(),
            todos: self.todos.clone(),
            parent_mode: self.parent_mode,
        }
    }

    /// 下一次生成是否会超出深度上限。
    #[must_use]
    pub fn would_exceed_depth(&self) -> bool {
        self.spawn_depth + 1 > self.max_spawn_depth
    }
}

/// 一个正在运行的子代理实例。
pub struct SubAgent {
    pub id: String,
    pub session_name: String,
    pub fork_context: bool,
    pub agent_type: SubAgentType,
    pub prompt: String,
    pub assignment: SubAgentAssignment,
    pub model: String,
    pub nickname: Option<String>,
    pub status: SubAgentStatus,
    pub result: Option<String>,
    pub steps_taken: u32,
    pub checkpoint: Option<SubAgentCheckpoint>,
    pub needs_input: Option<SubAgentNeedsInput>,
    pub started_at: Instant,
    pub last_activity_at: Instant,
    /// `None` = 完整注册表继承，需要批准的工具仍被阻止，除非父运行时是自动批准的。
    /// `Some(list)` = 显式窄化允许列表（自定义代理，遗留）。
    pub allowed_tools: Option<Vec<String>>,
    /// 生成此代理的管理器的稳定 id（#405）。与管理器的 `current_session_boot_id` 比较，
    /// 以在列出时将代理分类为当前会话与先前会话。
    pub session_boot_id: String,
    pub workspace: PathBuf,
    input_tx: Option<mpsc::UnboundedSender<SubAgentInput>>,
    task_handle: Option<JoinHandle<()>>,
}

impl SubAgent {
    /// 创建一个新的子代理。`id` 由调用方生成，以便确定性鲸鱼命名可以在构造前哈希 ID。
    #[allow(clippy::too_many_arguments)]
    fn new(
        id: String,
        agent_type: SubAgentType,
        prompt: String,
        assignment: SubAgentAssignment,
        model: String,
        nickname: Option<String>,
        allowed_tools: Option<Vec<String>>,
        input_tx: mpsc::UnboundedSender<SubAgentInput>,
        workspace: PathBuf,
        session_boot_id: String,
    ) -> Self {
        let session_name = id.clone();

        let started_at = Instant::now();
        Self {
            id,
            session_name,
            fork_context: false,
            agent_type,
            prompt,
            assignment,
            model,
            nickname,
            status: SubAgentStatus::Running,
            result: None,
            steps_taken: 0,
            checkpoint: None,
            needs_input: None,
            started_at,
            last_activity_at: started_at,
            allowed_tools,
            session_boot_id,
            workspace,
            input_tx: Some(input_tx),
            task_handle: None,
        }
    }

    /// 获取当前状态的快照。
    #[must_use]
    pub fn snapshot(&self) -> SubAgentResult {
        SubAgentResult {
            name: self.session_name.clone(),
            agent_id: self.id.clone(),
            context_mode: if self.fork_context { "forked" } else { "fresh" }.to_string(),
            fork_context: self.fork_context,
            workspace: Some(self.workspace.clone()),
            git_branch: current_git_branch(&self.workspace),
            agent_type: self.agent_type.clone(),
            assignment: self.assignment.clone(),
            model: self.model.clone(),
            nickname: self.nickname.clone(),
            status: self.status.clone(),
            worker_status: None,
            parent_run_id: None,
            spawn_depth: 0,
            result: self.result.clone(),
            steps_taken: self.steps_taken,
            checkpoint: self.checkpoint.clone(),
            needs_input: self.needs_input.clone(),
            duration_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            // 来自代理自身的快照不知道管理器的当前启动 id，因此默认为 false。
            // 管理器在通过自己的 `snapshot_for_listing` 辅助函数生成快照时填充此值（#405）。
            from_prior_session: false,
        }
    }
}

/// 活跃子代理的管理器。
pub struct SubAgentManager {
    agents: HashMap<String, SubAgent>,
    worker_records: HashMap<String, AgentWorkerRecord>,
    worker_event_seq: u64,
    #[allow(dead_code)] // 为未来的工作区范围操作而存储
    workspace: PathBuf,
    state_path: Option<PathBuf>,
    max_steps: u32,
    max_agents: usize,
    max_admitted_agents: usize,
    default_token_budget: Option<u64>,
    running_heartbeat_timeout: Duration,
    /// 管理器构造时分配的稳定 id（#405）。印在管理器生成的每个代理上；
    /// 从持久化状态文件加载的代理携带先前会话印制的任何 id
    /// （或 pre-#405 记录为空）。管理器将 `session_boot_id` 与此值不匹配的代理
    /// 分类为"来自先前会话"，以便列表可以默认隐藏它们。
    current_session_boot_id: String,
    /// 直接（深度为 1）子代理启动的启动门控（#3095）。每个许可对应一个正在执行
    /// 的直接子级；后续的直接子级立即生成，但在启动前排队等待许可，
    /// 发布可见的"已排队"原因而非爆发式启动。更深的后代绕过门控，
    /// 以便持有许可且正在等待其自己子级的父级不会死锁树。
    launch_gate: Arc<Semaphore>,
    /// #freeze: 热路径持久化防抖记账（参见 `SUBAGENT_PERSIST_DEBOUNCE`）。
    /// `last_persist_at` 是任何状态持久化上次运行的时间；
    /// `persist_pending` 记录一个热路径写入被合并掉了，
    /// 以便后续的刷新（终止写入或关闭）可以捕获最新的检查点。
    last_persist_at: Option<Instant>,
    persist_pending: bool,
    /// #3803: `cleanup` 上次运行的时间。侧边栏刷新（`Op::ListSubAgents`）从只读的 `list()` 快照渲染，
    /// 并且仅在有限节奏下运行写锁定的 `cleanup`，
    /// 因此子代理扇出期间的 UI 刷新风暴不再在每次请求时争夺写锁。
    last_cleanup_at: Option<Instant>,
}

impl SubAgentManager {
    /// 为子代理创建一个新管理器。
    #[must_use]
    pub fn new(workspace: PathBuf, max_agents: usize) -> Self {
        Self {
            agents: HashMap::new(),
            worker_records: HashMap::new(),
            worker_event_seq: 0,
            workspace,
            state_path: None,
            max_steps: DEFAULT_MAX_STEPS,
            max_agents,
            max_admitted_agents: max_agents,
            default_token_budget: None,
            running_heartbeat_timeout: Duration::from_secs(
                crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS,
            ),
            // 每个管理器的新启动 id。由 #405 用于将重新加载的持久化代理分类为"先前会话"。
            current_session_boot_id: format!("boot_{}", &Uuid::new_v4().to_string()[..12]),
            // 默认启动并发度 = 完整代理上限；门控仅在配置了较低的 `launch_concurrency` 时才会限流。
            launch_gate: Arc::new(Semaphore::new(max_agents.max(1))),
            last_persist_at: None,
            persist_pending: false,
            last_cleanup_at: None,
        }
    }

    /// 设置可以并发执行的直接子级数量，超过此数量后进一步的启动将排队（#3095）。
    /// 限制在 `1..=max_agents`。
    #[must_use]
    pub fn with_launch_concurrency(mut self, limit: usize) -> Self {
        self.launch_gate = Arc::new(Semaphore::new(limit.clamp(1, self.max_agents)));
        self
    }

    /// 设置此管理器的总排队 + 运行准入上限。
    /// 该值始终至少为瞬时并发上限。
    #[must_use]
    pub fn with_admission_limit(mut self, max_admitted: usize) -> Self {
        self.max_admitted_agents =
            max_admitted.clamp(self.max_agents, crate::config::MAX_SUBAGENT_ADMISSION);
        self
    }

    /// 设置根子代理运行的默认聚合 token 预算。
    /// `None` 和 `Some(0)` 都保留无限制的遗留行为。
    #[must_use]
    pub fn with_default_token_budget(mut self, budget: Option<u64>) -> Self {
        self.default_token_budget = positive_token_budget(budget);
        self
    }

    /// 返回此管理器在其生成的代理上印制的启动 id。
    /// 对测试公开；内部调用方直接使用该字段。
    #[cfg(test)]
    pub fn session_boot_id(&self) -> &str {
        &self.current_session_boot_id
    }

    /// 根据 `session_boot_id` 对代理进行分类：当代理 (a) 从磁盘加载且无 id，
    /// 或 (b) 携带与管理器当前启动不同的 id 时返回 `true`。
    /// 默认过滤列表输出（#405）。
    fn is_from_prior_session(&self, agent: &SubAgent) -> bool {
        agent.session_boot_id.is_empty() || agent.session_boot_id != self.current_session_boot_id
    }

    #[must_use]
    fn with_state_path(mut self, path: PathBuf) -> Self {
        self.state_path = Some(path);
        self
    }

    #[must_use]
    pub fn with_running_heartbeat_timeout(mut self, timeout: Duration) -> Self {
        self.running_heartbeat_timeout = if timeout.is_zero() {
            Duration::from_secs(crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS)
        } else {
            timeout
        };
        self
    }

    /// 应用实时运行时限制。仅当当前没有子代理运行时，启动信号量才会被替换，
    /// 因为活跃任务可能仍持有来自先前信号量的许可。
    pub fn update_runtime_limits(
        &mut self,
        max_agents: usize,
        max_admitted_agents: usize,
        running_heartbeat_timeout: Duration,
        launch_concurrency: usize,
        default_token_budget: Option<u64>,
    ) -> bool {
        self.max_agents = max_agents.clamp(1, crate::config::MAX_SUBAGENTS);
        self.max_admitted_agents =
            max_admitted_agents.clamp(self.max_agents, crate::config::MAX_SUBAGENT_ADMISSION);
        self.default_token_budget = positive_token_budget(default_token_budget);
        self.running_heartbeat_timeout = if running_heartbeat_timeout.is_zero() {
            Duration::from_secs(crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS)
        } else {
            running_heartbeat_timeout
        };
        if self.running_count() == 0 {
            self.launch_gate =
                Arc::new(Semaphore::new(launch_concurrency.clamp(1, self.max_agents)));
            true
        } else {
            false
        }
    }

/// 从当前舰队构建 [`PersistedSubAgentState`] 快照。
///
/// 这是一个在调用方锁下运行的廉价克隆操作。
/// 返回的有效载荷完全拥有所有权，可以安全地移动到后台线程进行磁盘 I/O。
    fn build_persist_payload(&self) -> Result<Option<(PathBuf, PersistedSubAgentState)>> {
        let Some(path) = self.state_path.as_ref() else {
            return Ok(None);
        };
        let path = checked_subagent_state_path(&self.workspace, path)?;
        let now_ms = epoch_millis_now();
        let mut agents = Vec::with_capacity(self.agents.len());
        for agent in self.agents.values() {
            agents.push(PersistedSubAgent {
                id: agent.id.clone(),
                session_name: Some(agent.session_name.clone()),
                fork_context: agent.fork_context,
                workspace: Some(agent.workspace.clone()),
                agent_type: agent.agent_type.clone(),
                prompt: agent.prompt.clone(),
                assignment: agent.assignment.clone(),
                model: agent.model.clone(),
                nickname: agent.nickname.clone(),
                status: agent.status.clone(),
                result: agent.result.clone(),
                steps_taken: agent.steps_taken,
                checkpoint: agent.checkpoint.clone(),
                needs_input: agent.needs_input.clone(),
                duration_ms: u64::try_from(agent.started_at.elapsed().as_millis())
                    .unwrap_or(u64::MAX),
                // 向后兼容：磁盘上为 Vec。None → 空 vec；Some(list) → 列表。
                // 重新加载将空 vec 转换回 None（完整继承）。
                allowed_tools: agent.allowed_tools.clone().unwrap_or_default(),
                updated_at_ms: now_ms,
                session_boot_id: agent.session_boot_id.clone(),
            });
        }
        agents.sort_by(|a, b| a.id.cmp(&b.id));

        let payload = PersistedSubAgentState {
            schema_version: SUBAGENT_STATE_SCHEMA_VERSION,
            agents,
            workers: self.sorted_worker_records(),
        };
        Ok(Some((path, payload)))
    }

    /// 将当前舰队状态持久化到磁盘。
    ///
    /// #freeze: JSON 序列化在调用方锁下廉价运行；昂贵的磁盘 I/O（`write_json_atomic`）
    /// 被生成到一个后台线程，以便调用方的写锁在接触文件系统之前被释放。
    ///
    /// 返回一个 [`std::thread::JoinHandle`]，在磁盘写入完成时解析。
    /// 调用方可以对它使用 `.join()` 获得同步语义，或丢弃它以执行即发即弃。
    fn persist_state(&self) -> Result<std::thread::JoinHandle<()>> {
        let Some((path, payload)) = self.build_persist_payload()? else {
            // 没有需要持久化的内容——返回一个空操作句柄。
            return Ok(std::thread::spawn(|| {}));
        };
        let workspace = self.workspace.clone();
        // 将磁盘 I/O 生成为写锁热路径之外的任务。`payload` 完全拥有所有权
        // （从 `self.agents` 克隆），因此它是 `Send` 且可以安全移动。
        let handle = std::thread::spawn(move || {
            if let Err(err) = write_json_atomic(&workspace, &path, &payload) {
                tracing::warn!(target: "subagent", ?err, "failed to persist sub-agent state");
            }
        });
        Ok(handle)
    }

    /// 即发即弃的持久化——记录错误，丢弃 join 句柄。
    fn persist_state_best_effort(&self) {
        if let Err(err) = self.persist_state() {
            // 不能使用 `eprintln!`——alt-screen 内的原始 stderr 会泄漏到缓冲区中，
            // 产生滚动恶魔回归（#1085）。通过 tracing 路由，以便 `runtime_log` 中的文件订阅者捕获它。
            tracing::warn!(target: "subagent", ?err, "failed to persist sub-agent state");
        } else {
            // Join 句柄在此处丢弃——磁盘 I/O 在后台继续。
        }
    }

    /// #freeze: 在热路径的每步检查点上持久化，每个 `SUBAGENT_PERSIST_DEBOUNCE` 间隔内
    /// 最多合并为一次磁盘写入。跳过的写入设置 `persist_pending`，
    /// 以便下一次终止持久化（总是重写整个舰队）或 `flush_pending_persist` 捕获它。
    fn persist_state_debounced(&mut self) {
        let now = Instant::now();
        let due = match self.last_persist_at {
            Some(last) => now.duration_since(last) >= SUBAGENT_PERSIST_DEBOUNCE,
            None => true,
        };
        if due {
            self.last_persist_at = Some(now);
            self.persist_pending = false;
            self.persist_state_best_effort();
            let writes =
                SUBAGENT_PERSIST_WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if subagent_perf_enabled() {
                let skipped = SUBAGENT_PERSIST_SKIPPED.load(std::sync::atomic::Ordering::Relaxed);
                tracing::info!(
                    target: "subagent_perf",
                    writes,
                    skipped,
                    agents = self.agents.len(),
                    "checkpoint persist (debounced write)"
                );
            }
        } else {
            self.persist_pending = true;
            SUBAGENT_PERSIST_SKIPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// #freeze: 如果热路径写入先前被合并掉，则强制持久化。
    /// 在优雅关闭/会话拆卸时调用，以便最新的中间检查点不会丢失。
    ///
    /// 与 [`persist_state`] 不同，此方法**同步**执行磁盘 I/O，
    /// 以保证数据在进程退出前被刷新。
    pub fn flush_pending_persist(&mut self) {
        if self.persist_pending {
            self.last_persist_at = Some(Instant::now());
            self.persist_pending = false;
            // 同步磁盘 I/O——安全，因为我们正在关闭，没有调用方依赖于快速释放写锁。
            if let Ok(Some((path, payload))) = self.build_persist_payload()
                && let Err(err) = write_json_atomic(&self.workspace, &path, &payload)
            {
                tracing::warn!(target: "subagent", ?err, "failed to flush pending sub-agent state");
            }
        }
    }

    fn load_state(&mut self) -> Result<()> {
        let Some(path) = self.state_path.as_ref() else {
            return Ok(());
        };
        let path = checked_subagent_state_path(&self.workspace, path)?;

        // 如果规范路径不存在，尝试遗留的 .deepseek/ 路径进行一次性迁移。
        // 下一次持久化将写入规范的 .codewhale/ 路径。
        let path = if path.exists() {
            path
        } else {
            let legacy = checked_subagent_state_path(
                &self.workspace,
                &Path::new(".deepseek")
                    .join("state")
                    .join(SUBAGENT_STATE_FILE),
            )?;
            if legacy.exists() {
                tracing::info!(
                    target: "subagent",
                    "loading sub-agent state from legacy path for migration: {}",
                    legacy.display()
                );
                legacy
            } else {
                return Ok(());
            }
        };

        let raw = read_subagent_state_file(&self.workspace, &path)?;
        let state = serde_json::from_str::<PersistedSubAgentState>(&raw)?;
        if state.schema_version != SUBAGENT_STATE_SCHEMA_VERSION {
            return Err(anyhow!(
                "Unsupported sub-agent state schema {}",
                state.schema_version
            ));
        }

        self.agents.clear();
        self.worker_records.clear();
        for persisted in state.agents {
            let mut status = persisted.status;
            if matches!(status, SubAgentStatus::Running) {
                status = SubAgentStatus::Interrupted(SUBAGENT_RESTART_REASON.to_string());
            }

            let started_at = instant_from_duration(Duration::from_millis(persisted.duration_ms));
            // 磁盘上的空 vec → None（完整继承，v0.6.6 默认值）。
            // 非空 vec → Some(list)（保留来自旧会话的窄范围）。
            let allowed_tools = if persisted.allowed_tools.is_empty() {
                None
            } else {
                Some(persisted.allowed_tools)
            };
            let agent = SubAgent {
                id: persisted.id.clone(),
                session_name: persisted
                    .session_name
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| persisted.id.clone()),
                fork_context: persisted.fork_context,
                workspace: persisted
                    .workspace
                    .unwrap_or_else(|| self.workspace.clone()),
                agent_type: persisted.agent_type,
                prompt: persisted.prompt,
                assignment: persisted.assignment,
                model: if persisted.model.is_empty() {
                    "unknown".to_string()
                } else {
                    persisted.model
                },
                nickname: persisted.nickname,
                status,
                result: persisted.result,
                steps_taken: persisted.steps_taken,
                checkpoint: persisted.checkpoint,
                needs_input: persisted.needs_input,
                started_at,
                last_activity_at: started_at,
                allowed_tools,
                // 加载 pre-#405 记录时为空字符串；管理器将其视为不匹配的 id——
                // 即代理被分类为先前会话。
                session_boot_id: persisted.session_boot_id,
                input_tx: None,
                task_handle: None,
            };
            self.agents.insert(persisted.id, agent);
        }
        for worker in state.workers {
            let worker = normalize_worker_record(worker);
            self.worker_event_seq = self.worker_event_seq.max(
                worker
                    .events
                    .iter()
                    .map(|event| event.seq)
                    .max()
                    .unwrap_or(0),
            );
            self.worker_records
                .insert(worker.spec.worker_id.clone(), worker);
        }
        self.refresh_all_budget_scopes();
        self.prune_worker_records();

        Ok(())
    }

    fn sorted_worker_records(&self) -> Vec<AgentWorkerRecord> {
        let mut workers: Vec<_> = self.worker_records.values().cloned().collect();
        workers.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then_with(|| a.spec.worker_id.cmp(&b.spec.worker_id))
        });
        workers
    }

    fn prune_worker_records(&mut self) {
        if self.worker_records.len() <= MAX_AGENT_WORKER_RECORDS {
            return;
        }
        let keep_ids: std::collections::HashSet<String> = self
            .sorted_worker_records()
            .into_iter()
            .take(MAX_AGENT_WORKER_RECORDS)
            .map(|record| record.spec.worker_id)
            .collect();
        self.worker_records
            .retain(|worker_id, _| keep_ids.contains(worker_id));
    }

    pub fn register_worker(&mut self, spec: AgentWorkerSpec) {
        let worker_id = spec.worker_id.clone();
        let now_ms = epoch_millis_now();
        let mut record = AgentWorkerRecord::new(normalize_worker_spec(spec), now_ms);
        self.push_worker_event(
            &mut record,
            AgentWorkerStatus::Starting,
            Some("starting".to_string()),
            None,
            None,
            now_ms,
        );
        self.worker_records.insert(worker_id, record);
        self.prune_worker_records();
    }

    pub fn list_worker_records(&self) -> Vec<AgentWorkerRecord> {
        self.sorted_worker_records()
    }

    pub fn get_worker_record(&self, worker_id: &str) -> Option<AgentWorkerRecord> {
        self.worker_records.get(worker_id).cloned()
    }

    fn aggregate_budget_spent(&self, scope_id: &str) -> u64 {
        self.worker_records
            .values()
            .filter(|record| record.usage.budget_scope.as_deref() == Some(scope_id))
            .fold(0_u64, |total, record| {
                total.saturating_add(record.usage.total_tokens.unwrap_or(0))
            })
    }

    fn inherited_budget_scope(&self, parent_run_id: Option<&str>) -> Option<(String, u64)> {
        let parent = self.worker_records.get(parent_run_id?)?;
        let limit = parent.usage.token_budget?;
        let scope_id = parent
            .usage
            .budget_scope
            .clone()
            .unwrap_or_else(|| parent.spec.worker_id.clone());
        Some((scope_id, limit))
    }

    fn resolve_spawn_budget_scope(
        &self,
        worker_id: &str,
        parent_run_id: Option<&str>,
        requested_budget: Option<u64>,
    ) -> Result<Option<AgentUsageBudgetScope>> {
        let scope = if let Some(limit) = positive_token_budget(requested_budget) {
            Some((worker_id.to_string(), limit))
        } else if let Some(parent_scope) = self.inherited_budget_scope(parent_run_id) {
            Some(parent_scope)
        } else {
            self.default_token_budget
                .map(|limit| (worker_id.to_string(), limit))
        };

        let Some((scope_id, limit)) = scope else {
            return Ok(None);
        };
        let spent = self.aggregate_budget_spent(&scope_id);
        let remaining = limit.saturating_sub(spent);
        if remaining < MIN_SUBAGENT_SPAWN_TOKEN_RESERVE {
            return Err(anyhow!(
                "Sub-agent token budget exhausted for scope {scope_id}: {spent}/{limit} tokens spent, {remaining} remaining. Wait for the parent/Workflow to summarize results or start a new agent run with an explicit token_budget override."
            ));
        }
        Ok(Some(AgentUsageBudgetScope {
            scope_id,
            limit,
            spent,
            remaining,
        }))
    }

    fn attach_budget_scope(&mut self, worker_id: &str, scope: AgentUsageBudgetScope) {
        let Some(record) = self.worker_records.get_mut(worker_id) else {
            return;
        };
        record.usage.token_budget = Some(scope.limit);
        record.usage.budget_scope = Some(scope.scope_id.clone());
        record.usage.budget_spent_tokens = Some(scope.spent);
        record.usage.budget_remaining_tokens = Some(scope.remaining);
        refresh_usage_note(&mut record.usage);
        self.refresh_budget_scope(&scope.scope_id);
    }

    /// 聚合共享工作流预算范围的 token 支出。
    pub(crate) fn budget_spent_for_scope(&self, scope_id: &str) -> u64 {
        self.aggregate_budget_spent(scope_id)
    }

    /// 将工作流子级附加到运行级别共享预算池。
    pub(crate) fn attach_shared_budget_scope(
        &mut self,
        worker_id: &str,
        scope_id: &str,
        limit: u64,
    ) {
        let spent = self.aggregate_budget_spent(scope_id);
        self.attach_budget_scope(
            worker_id,
            AgentUsageBudgetScope {
                scope_id: scope_id.to_string(),
                limit,
                spent,
                remaining: limit.saturating_sub(spent),
            },
        );
    }

    fn refresh_budget_scope(&mut self, scope_id: &str) {
        let Some(limit) = self
            .worker_records
            .values()
            .find(|record| record.usage.budget_scope.as_deref() == Some(scope_id))
            .and_then(|record| record.usage.token_budget)
        else {
            return;
        };
        let spent = self.aggregate_budget_spent(scope_id);
        let remaining = limit.saturating_sub(spent);
        for record in self.worker_records.values_mut() {
            if record.usage.budget_scope.as_deref() == Some(scope_id) {
                record.usage.token_budget = Some(limit);
                record.usage.budget_spent_tokens = Some(spent);
                record.usage.budget_remaining_tokens = Some(remaining);
                refresh_usage_note(&mut record.usage);
            }
        }
    }

    fn refresh_all_budget_scopes(&mut self) {
        let scope_ids = self
            .worker_records
            .values()
            .filter_map(|record| record.usage.budget_scope.clone())
            .collect::<std::collections::HashSet<_>>();
        for scope_id in scope_ids {
            self.refresh_budget_scope(&scope_id);
        }
    }

    fn record_worker_usage(&mut self, worker_id: &str, usage: &Usage) {
        let now_ms = epoch_millis_now();
        let total_delta = usage_total_tokens(usage);
        let Some(record) = self.worker_records.get_mut(worker_id) else {
            return;
        };
        record.updated_at_ms = now_ms;
        record.usage.input_tokens = Some(
            record
                .usage
                .input_tokens
                .unwrap_or(0)
                .saturating_add(u64::from(usage.input_tokens)),
        );
        record.usage.output_tokens = Some(
            record
                .usage
                .output_tokens
                .unwrap_or(0)
                .saturating_add(u64::from(usage.output_tokens)),
        );
        record.usage.total_tokens = Some(
            record
                .usage
                .total_tokens
                .unwrap_or(0)
                .saturating_add(total_delta),
        );
        let scope_id = record.usage.budget_scope.clone();
        refresh_usage_note(&mut record.usage);
        if let Some(scope_id) = scope_id {
            self.refresh_budget_scope(&scope_id);
        }
        self.persist_state_debounced();
    }

    fn push_worker_event(
        &mut self,
        record: &mut AgentWorkerRecord,
        status: AgentWorkerStatus,
        message: Option<String>,
        step: Option<u32>,
        tool_name: Option<String>,
        now_ms: u64,
    ) {
        self.worker_event_seq = self.worker_event_seq.saturating_add(1);
        record.events.push_back(AgentWorkerEvent {
            seq: self.worker_event_seq,
            worker_id: record.spec.worker_id.clone(),
            status,
            timestamp_ms: now_ms,
            message,
            step,
            tool_name,
        });
        while record.events.len() > MAX_AGENT_WORKER_EVENTS_PER_RECORD {
            record.events.pop_front();
        }
    }

    fn record_worker_event(
        &mut self,
        worker_id: &str,
        status: AgentWorkerStatus,
        message: Option<String>,
        step: Option<u32>,
        tool_name: Option<String>,
    ) {
        let now_ms = epoch_millis_now();
        let Some(mut record) = self.worker_records.remove(worker_id) else {
            return;
        };
        record.status = status;
        record.recommended_action = recommended_action_for_worker_status(status, &record.spec);
        record.updated_at_ms = now_ms;
        record.latest_message = message.clone();
        if matches!(
            status,
            AgentWorkerStatus::Starting | AgentWorkerStatus::Running
        ) && record.started_at_ms.is_none()
        {
            record.started_at_ms = Some(now_ms);
        }
        if matches!(
            status,
            AgentWorkerStatus::Completed
                | AgentWorkerStatus::Failed
                | AgentWorkerStatus::Cancelled
                | AgentWorkerStatus::Interrupted
        ) {
            record.completed_at_ms = Some(now_ms);
        }
        if let Some(step) = step {
            record.steps_taken = step;
        }
        self.push_worker_event(&mut record, status, message, step, tool_name, now_ms);
        self.worker_records.insert(worker_id.to_string(), record);
    }

    fn record_worker_progress(&mut self, worker_id: &str, message: String) {
        let (status, step, tool_name) = worker_progress_event_parts(&message);
        self.record_worker_event(worker_id, status, Some(message), step, tool_name);
    }

    fn complete_worker_from_result(&mut self, worker_id: &str, result: &SubAgentResult) {
        let status = worker_status_from_subagent_result(result);
        let message = match &result.status {
            SubAgentStatus::Completed => Some("completed".to_string()),
            SubAgentStatus::Failed(err) => Some(err.clone()),
            SubAgentStatus::Interrupted(reason) => Some(reason.clone()),
            SubAgentStatus::Cancelled => Some("cancelled".to_string()),
            SubAgentStatus::BudgetExhausted => Some("token budget exhausted".to_string()),
            SubAgentStatus::Running => Some("running".to_string()),
        };
        self.record_worker_event(worker_id, status, message, Some(result.steps_taken), None);
        if let Some(record) = self.worker_records.get_mut(worker_id) {
            record.result_summary = result.result.clone();
            record.steps_taken = result.steps_taken;
            if let SubAgentStatus::Failed(err) = &result.status {
                record.error = Some(err.clone());
            }
        }
    }

    fn fail_worker(&mut self, worker_id: &str, error: String) {
        self.record_worker_event(
            worker_id,
            AgentWorkerStatus::Failed,
            Some(error.clone()),
            None,
            None,
        );
        if let Some(record) = self.worker_records.get_mut(worker_id) {
            record.error = Some(error);
        }
    }

    pub fn cancel_agent(&mut self, agent_ref: &str) -> Result<SubAgentResult> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        let snapshot = {
            let agent = self
                .agents
                .get_mut(&agent_id)
                .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
            if agent.status != SubAgentStatus::Running {
                return Ok(agent.snapshot());
            }
            agent.status = SubAgentStatus::Cancelled;
            agent.result = Some("Cancelled by parent request.".to_string());
            release_resident_leases_for(&agent.id);
            if let Some(handle) = agent.task_handle.take() {
                handle.abort();
            }
            agent.input_tx = None;
            agent.snapshot()
        };
        self.record_worker_event(
            &agent_id,
            AgentWorkerStatus::Cancelled,
            snapshot.result.clone(),
            Some(snapshot.steps_taken),
            None,
        );
        self.persist_state_best_effort();
        Ok(snapshot)
    }

    /// 计数正在运行的代理。
    pub fn running_count(&self) -> usize {
        self.admitted_count()
    }

    /// 计数已被准入的活跃子代理，包括在启动门控上等待的排队工作者。
    pub fn admitted_count(&self) -> usize {
        self.agents
            .values()
            .filter(|agent| {
                // 排除非运行状态
                if agent.status != SubAgentStatus::Running {
                    return false;
                }
                // 排除没有 task_handle 的持久化代理（它们实际上并未运行）
                if agent.task_handle.is_none() {
                    return false;
                }
                // 保持最近完成的句柄被计数，直到终止状态更新已协调。
                // 否则扇出爆发可能会在 UI/状态跟上之前重新填满上限（#2211）。
                !self.running_heartbeat_timed_out(agent)
            })
            .count()
    }

    /// 计数当前正在等待启动门控的已准入工作者。
    pub fn queued_count(&self) -> usize {
        self.agents
            .values()
            .filter(|agent| {
                agent.status == SubAgentStatus::Running
                    && agent.task_handle.is_some()
                    && !self.running_heartbeat_timed_out(agent)
                    && self
                        .worker_records
                        .get(&agent.id)
                        .is_some_and(|record| record.status == AgentWorkerStatus::Queued)
            })
            .count()
    }

    /// 计数当前不在排队启动状态的已准入工作者。
    pub fn active_count(&self) -> usize {
        self.admitted_count().saturating_sub(self.queued_count())
    }

    fn check_admission_capacity(&self) -> Result<()> {
        let admitted = self.admitted_count();
        if admitted >= self.max_admitted_agents {
            return Err(anyhow!(
                "Sub-agent admission limit reached (max_admitted {}, admitted {}, running {}, queued {}). Wait for queued/running agents to finish, cancel unneeded agents, or raise [subagents] max_admitted for this Workflow.",
                self.max_admitted_agents,
                admitted,
                self.active_count(),
                self.queued_count()
            ));
        }
        Ok(())
    }

    fn running_heartbeat_timed_out(&self, agent: &SubAgent) -> bool {
        agent.status == SubAgentStatus::Running
            && agent.task_handle.is_some()
            && agent.last_activity_at.elapsed() >= self.running_heartbeat_timeout
    }

    pub fn touch(&mut self, agent_id: &str) -> bool {
        let Some(agent) = self.agents.get_mut(agent_id) else {
            return false;
        };
        if agent.status != SubAgentStatus::Running {
            return false;
        }
        agent.last_activity_at = Instant::now();
        true
    }

    /// 生成一个新的后台子代理。
    pub fn spawn_background(
        &mut self,
        manager_handle: SharedSubAgentManager,
        runtime: SubAgentRuntime,
        agent_type: SubAgentType,
        prompt: String,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<SubAgentResult> {
        self.spawn_background_with_assignment(
            manager_handle,
            runtime,
            agent_type,
            prompt.clone(),
            SubAgentAssignment::new(prompt, None),
            allowed_tools,
        )
    }

    /// 使用显式的分配元数据生成一个新的后台子代理。
    pub fn spawn_background_with_assignment(
        &mut self,
        manager_handle: SharedSubAgentManager,
        runtime: SubAgentRuntime,
        agent_type: SubAgentType,
        prompt: String,
        assignment: SubAgentAssignment,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<SubAgentResult> {
        self.spawn_background_with_assignment_options(
            manager_handle,
            runtime,
            agent_type,
            prompt,
            assignment,
            allowed_tools,
            SubAgentSpawnOptions::default(),
        )
    }

    /// 使用显式的分配和显示元数据生成一个新的后台子代理。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_background_with_assignment_options(
        &mut self,
        manager_handle: SharedSubAgentManager,
        mut runtime: SubAgentRuntime,
        agent_type: SubAgentType,
        prompt: String,
        assignment: SubAgentAssignment,
        allowed_tools: Option<Vec<String>>,
        options: SubAgentSpawnOptions,
    ) -> Result<SubAgentResult> {
        self.cleanup(COMPLETED_AGENT_RETENTION);

        self.check_admission_capacity()?;

        if let Some(model) = options.model.as_deref() {
            runtime.model = model.to_string();
        }
        let effective_model = runtime.model.clone();
        let agent_id = format!("agent_{}", &Uuid::new_v4().to_string()[..8]);
        let budget_scope = self.resolve_spawn_budget_scope(
            &agent_id,
            runtime.parent_agent_id.as_deref(),
            options.token_budget,
        )?;
        let active_names: std::collections::HashSet<String> = self
            .agents
            .values()
            .filter_map(|a| a.nickname.clone())
            .collect();
        let nickname = options
            .nickname
            .or_else(|| Some(assign_unique_whale_name(&agent_id, &active_names)));
        let tools = build_allowed_tools(&agent_type, allowed_tools, runtime.allow_shell)?;
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let mut agent = SubAgent::new(
            agent_id.clone(),
            agent_type.clone(),
            prompt.clone(),
            assignment.clone(),
            effective_model,
            nickname,
            tools.clone(),
            input_tx,
            runtime.context.workspace.clone(),
            self.current_session_boot_id.clone(),
        );
        if let Some(name) = options
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            if let Some(existing) = self
                .agents
                .values()
                .find(|existing| existing.session_name == name)
            {
                // #3020: 包含经过时间，以便父级可以区分活跃工作者与陈旧/失败的早期生成（#2656）。
                let elapsed = existing.started_at.elapsed();
                let since = if elapsed.as_secs() < 120 {
                    format!("{}s ago", elapsed.as_secs())
                } else {
                    let mins = elapsed.as_secs() / 60;
                    let secs = elapsed.as_secs() % 60;
                    format!("{mins}m{secs}s ago")
                };
                return Err(anyhow!(
                    "Sub-agent session name '{name}' is already in use by agent_id '{}' \
                     (status: {}, started {since}). \
                     Wait for its completion event, or open a new agent with a different name.",
                    existing.id,
                    subagent_status_name(&existing.status)
                ));
            }
            agent.session_name = name.to_string();
        }
        agent.fork_context = options.fork_context;
        let agent_id = agent.id.clone();
        let started_at = agent.started_at;
        let max_steps = self.max_steps;
        let tool_profile = match tools.clone() {
            Some(tools) => AgentWorkerToolProfile::Explicit(tools),
            None => AgentWorkerToolProfile::Inherited,
        };
        let runtime_profile = worker_profile_for_spawn(
            &runtime,
            &agent_type,
            &tool_profile,
            &agent.model,
            options.model_route.clone(),
        );
        runtime.worker_profile = runtime_profile.clone();
        let worker_spec = AgentWorkerSpec {
            worker_id: agent_id.clone(),
            run_id: agent_id.clone(),
            parent_run_id: runtime.parent_agent_id.clone(),
            session_name: Some(agent.session_name.clone()),
            objective: assignment.objective.clone(),
            role: assignment.role.clone(),
            agent_type: agent_type.clone(),
            model: agent.model.clone(),
            workspace: agent.workspace.clone(),
            git_branch: current_git_branch(&agent.workspace),
            context_mode: if options.fork_context {
                "forked"
            } else {
                "fresh"
            }
            .to_string(),
            fork_context: options.fork_context,
            tool_profile,
            runtime_profile,
            max_steps,
            spawn_depth: runtime.spawn_depth,
            max_spawn_depth: runtime.max_spawn_depth,
        };
        self.register_worker(worker_spec);
        if let Some(scope) = budget_scope {
            self.attach_budget_scope(&agent_id, scope);
        }

        if let Some(mb) = runtime.mailbox.as_ref() {
            let _ = mb.send(MailboxMessage::started(&agent_id, agent_type.clone()));
        }

        if let Some(event_tx) = runtime.event_tx.clone() {
            let _ = event_tx.try_send(Event::AgentSpawned {
                id: agent_id.clone(),
                prompt: prompt.clone(),
                parent_run_id: runtime.parent_agent_id.clone(),
                spawn_depth: runtime.spawn_depth,
            });
        }

        let launch_gate = (runtime.spawn_depth == 1).then(|| self.launch_gate.clone());
        let task = SubAgentTask {
            manager_handle,
            runtime,
            agent_id: agent_id.clone(),
            agent_type,
            prompt,
            assignment,
            allowed_tools: tools,
            fork_context: options.fork_context,
            started_at,
            max_steps,
            token_budget: options.token_budget,
            input_rx,
            launch_gate,
        };
        let handle = spawn_supervised(
            "subagent-task",
            std::panic::Location::caller(),
            run_subagent_task(task),
        );
        agent.task_handle = Some(handle);
        self.agents.insert(agent_id.clone(), agent);
        self.persist_state_best_effort();

        Ok(self
            .agents
            .get(&agent_id)
            .expect("agent should exist after spawn")
            .snapshot())
    }

    /// 获取代理的当前快照。
    pub fn get_result(&self, agent_id: &str) -> Result<SubAgentResult> {
        let agent = self
            .agents
            .get(agent_id)
            .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
        Ok(agent.snapshot())
    }

    pub fn get_result_by_ref(&self, agent_ref: &str) -> Result<SubAgentResult> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        self.get_result(&agent_id)
    }

    pub fn terminal_results_excluding(
        &self,
        delivered_ids: &std::collections::HashSet<String>,
    ) -> Vec<SubAgentResult> {
        let mut results = self
            .agents
            .values()
            .filter(|agent| agent.status != SubAgentStatus::Running)
            .filter(|agent| agent.session_boot_id == self.current_session_boot_id)
            .filter(|agent| {
                self.worker_records
                    .get(&agent.id)
                    .is_none_or(|record| record.spec.parent_run_id.is_none())
            })
            .filter(|agent| !delivered_ids.contains(&agent.id))
            .map(SubAgent::snapshot)
            .collect::<Vec<_>>();
        results.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        results
    }

    /// 解析持久化的代理 id 或面向模型的会话名称。
    fn resolve_agent_ref(&self, agent_ref: &str) -> Result<String> {
        let agent_ref = agent_ref.trim();
        if self.agents.contains_key(agent_ref) {
            return Ok(agent_ref.to_string());
        }

        let matches = self
            .agents
            .values()
            .filter(|agent| agent.session_name == agent_ref)
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();

        match matches.as_slice() {
            [id] => Ok(id.clone()),
            [] => Err(anyhow!("Agent session {agent_ref} not found")),
            _ => Err(anyhow!(
                "Agent session name '{agent_ref}' is ambiguous; use an agent_id"
            )),
        }
    }

    /// 列出所有代理及其状态。
    #[must_use]
    /// 快照单个代理并用管理器的分类标记它。裸的 `SubAgent::snapshot`
    /// 将 `from_prior_session` 默认为 `false`；只有管理器知道匹配的启动 id，
    /// 因此列表通过此方法处理。
    fn snapshot_for_listing(&self, agent: &SubAgent) -> SubAgentResult {
        let mut snap = agent.snapshot();
        snap.from_prior_session = self.is_from_prior_session(agent);
        if let Some(record) = self.worker_records.get(&agent.id) {
            snap.worker_status = Some(record.status);
            snap.parent_run_id = record
                .parent_run_id
                .clone()
                .or_else(|| record.spec.parent_run_id.clone());
            snap.spawn_depth = record.spec.spawn_depth;
        }
        snap
    }

    /// 列出管理器当前持有的所有代理，无论会话来源如何。
    /// 在面向用户的工具路径中使用 [`Self::list_filtered`]，
    /// 以便先前会话的代理默认保持隐藏（#405）。
    pub fn list(&self) -> Vec<SubAgentResult> {
        self.agents
            .values()
            .map(|agent| self.snapshot_for_listing(agent))
            .collect()
    }

    /// 列表代理时遵守会话边界过滤器（#405）。
    ///
    /// `include_archived = false` 丢弃任何不再运行的先前会话代理。
    /// 仍然为 `Running` 的先前会话代理（例如，被进程重启中断的）保持可见——
    /// 它们可能对正在进行的恢复很重要。
    ///
    /// `include_archived = true` 返回所有内容，每个 `SubAgentResult` 上带有
    /// `from_prior_session` 标志，以便模型可以一眼区分活跃和归档代理。
    pub fn list_filtered(&self, include_archived: bool) -> Vec<SubAgentResult> {
        self.agents
            .values()
            .filter(|agent| {
                if include_archived {
                    return true;
                }
                if agent.status == SubAgentStatus::Running {
                    return true;
                }
                !self.is_from_prior_session(agent)
            })
            .map(|agent| self.snapshot_for_listing(agent))
            .collect()
    }

    /// 清理过时的正在运行的代理和超过给定持续时间的已完成代理。
    /// 返回在此过程中自动取消的正在运行的代理数量。
    pub fn cleanup(&mut self, max_age: Duration) -> usize {
        let before = self.agents.len();
        let before_workers = self.worker_records.len();
        let mut auto_cancelled = 0;
        let timeout = self.running_heartbeat_timeout;
        let mut worker_cancellations = Vec::new();
        for agent in self.agents.values_mut() {
            if agent.status == SubAgentStatus::Running
                && agent.task_handle.is_some()
                && agent.last_activity_at.elapsed() >= timeout
            {
                tracing::warn!(
                    target: "subagent",
                    agent_id = %agent.id,
                    timeout_secs = timeout.as_secs(),
                    "auto-cancelling stale sub-agent with no manager-visible progress"
                );
                agent.status = SubAgentStatus::Cancelled;
                agent.result = Some(format!(
                    "Auto-cancelled after {}s without sub-agent progress.",
                    timeout.as_secs()
                ));
                release_resident_leases_for(&agent.id);
                if let Some(handle) = agent.task_handle.take() {
                    handle.abort();
                }
                agent.input_tx = None;
                worker_cancellations.push((
                    agent.id.clone(),
                    agent.result.clone(),
                    agent.steps_taken,
                ));
                auto_cancelled += 1;
            }
        }
        for (agent_id, message, steps_taken) in worker_cancellations {
            self.record_worker_event(
                &agent_id,
                AgentWorkerStatus::Cancelled,
                message,
                Some(steps_taken),
                None,
            );
        }
        self.agents.retain(|_, agent| {
            if agent.status == SubAgentStatus::Running {
                true
            } else {
                agent.started_at.elapsed() < max_age
            }
        });
        // #4217: 按时间逐出终止工作者的分类账条目。代理已经在 `max_age` 后被丢弃，
        // 但 worker_records 之前只有 256 的 LRU 上限——长期会话会永远重写多 MB 的 subagents.v1.json。
        // 运行中/启动中/等待中的记录始终保留。
        let now_ms = epoch_millis_now();
        let max_age_ms = max_age.as_millis() as u64;
        self.worker_records.retain(|_, record| {
            if !record.status.is_terminal() {
                return true;
            }
            let anchor_ms = record.completed_at_ms.unwrap_or(record.updated_at_ms);
            now_ms.saturating_sub(anchor_ms) < max_age_ms
        });
        if self.agents.len() != before
            || auto_cancelled > 0
            || self.worker_records.len() != before_workers
        {
            self.persist_state_best_effort();
        }
        self.last_cleanup_at = Some(Instant::now());
        auto_cancelled
    }

    /// #3803: 自上次 `cleanup` 以来是否已经过足够时间，以至于下一次侧边栏刷新应再次运行写锁定的清理。
    /// 每隔一次刷新从只读的 `list()` 快照渲染，因此扇出期间的 UI 刷新风暴不会每次请求都获取写锁。
    #[must_use]
    pub fn cleanup_due(&self, min_interval: Duration) -> bool {
        self.last_cleanup_at
            .is_none_or(|last| last.elapsed() >= min_interval)
    }

    fn update_from_result(&mut self, agent_id: &str, result: SubAgentResult) {
        let mut changed = false;
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.status = result.status.clone();
            agent.assignment = result.assignment.clone();
            agent.result = result.result.clone();
            agent.steps_taken = result.steps_taken;
            agent.checkpoint = result.checkpoint.clone();
            agent.needs_input = result.needs_input.clone();
            if result.status != SubAgentStatus::Running {
                agent.input_tx = None;
            }
            agent.task_handle = None;
            changed = true;
        }
        self.complete_worker_from_result(agent_id, &result);
        if changed {
            self.persist_state_best_effort();
        }
    }

    fn update_failed(&mut self, agent_id: &str, error: String) {
        let mut changed = false;
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.status = SubAgentStatus::Failed(error.clone());
            release_resident_leases_for(agent_id);
            agent.input_tx = None;
            agent.task_handle = None;
            changed = true;
        }
        self.fail_worker(agent_id, error);
        if changed {
            self.persist_state_best_effort();
        }
    }

    fn update_checkpoint(&mut self, agent_id: &str, checkpoint: SubAgentCheckpoint) -> bool {
        let Some(agent) = self.agents.get_mut(agent_id) else {
            return false;
        };
        agent.steps_taken = checkpoint.steps_taken;
        agent.checkpoint = Some(checkpoint);
        agent.last_activity_at = Instant::now();
        // #freeze: 热路径的每步路径——合并全舰队持久化，以便 20 个代理同时步进时，
        // 不会在每个步骤的写锁下将整个舰队（含完整转录）序列化到磁盘。
        self.persist_state_debounced();
        true
    }

    fn interrupt_with_checkpoint(
        &mut self,
        agent_id: &str,
        reason: String,
        checkpoint: SubAgentCheckpoint,
        needs_input: Option<SubAgentNeedsInput>,
    ) -> Result<SubAgentResult> {
        let snapshot = {
            let agent = self
                .agents
                .get_mut(agent_id)
                .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
            agent.status = SubAgentStatus::Interrupted(reason.clone());
            agent.result = Some(reason);
            agent.steps_taken = checkpoint.steps_taken;
            agent.checkpoint = Some(checkpoint);
            agent.needs_input = needs_input;
            agent.last_activity_at = Instant::now();
            release_resident_leases_for(agent_id);
            agent.snapshot()
        };
        self.record_worker_event(
            agent_id,
            AgentWorkerStatus::Interrupted,
            snapshot.result.clone(),
            Some(snapshot.steps_taken),
            None,
        );
        self.persist_state_best_effort();
        Ok(snapshot)
    }
}

/// `SubAgentManager` 的线程安全包装器。
pub type SharedSubAgentManager = Arc<RwLock<SubAgentManager>>;

pub fn load_persisted_agent_worker_records(workspace: &Path) -> Result<Vec<AgentWorkerRecord>> {
    let mut manager = SubAgentManager::new(workspace.to_path_buf(), 1)
        .with_state_path(default_state_path(workspace)?);
    manager.load_state()?;
    Ok(manager.list_worker_records())
}

/// v0.8.33 子代理 API 返回的面向模型的会话投影。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentSessionProjection {
    pub name: String,
    pub agent_id: String,
    #[serde(default)]
    pub run_id: String,
    pub status: String,
    pub terminal: bool,
    pub context_mode: String,
    pub fork_context: bool,
    pub prefix_cache: SubAgentPrefixCacheProjection,
    pub transcript_handle: VarHandle,
    #[serde(default = "default_agent_run_follow_up")]
    pub follow_up: AgentRunFollowUpTarget,
    #[serde(default = "default_agent_run_takeover")]
    pub takeover: AgentRunTakeoverTarget,
    #[serde(default)]
    pub artifacts: Vec<AgentRunArtifactRef>,
    #[serde(default = "default_agent_run_usage")]
    pub usage: AgentRunUsage,
    #[serde(default = "default_agent_run_verification")]
    pub verification: AgentRunVerificationSummary,
    pub snapshot: SubAgentResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<SubAgentCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_input: Option<SubAgentNeedsInput>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub continuable: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub needs_continuation: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub timed_out: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub timed_out_with_checkpoint: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_record: Option<AgentWorkerRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentPrefixCacheProjection {
    pub mode: String,
    pub parent_prefix: String,
    pub deepseek_prefix_cache_reuse: String,
}

fn subagent_prefix_cache_projection(snapshot: &SubAgentResult) -> SubAgentPrefixCacheProjection {
    if snapshot.fork_context {
        SubAgentPrefixCacheProjection {
            mode: "forked".to_string(),
            parent_prefix: "preserved_byte_identical_when_available".to_string(),
            deepseek_prefix_cache_reuse: "optimized_for_existing_parent_prefill".to_string(),
        }
    } else {
        SubAgentPrefixCacheProjection {
            mode: "fresh".to_string(),
            parent_prefix: "not_inherited".to_string(),
            deepseek_prefix_cache_reuse: "independent_child_prefill".to_string(),
        }
    }
}

fn subagent_checkpoint_is_continuable(snapshot: &SubAgentResult) -> bool {
    matches!(snapshot.status, SubAgentStatus::Interrupted(_))
        && snapshot
            .checkpoint
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.continuable && !checkpoint.messages.is_empty())
}

async fn subagent_session_projection(
    snapshot: SubAgentResult,
    timed_out: bool,
    context: &ToolContext,
    worker_record: Option<AgentWorkerRecord>,
) -> SubAgentSessionProjection {
    let transcript_session_id = format!("agent:{}", snapshot.agent_id);
    let continuable = subagent_checkpoint_is_continuable(&snapshot);
    let transcript_payload = json!({
        "kind": "subagent_session_snapshot",
        "agent_id": snapshot.agent_id.clone(),
        "name": snapshot.name.clone(),
        "status": subagent_status_name(&snapshot.status),
        "context_mode": snapshot.context_mode.clone(),
        "fork_context": snapshot.fork_context,
        "result": snapshot.result.clone(),
        "steps_taken": snapshot.steps_taken,
        "duration_ms": snapshot.duration_ms,
        "assignment": snapshot.assignment.clone(),
        "checkpoint": snapshot.checkpoint.clone(),
        "needs_input": snapshot.needs_input.clone(),
        "needs_continuation": continuable,
        "timed_out_with_checkpoint": timed_out && continuable,
        "snapshot": snapshot.clone(),
    });
    let transcript_handle = {
        let mut store = context.runtime.handle_store.lock().await;
        let full_transcript_lookup = VarHandle {
            kind: "var_handle".to_string(),
            session_id: transcript_session_id.clone(),
            name: "full_transcript".to_string(),
            type_name: String::new(),
            length: 0,
            repr_preview: String::new(),
            sha256: String::new(),
        };
        if snapshot.status != SubAgentStatus::Running
            && let Some(record) = store.get(&full_transcript_lookup)
        {
            record.handle.clone()
        } else {
            store.insert_json(transcript_session_id, "transcript", transcript_payload)
        }
    };
    let run_id = worker_record
        .as_ref()
        .map(|record| agent_worker_run_id(&record.spec))
        .unwrap_or_else(|| snapshot.agent_id.clone());
    let follow_up = worker_record
        .as_ref()
        .map(|record| record.follow_up.clone())
        .unwrap_or_else(|| AgentRunFollowUpTarget {
            tool: default_agent_inspect_tool(),
            agent_id: snapshot.agent_id.clone(),
            session_name: Some(snapshot.name.clone()),
            accepted_statuses: vec!["running".to_string(), "interrupted_continuable".to_string()],
            latest_delivery: None,
        });
    let takeover = worker_record
        .as_ref()
        .map(|record| record.takeover.clone())
        .unwrap_or_else(|| AgentRunTakeoverTarget {
            kind: default_subagent_takeover_kind(),
            supported: true,
            agent_id: snapshot.agent_id.clone(),
            session_name: Some(snapshot.name.clone()),
            instructions: format!(
                "Inspect agent '{}' through the returned transcript_handle with handle_read; open a replacement with agent if the lane no longer fits.",
                snapshot.agent_id
            ),
            unsupported_reason: None,
        });
    let artifacts = worker_record
        .as_ref()
        .map(|record| record.artifacts.clone())
        .unwrap_or_else(|| default_subagent_artifacts(&run_id));
    let usage = worker_record
        .as_ref()
        .map(|record| record.usage.clone())
        .unwrap_or_else(default_agent_run_usage);
    let verification = worker_record
        .as_ref()
        .map(|record| record.verification.clone())
        .unwrap_or_else(default_agent_run_verification);
    // 状态必须与下面的继续标志保持一致。一个携带可继续检查点的
    // Interrupted 快照（`continuable`/`needs_continuation` 为 true，`terminal` 为 true）
    // 意味着工作者已停放等待父级操作，因此它必须投影为 `waiting_for_user` 而非裸的 `interrupted`。
    // 当工作者记录存在时，其状态已经通过 `worker_status_from_subagent_result` 推导；
    // 当没有记录时镜像该推导，以便两条路径在"需要父级操作"信号上保持一致。
    let status = worker_record
        .as_ref()
        .map(|record| agent_worker_status_name(record.status))
        .unwrap_or_else(|| agent_worker_status_name(worker_status_from_subagent_result(&snapshot)))
        .to_string();

    SubAgentSessionProjection {
        name: snapshot.name.clone(),
        agent_id: snapshot.agent_id.clone(),
        run_id,
        status,
        terminal: snapshot.status != SubAgentStatus::Running,
        context_mode: snapshot.context_mode.clone(),
        fork_context: snapshot.fork_context,
        prefix_cache: subagent_prefix_cache_projection(&snapshot),
        transcript_handle,
        follow_up,
        takeover,
        artifacts,
        usage,
        verification,
        checkpoint: snapshot.checkpoint.clone(),
        needs_input: snapshot.needs_input.clone(),
        continuable: subagent_checkpoint_is_continuable(&snapshot),
        needs_continuation: continuable,
        snapshot,
        timed_out,
        timed_out_with_checkpoint: timed_out && continuable,
        worker_record,
    }
}

fn default_state_path(workspace: &Path) -> Result<PathBuf> {
    let workspace = normalize_subagent_workspace(workspace);
    // 品牌重塑后的规范状态路径。首次运行时文件尚不存在；
    // write_json_atomic 创建父目录。遗留的 .deepseek/state/ 数据在加载时迁移（参见 load_state）。
    checked_subagent_state_path(
        &workspace,
        &Path::new(".codewhale")
            .join("state")
            .join(SUBAGENT_STATE_FILE),
    )
}

fn checked_subagent_state_path(workspace: &Path, path: &Path) -> Result<PathBuf> {
    let workspace = normalize_subagent_workspace(workspace);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    let file_name = absolute
        .file_name()
        .ok_or_else(|| anyhow!("sub-agent state path must include a file name"))?;
    let parent = absolute
        .parent()
        .ok_or_else(|| anyhow!("sub-agent state path must include a parent directory"))?;
    let parent = match parent.canonicalize() {
        Ok(parent) => parent,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => normalize_path_components(parent),
        Err(err) => return Err(err.into()),
    };
    let state_path = parent.join(file_name);
    if !state_path.starts_with(&workspace) {
        return Err(anyhow!(
            "sub-agent state path must stay within workspace: {}",
            state_path.display()
        ));
    }
    reject_workspace_relative_symlinks(&workspace, &state_path)?;
    Ok(state_path)
}

fn normalize_subagent_workspace(workspace: &Path) -> PathBuf {
    if let Ok(canonical) = workspace.canonicalize() {
        return canonical;
    }
    let absolute = if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(workspace)
    };
    normalize_path_components(&absolute)
}

fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn reject_workspace_relative_symlinks(workspace: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(workspace).map_err(|_| {
        anyhow!(
            "sub-agent state path must stay within workspace: {}",
            path.display()
        )
    })?;
    let mut current = workspace.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            return Err(anyhow!(
                "sub-agent state path must not traverse symlinks: {}",
                current.display()
            ));
        }
    }
    Ok(())
}

fn read_subagent_state_file(workspace: &Path, path: &Path) -> Result<String> {
    let workspace = normalize_subagent_workspace(workspace);
    reject_workspace_relative_symlinks(&workspace, path)?;
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() || !file_type.is_file() {
        return Err(anyhow!(
            "sub-agent state path must be a regular file: {}",
            path.display()
        ));
    }

    let mut file = open_subagent_state_file(path)?;
    let mut raw = String::new();
    file.read_to_string(&mut raw)?;
    Ok(raw)
}

#[cfg(unix)]
fn open_subagent_state_file(path: &Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(Into::into)
}

#[cfg(not(unix))]
fn open_subagent_state_file(path: &Path) -> Result<fs::File> {
    fs::File::open(path).map_err(Into::into)
}

fn epoch_millis_now() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        Err(_) => 0,
    }
}

fn instant_from_duration(duration: Duration) -> Instant {
    Instant::now()
        .checked_sub(duration)
        .unwrap_or_else(Instant::now)
}

/// 每次写入的序列号，以便每个 `write_json_atomic` 使用不同的临时文件。
/// `persist_state_best_effort` 每次调用启动一个新线程，因此同一 `state.json` 的多个持久化
/// 可能同时进行；仅以 pid 作为临时文件名（像以前一样）会导致每个线程写入*同一个*
/// `state.<pid>.tmp`，并且重命名可能发布一个半写入的文件——损坏的状态在重新加载时无法解析。
static WRITE_JSON_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn write_json_atomic<T: Serialize>(workspace: &Path, path: &Path, value: &T) -> Result<()> {
    let workspace = normalize_subagent_workspace(workspace);
    reject_workspace_relative_symlinks(&workspace, path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(value)?;
    let seq = WRITE_JSON_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_path = path.with_extension(format!("{}.{seq}.tmp", std::process::id()));
    reject_workspace_relative_symlinks(&workspace, &tmp_path)?;
    fs::write(&tmp_path, payload)?;
    if let Err(err) = fs::rename(&tmp_path, path) {
        // 如果发布失败，不要留下残留的临时文件。
        let _ = fs::remove_file(&tmp_path);
        return Err(err.into());
    }
    Ok(())
}

/// 创建一个具有可配置限制的共享子代理管理器。
#[cfg(test)]
#[must_use]
pub fn new_shared_subagent_manager(workspace: PathBuf, max_agents: usize) -> SharedSubAgentManager {
    new_shared_subagent_manager_with_timeout(
        workspace,
        max_agents,
        max_agents,
        Duration::from_secs(crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS),
        max_agents,
        None,
    )
}

/// 创建一个具有可配置并发度和陈旧运行代理心跳超时的共享子代理管理器。
#[must_use]
pub fn new_shared_subagent_manager_with_timeout(
    workspace: PathBuf,
    max_agents: usize,
    max_admitted_agents: usize,
    running_heartbeat_timeout: Duration,
    launch_concurrency: usize,
    default_token_budget: Option<u64>,
) -> SharedSubAgentManager {
    let max_agents = max_agents.clamp(1, MAX_SUBAGENTS);
    let state_path = match default_state_path(&workspace) {
        Ok(path) => Some(path),
        Err(err) => {
            tracing::warn!(target: "subagent", ?err, "failed to resolve sub-agent state path");
            None
        }
    };
    let mut manager = SubAgentManager::new(workspace, max_agents)
        .with_admission_limit(max_admitted_agents)
        .with_running_heartbeat_timeout(running_heartbeat_timeout)
        .with_launch_concurrency(launch_concurrency)
        .with_default_token_budget(default_token_budget);
    if let Some(state_path) = state_path {
        manager = manager.with_state_path(state_path);
    }
    if let Err(err) = manager.load_state() {
        // 通过 tracing 而非 stderr 路由——参见上面 `persist_state_best_effort` 中的注释。
        tracing::warn!(target: "subagent", ?err, "failed to load sub-agent state");
    }
    Arc::new(RwLock::new(manager))
}

// === 工具实现 ===

/// 通过一个简化的面向模型接口启动子代理任务。
pub struct AgentTool {
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
    /// 每个代理的上次投影指纹，用于限制观察到无变化的重复 peek/status 调用（#4097）。
    /// 标准互斥锁：仅用于短暂的 map 读取/写入，从不在 await 期间持有。
    inspect_memo: Arc<std::sync::Mutex<HashMap<String, PeekMemo>>>,
}

/// 一个代理的上次 peek/status 响应的指纹（#4097）。
#[derive(Debug, Clone, Copy)]
struct PeekMemo {
    fingerprint: u64,
    at: Instant,
}

impl AgentTool {
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, runtime: SubAgentRuntime) -> Self {
        Self {
            manager,
            runtime,
            inspect_memo: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentToolAction {
    Start,
    Status,
    Peek,
    Wait,
    Cancel,
}

fn parse_agent_tool_action(input: &Value) -> Result<AgentToolAction, ToolError> {
    let Some(action) = optional_input_str(input, &["action", "op"]) else {
        return Ok(AgentToolAction::Start);
    };
    match action.trim().to_ascii_lowercase().as_str() {
        "" | "start" | "spawn" | "run" => Ok(AgentToolAction::Start),
        "status" | "list" | "inspect" => Ok(AgentToolAction::Status),
        "peek" | "progress" => Ok(AgentToolAction::Peek),
        "wait" | "join" | "await" | "block" => Ok(AgentToolAction::Wait),
        "cancel" | "stop" | "abort" => Ok(AgentToolAction::Cancel),
        other => Err(ToolError::invalid_input(format!(
            "Invalid agent action '{other}'. Use start, status, peek, wait, or cancel."
        ))),
    }
}

fn parse_agent_ref(input: &Value) -> Option<String> {
    optional_input_str(input, &["agent_id", "id", "session_name", "name"])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[async_trait]
impl ToolSpec for AgentTool {
    fn name(&self) -> &'static str {
        "agent"
    }

    fn description(&self) -> &'static str {
        concat!(
            "Start, inspect, peek at, or cancel focused child agent tasks through one surface. Use start only for independent work that benefits from a clean context. ",
            "For several independent targets, call agent separately for each target; CodeWhale runs or queues them under runtime capacity and provider rate-limit backpressure. ",
            "The child runs in the background and reports back automatically when finished; keep tiny reads/searches local. ",
            "Pass profile to spawn a saved Fleet roster member (e.g. reviewer, scout, builder) with its role posture, model routing, and instructions. ",
            "Use action=status or action=peek with agent_id to inspect progress, and action=cancel with agent_id to stop a running child. Returns session projections with transcript_handle for UI/debug inspection. ",
            "Never poll with repeated peek/status calls or sleep while children run: results arrive automatically as completion sentinels. If you must block until a child finishes (fan-in before synthesis), make one action=wait call — it blocks until a child settles (all children when agent_id is omitted; timeout_secs caps the block, default 300)."
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["start", "status", "peek", "wait", "cancel"],
                    "description": "start (default) launches a child. status lists current children or inspects agent_id. peek is status for one child. wait blocks until a running child settles (agent_id for one specific child, otherwise the next completion) — use this instead of polling peek/status or sleeping. cancel stops a running child by agent_id."
                },
                "agent_id": {
                    "type": "string",
                    "description": "Agent id or session name for action=status, action=peek, action=wait, or action=cancel."
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 5,
                    "maximum": 1800,
                    "description": "For action=wait: maximum seconds to block before returning a still-running snapshot. Default 300."
                },
                "include_archived": {
                    "type": "boolean",
                    "description": "For action=status without agent_id, include prior-session completed agents."
                },
                "name": {
                    "type": "string",
                    "description": "For action=start, optional stable session name. For status/peek/cancel, accepted as an alias for agent_id."
                },
                "prompt": {
                    "type": "string",
                    "description": "Focused task for the child agent. Prefer a compact Subagent Brief with QUESTION, SCOPE, ALREADY_KNOWN, EFFORT, STOP_CONDITION, and OUTPUT."
                },
                "type": {
                    "type": "string",
                    "description": SUBAGENT_TYPE_DESCRIPTION
                },
                "profile": {
                    "type": "string",
                    "description": "Optional Fleet roster member to run this child as (e.g. reviewer, scout, builder, verifier, synthesizer, manager, or a custom member from .codewhale/agents/ or [fleet.profiles] config). The member supplies role posture, model routing, instruction overlay, and delegation bounds; explicit type/model/model_strength/max_depth here override the member's defaults. See /fleet."
                },
                "model_strength": {
                    "type": "string",
                    "enum": ["same", "faster"],
                    "description": "Optional child model strength. Use same when the child should be as capable as the current model. Use faster for type=explore, read-only lookup/search, status, or other low-risk tasks that can run on a smaller/faster same-family sibling; CodeWhale maps known families such as DeepSeek V4 Pro to Flash and GLM-5.2 to GLM-5-Turbo. type=explore defaults to faster unless you pass model_strength or model explicitly. No hidden auto-downgrade happens."
                },
                "model": {
                    "type": "string",
                    "description": "Optional exact provider model id for the child. Overrides model_strength. Prefer model_strength unless you know the provider-specific id."
                },
                "thinking": {
                    "type": "string",
                    "enum": ["inherit", "auto", "off", "low", "medium", "high", "max"],
                    "description": "Optional child thinking budget. inherit (default) follows the parent thinking mode. auto chooses from the child prompt. off is best for faster explore/lookups. high is for normal reasoning. max is for hard design/debug/release/security work. Explicit thinking overrides the default off used by model_strength=faster."
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional pre-existing working directory for the child; must be inside the parent workspace. Prefer worktree=true for isolated parallel edit tasks."
                },
                "worktree": {
                    "type": "boolean",
                    "description": "When true, create a fresh git worktree and branch for this child before it starts. Use for parallel edit tasks that must not collide with the parent checkout."
                },
                "worktree_branch": {
                    "type": "string",
                    "description": "Optional branch name for worktree=true. Defaults to codex/agent-<name>-<id>."
                },
                "worktree_base": {
                    "type": "string",
                    "description": "Optional git ref to branch the worktree from. Defaults to HEAD in the parent checkout."
                },
                "worktree_path": {
                    "type": "string",
                    "description": "Optional worktree checkout path. Relative paths are created under the default sibling .codewhale-worktrees directory, not inside the parent checkout."
                },
                "fork_context": {
                    "type": "boolean",
                    "description": "false (default): fresh child context. true: include the current parent context prefix when the child needs shared context or a byte-identical parent prefix for DeepSeek prefix-cache reuse."
                },
                "max_depth": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 3,
                    "description": "Optional remaining nested-agent depth budget for this child. Defaults to the configured runtime budget."
                },
                "token_budget": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional aggregate token budget for this child and descendants. When unset, the child inherits the parent budget pool or the configured root default."
                }
            },
            "required": []
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::ExecutesCode,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    /// #3801: status 和 peek 是只读查询——无需批准。
    /// #4097: wait 被动观察子级——也是只读的。
    fn approval_requirement_for(&self, input: &Value) -> ApprovalRequirement {
        match parse_agent_tool_action(input) {
            Ok(AgentToolAction::Status | AgentToolAction::Peek | AgentToolAction::Wait) => {
                ApprovalRequirement::Auto
            }
            _ => ApprovalRequirement::Required,
        }
    }

    /// #3801: `action=start` 启动一个后台代理并立即返回——
    /// 这是一个分离的启动，不应在子级启动时持有全局工具执行写锁。
    /// 在自动批准模式（YOLO）下，这允许多个独立的 `agent start` 调用加入一个并行批次，
    /// 而不是 N 路串行化。
    fn starts_detached_for(&self, input: &Value) -> bool {
        matches!(parse_agent_tool_action(input), Ok(AgentToolAction::Start))
    }

    /// #3801: 只读的 `agent` 操作（status, peek）可以安全地并行运行。
    fn supports_parallel_for(&self, input: &Value) -> bool {
        matches!(
            parse_agent_tool_action(input),
            Ok(AgentToolAction::Status) | Ok(AgentToolAction::Peek)
        )
    }

    /// #3801: status/peek 操作是管理器状态的只读查询。
    /// #4097: wait 仅观察子级生命周期——也是只读的。
    fn is_read_only_for(&self, input: &Value) -> bool {
        matches!(
            parse_agent_tool_action(input),
            Ok(AgentToolAction::Status | AgentToolAction::Peek | AgentToolAction::Wait)
        )
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let action = parse_agent_tool_action(&input)?;
        match action {
            AgentToolAction::Start => {}
            AgentToolAction::Status | AgentToolAction::Peek => {
                return inspect_agent_from_input(
                    &input,
                    self.manager.clone(),
                    context,
                    matches!(action, AgentToolAction::Peek),
                    Some(&self.inspect_memo),
                )
                .await;
            }
            AgentToolAction::Wait => {
                return wait_for_subagents_from_input(&input, self.manager.clone(), context).await;
            }
            AgentToolAction::Cancel => {
                return cancel_agent_from_input(&input, self.manager.clone(), context).await;
            }
        }
        let (snapshot, spawn_policy_note, _) =
            spawn_subagent_from_input(input, self.manager.clone(), self.runtime.clone()).await?;
        let worker_record = {
            let manager = self.manager.read().await;
            manager.get_worker_record(&snapshot.agent_id)
        };
        let projection = subagent_session_projection(snapshot, false, context, worker_record).await;
        let mut tool_result = ToolResult::json(&projection)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        let mut metadata = json!({
            "status": projection.status,
            "terminal": projection.terminal,
            "context_mode": projection.context_mode,
            "prefix_cache": projection.prefix_cache,
        });
        if let Some(note) = spawn_policy_note {
            metadata["spawn_policy"] = json!(note);
        }
        tool_result.metadata = Some(metadata);
        Ok(tool_result)
    }
}

/// 在此窗口内对未变化的运行中子级重复进行 peek/status 调用时，
/// 返回一个紧凑的"无变化"提示，而非完整的投影（#4097）。
const PEEK_UNCHANGED_THROTTLE_WINDOW: Duration = Duration::from_secs(30);

/// 运行中子级的模型可见状态的稳定变化指纹。
/// 易变字段（持续时间、时间戳）被有意排除，以便空闲子级在连续 peek 中指纹相同。
fn inspect_fingerprint(snapshot: &SubAgentResult) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    subagent_status_name(&snapshot.status).hash(&mut hasher);
    snapshot.steps_taken.hash(&mut hasher);
    snapshot.result.is_some().hash(&mut hasher);
    snapshot.needs_input.is_some().hash(&mut hasher);
    snapshot.checkpoint.is_some().hash(&mut hasher);
    hasher.finish()
}

async fn inspect_agent_from_input(
    input: &Value,
    manager: SharedSubAgentManager,
    context: &ToolContext,
    peek: bool,
    inspect_memo: Option<&Arc<std::sync::Mutex<HashMap<String, PeekMemo>>>>,
) -> Result<ToolResult, ToolError> {
    let include_archived =
        parse_optional_bool(input, &["include_archived", "includeArchived"]).unwrap_or(false);

    if let Some(agent_ref) = parse_agent_ref(input) {
        let (snapshot, worker_record) = {
            let mut manager = manager.write().await;
            manager.cleanup(COMPLETED_AGENT_RETENTION);
            let snapshot = manager
                .get_result_by_ref(&agent_ref)
                .map_err(|err| ToolError::invalid_input(err.to_string()))?;
            let worker_record = manager.get_worker_record(&snapshot.agent_id);
            (snapshot, worker_record)
        };

        // #4097: 自上次 peek 以来模型可见状态未变化的运行中子级
        // 会收到一个紧凑提示，而非另一个完整投影。
        // 终止/停放子级始终返回完整信息——模型可能正在合法地获取结果。
        if snapshot.status == SubAgentStatus::Running
            && let Some(memo_map) = inspect_memo
        {
            let fingerprint = inspect_fingerprint(&snapshot);
            let now = Instant::now();
            let unchanged = {
                let mut memo_map = memo_map.lock().expect("inspect memo lock");
                let unchanged = memo_map.get(&snapshot.agent_id).is_some_and(|memo| {
                    memo.fingerprint == fingerprint
                        && now.duration_since(memo.at) < PEEK_UNCHANGED_THROTTLE_WINDOW
                });
                memo_map.insert(
                    snapshot.agent_id.clone(),
                    PeekMemo {
                        fingerprint,
                        at: now,
                    },
                );
                unchanged
            };
            if unchanged {
                let payload = json!({
                    "action": if peek { "peek" } else { "status" },
                    "agent_id": snapshot.agent_id,
                    "name": snapshot.name,
                    "status": "running",
                    "unchanged": true,
                    "hint": "No change since your last check. Do not poll: results arrive automatically as <codewhale:subagent.done> sentinels. Either continue independent work, end your turn, or make one agent(action=\"wait\") call to block until this child settles.",
                });
                let mut tool_result = ToolResult::json(&payload)
                    .map_err(|err| ToolError::execution_failed(err.to_string()))?;
                tool_result.metadata = Some(json!({
                    "action": if peek { "peek" } else { "status" },
                    "status": "running",
                    "terminal": false,
                    "agent_id": payload["agent_id"],
                    "unchanged": true,
                }));
                return Ok(tool_result);
            }
        }

        let projection =
            subagent_session_projection(snapshot, include_archived, context, worker_record).await;
        let mut tool_result = ToolResult::json(&projection)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        tool_result.metadata = Some(json!({
            "action": if peek { "peek" } else { "status" },
            "status": projection.status,
            "terminal": projection.terminal,
            "agent_id": projection.agent_id,
        }));
        return Ok(tool_result);
    }

    let snapshots = {
        let mut manager = manager.write().await;
        manager.cleanup(COMPLETED_AGENT_RETENTION);
        manager
            .list_filtered(include_archived)
            .into_iter()
            .map(|snapshot| {
                let worker_record = manager.get_worker_record(&snapshot.agent_id);
                (snapshot, worker_record)
            })
            .collect::<Vec<_>>()
    };

    let mut projections = Vec::with_capacity(snapshots.len());
    for (snapshot, worker_record) in snapshots {
        projections.push(
            subagent_session_projection(snapshot, include_archived, context, worker_record).await,
        );
    }
    let payload = json!({
        "action": if peek { "peek" } else { "status" },
        "count": projections.len(),
        "agents": projections,
    });
    let mut tool_result =
        ToolResult::json(&payload).map_err(|err| ToolError::execution_failed(err.to_string()))?;
    tool_result.metadata = Some(json!({
        "action": if peek { "peek" } else { "status" },
        "count": payload["count"],
    }));
    Ok(tool_result)
}

async fn cancel_agent_from_input(
    input: &Value,
    manager: SharedSubAgentManager,
    context: &ToolContext,
) -> Result<ToolResult, ToolError> {
    let agent_ref = parse_agent_ref(input).ok_or_else(|| ToolError::missing_field("agent_id"))?;
    let (snapshot, worker_record) = {
        let mut manager = manager.write().await;
        let snapshot = manager
            .cancel_agent(&agent_ref)
            .map_err(|err| ToolError::invalid_input(err.to_string()))?;
        let worker_record = manager.get_worker_record(&snapshot.agent_id);
        (snapshot, worker_record)
    };
    let projection = subagent_session_projection(snapshot, false, context, worker_record).await;
    let mut tool_result = ToolResult::json(&projection)
        .map_err(|err| ToolError::execution_failed(err.to_string()))?;
    tool_result.metadata = Some(json!({
        "action": "cancel",
        "status": projection.status,
        "terminal": projection.terminal,
        "agent_id": projection.agent_id,
    }));
    Ok(tool_result)
}

/// `agent(action="wait")` 的边界（#4097）。默认值使一次 wait 调用
/// 远低于提供商/工具超时，同时覆盖典型的子级运行时间；
/// 到期时模型会得到一个仍在运行的快照，可以再次等待。
const SUBAGENT_WAIT_DEFAULT_TIMEOUT_SECS: u64 = 300;
/// 运行时下限为 1 秒（schema 声明为 5），以便测试无需多秒休眠即可演练超时路径。
const SUBAGENT_WAIT_MIN_TIMEOUT_SECS: u64 = 1;
const SUBAGENT_WAIT_MAX_TIMEOUT_SECS: u64 = 1800;
/// 阻塞时的内部状态检查节奏。对模型不可见——#4097 反模式是模型可见的轮询，
/// 会消耗轮次和 token，而非廉价的进程内定时器。
const SUBAGENT_WAIT_CHECK_INTERVAL: Duration = Duration::from_millis(250);

/// `agent(action="wait")`: 阻塞直到运行中的子级稳定下来（离开 `Running` 状态——
/// 完成、失败、取消、中断/需要输入或预算耗尽），然后返回一个紧凑摘要。
/// 完整的子级结果仍由运行时作为 `<codewhale:subagent.done>` 哨兵传递；
/// 此调用仅提供模型以前通过 peek→sleep 循环伪造的合法"join"（#4097）。
///
/// 提供 `agent_id` 时，专门等待该子级。不提供时，等待下一个子级稳定下来
/// （返回所有在阻塞期间稳定下来的子级）。没有运行中的子级时立即返回。
/// 可安全取消：引擎轮次的取消令牌中断阻塞，且不会在 await 期间持有任何锁。
async fn wait_for_subagents_from_input(
    input: &Value,
    manager: SharedSubAgentManager,
    context: &ToolContext,
) -> Result<ToolResult, ToolError> {
    let timeout_secs = input
        .get("timeout_secs")
        .or_else(|| input.get("timeout"))
        .and_then(Value::as_u64)
        .unwrap_or(SUBAGENT_WAIT_DEFAULT_TIMEOUT_SECS)
        .clamp(
            SUBAGENT_WAIT_MIN_TIMEOUT_SECS,
            SUBAGENT_WAIT_MAX_TIMEOUT_SECS,
        );
    let timeout = Duration::from_secs(timeout_secs);
    let agent_ref = parse_agent_ref(input);

    // 预先解析监视集，以便错误引用立即失败，而不是阻塞整个超时时间。
    let watched: Vec<String> = {
        let manager = manager.read().await;
        if let Some(agent_ref) = &agent_ref {
            let snapshot = manager
                .get_result_by_ref(agent_ref)
                .map_err(|err| ToolError::invalid_input(err.to_string()))?;
            if snapshot.status != SubAgentStatus::Running {
                let running = manager.running_count();
                drop(manager);
                return wait_result_payload(&[snapshot], running, 0, false).await;
            }
            vec![snapshot.agent_id]
        } else {
            manager
                .list_filtered(false)
                .into_iter()
                .filter(|snapshot| snapshot.status == SubAgentStatus::Running)
                .map(|snapshot| snapshot.agent_id)
                .collect()
        }
    };

    if watched.is_empty() {
        let payload = json!({
            "action": "wait",
            "settled": [],
            "running": 0,
            "note": "No running sub-agents; nothing to wait for.",
        });
        let mut tool_result = ToolResult::json(&payload)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        tool_result.metadata = Some(json!({ "action": "wait", "settled": 0, "running": 0 }));
        return Ok(tool_result);
    }

    let started = Instant::now();
    let cancelled = async {
        match &context.cancel_token {
            Some(token) => token.cancelled().await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(cancelled);

    loop {
        let (settled, running) = {
            let manager = manager.read().await;
            let mut settled = Vec::new();
            for agent_id in &watched {
                if let Ok(snapshot) = manager.get_result_by_ref(agent_id)
                    && snapshot.status != SubAgentStatus::Running
                {
                    settled.push(snapshot);
                }
            }
            (settled, manager.running_count())
        };

        if !settled.is_empty() || running == 0 {
            return wait_result_payload(&settled, running, started.elapsed().as_millis(), false)
                .await;
        }
        if started.elapsed() >= timeout {
            return wait_result_payload(&[], running, started.elapsed().as_millis(), true).await;
        }

        tokio::select! {
            () = &mut cancelled => {
                return Ok(ToolResult::success(
                    "Wait interrupted by user cancellation before any sub-agent settled.",
                ));
            }
            () = tokio::time::sleep(SUBAGENT_WAIT_CHECK_INTERVAL) => {}
        }
    }
}

/// 紧凑的 `action=wait` 结果。有意不提供完整投影：
/// 运行时的完成哨兵（和对已稳定子级的后续 peek）携带完整有效载荷；
/// 在此处复制它会加倍 token 成本。
async fn wait_result_payload(
    settled: &[SubAgentResult],
    running: usize,
    waited_ms: u128,
    timed_out: bool,
) -> Result<ToolResult, ToolError> {
    let settled_entries: Vec<Value> = settled
        .iter()
        .map(|snapshot| {
            json!({
                "agent_id": snapshot.agent_id,
                "name": snapshot.name,
                "status": subagent_status_name(&snapshot.status),
            })
        })
        .collect();
    let note = if timed_out {
        "Wait timed out with children still running. Do not poll — either wait again, continue independent work, or end your turn; results arrive automatically as <codewhale:subagent.done> sentinels."
    } else if settled_entries.is_empty() {
        "No sub-agents are running anymore."
    } else {
        "Full results arrive as <codewhale:subagent.done> sentinels — read those before synthesizing; do not re-peek settled children unless you need the full projection."
    };
    let payload = json!({
        "action": "wait",
        "settled": settled_entries,
        "running": running,
        "waited_ms": u64::try_from(waited_ms).unwrap_or(u64::MAX),
        "timed_out": timed_out,
        "note": note,
    });
    let mut tool_result =
        ToolResult::json(&payload).map_err(|err| ToolError::execution_failed(err.to_string()))?;
    tool_result.metadata = Some(json!({
        "action": "wait",
        "settled": settled.len(),
        "running": running,
        "timed_out": timed_out,
    }));
    Ok(tool_result)
}

fn provider_pin_matches_session(runtime: &SubAgentRuntime, provider_id: &str) -> bool {
    let provider_id = provider_id.trim();
    let session_provider = runtime.client.api_provider();
    if let Some(provider) = crate::config::ApiProvider::parse(provider_id) {
        return provider == session_provider;
    }
    session_provider == crate::config::ApiProvider::Custom
        && runtime
            .api_config
            .as_ref()
            .and_then(|config| config.provider.as_deref())
            .map(str::trim)
            .is_some_and(|active| active == provider_id)
}

/// 解析一个新生成的进程内子级应在哪个 LLM 客户端上运行，
/// 遵循舰队名册成员的显式提供商固定（#4193）。
///
/// - 无成员、成员未固定提供商（无配置文件 / `inherit`），或成员固定了会话自己的提供商：
///   不变地复用父级/会话客户端。保留 pre-#4193 行为——无回归。
/// - 成员固定了与会话**不同**的提供商：为该提供商构建一个新客户端（其 base URL + 凭据）。
///   这是实质性修复；仅 `provider` 元数据标签在客户端共享时是无效的，
///   因此没有这个修复，请求仍会以模型 B 的 id 命中会话提供商的端点（#4093）。
///
/// 固定但无法构建的提供商是硬错误——永远不回退到会话客户端
/// （该静默回退正是 #4093 路由错误）。提供商仅来自显式固定
/// （[`explicit_fleet_provider`]），从不从模型 id 推断（EPIC #2608）。
fn child_client_for_member(
    runtime: &SubAgentRuntime,
    member: Option<&crate::fleet::profile::AgentProfile>,
) -> Result<DeepSeekClient, ToolError> {
    let session_provider = runtime.client.api_provider();
    match crate::fleet::worker_runtime::explicit_fleet_provider_id(member) {
        Some(pinned_id) if !provider_pin_matches_session(runtime, &pinned_id) => {
            runtime.client_for_provider_id(&pinned_id).map_err(|err| {
                ToolError::execution_failed(format!(
                    "fleet profile pins provider '{}' but its client could not be built \
                     ({err}). Configure that provider's credentials/base URL, or drop the \
                     provider pin to inherit the session provider '{}'.",
                    pinned_id,
                    session_provider.as_str()
                ))
            })
        }
        _ => Ok(runtime.client.clone()),
    }
}

async fn spawn_subagent_from_input(
    input: Value,
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
) -> Result<(SubAgentResult, Option<String>, WorkflowTaskSpawnMetadata), ToolError> {
    let mut spawn_request = parse_spawn_request(&input)?;
    let spawn_policy_note = apply_session_spawn_policy(&runtime, &mut spawn_request);
    let profile_member = apply_spawn_profile(&mut spawn_request, &runtime.fleet_roster)?;

    if runtime.would_exceed_depth() {
        return Err(ToolError::execution_failed(format!(
            "Sub-agent depth limit reached (current depth {}, max {}). \
             Increase via [subagents] max_depth in config.toml.",
            runtime.spawn_depth, runtime.max_spawn_depth
        )));
    }

    if let Some(remaining) = crate::retry_status::rate_limit_remaining() {
        let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
        return Err(ToolError::execution_failed(format!(
            "Provider is rate-limiting; sub-agent spawning is paused for {seconds}s. \
             Wait for the current backoff window before starting new agent work."
        )));
    }

    if spawn_request.worktree.is_some() {
        let manager_guard = manager.read().await;
        manager_guard
            .check_admission_capacity()
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
    }
    let child_workspace = prepare_child_workspace(&runtime.context.workspace, &spawn_request)?;

    let mut child_runtime = runtime.background_runtime();
    // #4193 seam 3（实质性修复）：如果已解析的名册成员的配置文件固定了与会话不同的提供商，
    // 则在任何模型标准化/路由**之前**，将子级重新绑定到该提供商的新客户端。
    // 下面每个下游模型决策都从 `child_runtime.client.api_provider()` 获取其提供商，
    // 因此在此处交换客户端才是实际将请求路由到提供商 B 的端点并使用 B 的凭据——
    // 而不是在仍指向 A 的客户端上标记 `provider = B`（#4093）。
    child_runtime.client = child_client_for_member(&runtime, profile_member.as_ref())?;
    child_runtime.max_spawn_depth = child_max_spawn_depth_for_spawn(
        child_runtime.max_spawn_depth,
        child_runtime.spawn_depth,
        spawn_request.max_depth,
        profile_member
            .as_ref()
            .and_then(|member| member.profile.delegation.max_spawn_depth),
    );
    if let Some(workspace) = child_workspace {
        child_runtime.context.workspace = workspace;
    }
    // #4042: 合并父运行时的继承拒绝列表与调用方的显式 `disallowed_tools`。
    // `background_runtime()` 已经克隆了父级的 `worker_profile.denied_tools`
    // （会话的 `--disallowed-tools`），因此默认情况下子级继承它。
    // `inherit_disallowed_tools: false` *仅*丢弃继承的列表；
    // 调用方的显式 `disallowed_tools` 始终应用（并集，拒绝不会放宽）。
    if !spawn_request.inherit_disallowed_tools {
        child_runtime.worker_profile.denied_tools.clear();
    }
    if let Some(ref caller_deny) = spawn_request.disallowed_tools {
        for tool in caller_deny {
            if !child_runtime
                .worker_profile
                .denied_tools
                .iter()
                .any(|existing| existing == tool)
            {
                child_runtime.worker_profile.denied_tools.push(tool.clone());
            }
        }
    }
    // #4193 seam 2: 针对子级的（固定）提供商而非会话提供商对请求的模型进行标准化/验证。
    // `child_runtime` 携带上面设置的提供商 B 客户端，因此无配置文件/`inherit` 成员
    // 在此处仍然看到会话提供商（无回归）。
    let configured_model = match spawn_request.model.clone() {
        Some(model) => Some(normalize_requested_subagent_model(
            &model,
            "model",
            child_runtime.client.api_provider(),
        )?),
        None => configured_model_for_role_or_type(
            &child_runtime,
            spawn_request.assignment.role.as_deref(),
            &spawn_request.agent_type,
        )?,
    };
    // 在提示词被移出下面的请求之前解析。
    let requested_model_route = spawn_model_route(&spawn_request, profile_member.as_ref());
    let (effective_prompt, _resident_conflict) = if let Some(ref file_path) =
        spawn_request.resident_file
    {
        let abs_path = if std::path::Path::new(file_path).is_absolute() {
            std::path::PathBuf::from(file_path)
        } else {
            runtime.context.workspace.join(file_path)
        };
        let file_contents = std::fs::read_to_string(&abs_path)
            .unwrap_or_else(|e| format!("<!-- resident_file read error: {e} -->"));
        let prefixed = format!(
            "<!-- resident_file: {file_path} -->\n```\n{file_contents}\n```\n\n{}",
            spawn_request.prompt
        );
        let conflict = {
            let leases = RESIDENT_LEASES.get_or_init(|| parking_lot::Mutex::new(HashMap::new()));
            let mut guard = leases.lock();
            if let Some(owner) = guard.get(file_path) {
                Some(format!(
                    "Warning: agent {owner} already holds a resident lease on {file_path}"
                ))
            } else {
                guard.insert(file_path.clone(), "pending".to_string());
                None
            }
        };
        (prefixed, conflict)
    } else {
        (spawn_request.prompt, None)
    };

    // #4193 seam 2（续）：强度/继承/更快路由和最终的提供商命名空间守卫
    // 都从运行时的客户端读取提供商，因此通过 `child_runtime`（固定提供商）
    // 而非会话 `runtime` 路由它们。路由器候选、推理努力默认值和固定模型验证
    // 然后都针对提供商 B 解析。
    let route = resolve_subagent_assignment_route(
        &child_runtime,
        configured_model,
        &effective_prompt,
        &spawn_request.agent_type,
        requested_model_route,
        spawn_request.thinking,
    )
    .await;
    let effective_model =
        ensure_subagent_model_for_provider(&child_runtime, &route.model_route, route.model)?;
    child_runtime.model = effective_model.clone();
    child_runtime.reasoning_effort = route.reasoning_effort.clone();
    child_runtime.reasoning_effort_auto = false;
    let model_route = route.model_route;
    let resolved_role = profile_member
        .as_ref()
        .map(|member| member.profile.role.name.clone())
        .filter(|name| !name.trim().is_empty())
        .or_else(|| spawn_request.assignment.role.clone());
    let resolved_profile = profile_member
        .as_ref()
        .map(|member| member.id.clone())
        .or_else(|| spawn_request.profile.clone());
    let spawn_metadata = WorkflowTaskSpawnMetadata {
        resolved_provider: child_runtime.client.api_provider().as_str().to_string(),
        resolved_model: effective_model.clone(),
        route_source: route_source_label(&model_route),
        resolved_role,
        resolved_profile,
        parent_task_id: child_runtime.parent_agent_id.clone(),
        depth: child_runtime.spawn_depth,
        workflow_run_id: None,
        workflow_phase_id: None,
        workflow_task_label: None,
        workflow_child_index: None,
    };

    let mut manager_guard = manager.write().await;

    let result = manager_guard
        .spawn_background_with_assignment_options(
            Arc::clone(&manager),
            child_runtime,
            spawn_request.agent_type,
            effective_prompt,
            spawn_request.assignment,
            spawn_request.allowed_tools,
            SubAgentSpawnOptions {
                name: spawn_request.session_name.clone(),
                model: Some(effective_model),
                model_route: Some(model_route),
                nickname: None,
                fork_context: spawn_request.fork_context,
                token_budget: spawn_request.token_budget,
            },
        )
        .map_err(|e| ToolError::execution_failed(format!("Failed to spawn sub-agent: {e}")))?;

    if let Some(ref file_path) = spawn_request.resident_file
        && let Some(lock) = RESIDENT_LEASES.get()
    {
        let mut guard = lock.lock();
        if let Some(owner) = guard.get_mut(file_path)
            && owner == "pending"
        {
            *owner = result.agent_id.clone();
        }
    }

    Ok((result, spawn_policy_note, spawn_metadata))
}

/// 根编排器的模式感知生成默认值（Wave 7 M4/M5）。
fn apply_session_spawn_policy(
    runtime: &SubAgentRuntime,
    request: &mut SpawnRequest,
) -> Option<String> {
    if runtime.spawn_depth > 0 {
        return None;
    }
    match runtime.parent_mode {
        AppMode::Operate => {
            if request.profile.is_some() || request.agent_type_explicit {
                return None;
            }
            Some(
                "Operate spawn policy: pass profile=scout|builder|reviewer|verifier or use workflow for multi-step work; the operator orchestrates, workers execute."
                    .to_string(),
            )
        }
        _ => None,
    }
}

/// 通过公共 `agent` 工具的相同路径生成一个 Workflow `task(...)`。
/// 将此适配器保留在子代理模块内，防止 Workflow 驱动复制舰队名册/配置文件/深度/预算语义。
///
/// `identity` 被标记到返回的生成元数据上，以便面板/历史消费者无需解析提示词文本
/// 即可渲染工作流子级（#4119）。
pub(crate) async fn spawn_workflow_task(
    request: codewhale_workflow_js::TaskRequest,
    manager: SharedSubAgentManager,
    mut runtime: SubAgentRuntime,
    identity: WorkflowTaskSpawnIdentity,
) -> Result<WorkflowTaskSpawnResult, ToolError> {
    // 在将 `request` 字段消费到 agent-tool 输入 JSON 之前捕获身份回退。
    let request_label = request
        .label
        .as_ref()
        .map(|label| label.trim())
        .filter(|label| !label.is_empty())
        .map(str::to_string);
    let request_phase = request
        .phase
        .as_ref()
        .map(|phase| phase.trim())
        .filter(|phase| !phase.is_empty())
        .map(str::to_string);
    let mut input = json!({
        "prompt": request.description,
        "worktree": request.worktree,
    });
    if let Some(value) = request.subagent_type {
        input["type"] = json!(value);
    }
    if let Some(value) = request.role {
        input["role"] = json!(value);
    }
    if let Some(value) = request.profile {
        input["profile"] = json!(value);
    }
    if let Some(value) = request.model {
        input["model"] = json!(value);
    }
    if let Some(value) = request.model_strength {
        input["model_strength"] = json!(value);
    }
    if let Some(value) = request.thinking {
        input["thinking"] = json!(value);
    }
    if let Some(value) = request.allowed_tools {
        input["allowed_tools"] = json!(value);
    }
    if let Some(value) = request.max_depth {
        input["max_depth"] = json!(value);
    }
    if let Some(value) = request.token_budget {
        input["token_budget"] = json!(value);
    }
    // Workflow 子级继承父级工具表面，并对可写角色自动接受 Suggest 级别的文件编辑。
    // Shell/网络/MCP 仍然需要父级自动批准（或静默失败）。
    runtime.accept_edits = true;
    let (result, _, mut metadata) = spawn_subagent_from_input(input, manager, runtime).await?;
    // 优先使用驱动程序标记的身份值；回退到任务选项。
    let workflow_task_label = identity
        .workflow_task_label
        .filter(|label| !label.trim().is_empty())
        .or(request_label);
    let workflow_phase_id = identity
        .workflow_phase_id
        .filter(|phase| !phase.trim().is_empty())
        .or(request_phase);
    metadata.workflow_run_id = Some(identity.workflow_run_id);
    metadata.workflow_phase_id = workflow_phase_id;
    metadata.workflow_task_label = workflow_task_label;
    metadata.workflow_child_index = Some(identity.workflow_child_index);
    Ok(WorkflowTaskSpawnResult { result, metadata })
}

// === 子代理执行 ===

/// 构建子代理的系统提示词。
///
/// 以每个类型的提示词（`SubAgentType::system_prompt`）开始，
/// 并在 `assignment.role` 设置时附加一行角色覆盖。
/// 完整的角色库——来自 `~/.deepseek/roles/` 的 TOML 覆盖、`/roles` 斜杠命令、
/// 每个角色的模型覆盖——在 0.6.7 中落地。
/// 对于 0.6.6，我们只是不丢弃角色：模型看到"You are operating in the role of `{name}`."
/// 作为最后一行，以便其行为反映用户的选择。
fn build_subagent_system_prompt(
    agent_type: &SubAgentType,
    assignment: &SubAgentAssignment,
) -> String {
    let base = agent_type.system_prompt();
    let mut prompt = match assignment.role.as_deref() {
        Some(role) if !role.trim().is_empty() => {
            format!(
                "{base}\n\nYou are operating in the role of `{}`.",
                role.trim()
            )
        }
        _ => base,
    };
    // 子代理是后台工作者：编排代理是它们唯一的调用方。它们从不与最终用户通信。
    prompt.push_str(
        "\n\nYou are a background sub-agent: every instruction comes from the orchestrating agent, not a human. Never address the end user or ask them questions — do the assigned work and report results back to the orchestrator.",
    );
    prompt
}

fn subagent_request_system_prompt(
    subagent_system_prompt: &str,
    fork_context: Option<&SubAgentForkContext>,
) -> SystemPrompt {
    fork_context
        .and_then(|context| context.system.clone())
        .unwrap_or_else(|| SystemPrompt::Text(subagent_system_prompt.to_string()))
}

fn build_initial_subagent_messages(
    prompt: &str,
    assignment: &SubAgentAssignment,
    agent_type: &SubAgentType,
    fork_context: Option<&SubAgentForkContext>,
) -> Vec<Message> {
    let mut messages = fork_context
        .map(|context| context.messages.clone())
        .unwrap_or_default();

    if let Some(context) = fork_context {
        if let Some(state) = context
            .structured_state_block
            .as_deref()
            .map(str::trim)
            .filter(|state| !state.is_empty())
        {
            messages.push(system_text_message(format!(
                "<codewhale:fork_state>\n{state}\n</codewhale:fork_state>"
            )));
        }

        messages.push(system_text_message(format!(
            "<codewhale:subagent_context>\n{}\n</codewhale:subagent_context>",
            build_subagent_system_prompt(agent_type, assignment)
        )));
    }

    messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: build_assignment_prompt(prompt, assignment, agent_type),
            cache_control: None,
        }],
    });

    messages
}

fn system_text_message(text: String) -> Message {
    Message {
        role: "system".to_string(),
        content: vec![ContentBlock::Text {
            text,
            cache_control: None,
        }],
    }
}

struct SubAgentTask {
    manager_handle: SharedSubAgentManager,
    runtime: SubAgentRuntime,
    agent_id: String,
    agent_type: SubAgentType,
    prompt: String,
    assignment: SubAgentAssignment,
    /// `None` = 完整注册表继承。`Some(list)` = 显式窄化。
    /// 需要批准的工具仍然需要自动批准的父运行时。
    allowed_tools: Option<Vec<String>>,
    fork_context: bool,
    started_at: Instant,
    max_steps: u32,
    /// 来自生成请求 `token_budget`（显式的 `max_tokens`/`tokenBudget` 覆盖）的每工作者 token 上限。
    /// `None` 表示无每工作者限制；工作者仍然遵守作用域准入门控。
    /// 设置时，一旦累积的模型 token 超过此值，工作者将以 `BudgetExhausted` 停止。
    /// 独立于作用域预算（#3319）。
    token_budget: Option<u64>,
    input_rx: mpsc::UnboundedReceiver<SubAgentInput>,
    /// 交互式启动门控（#3095）。仅对直接（深度为 1）子级为 `Some`：
    /// 任务在其第一个模型步骤之前获取许可并持有直到完成，
    /// 因此超出限制的扇出爆发会以可见原因排队，而非一次性全部执行。
    launch_gate: Option<Arc<Semaphore>>,
}

#[allow(clippy::too_many_lines)]
async fn run_subagent_task(task: SubAgentTask) {
    // 交互式启动门控（#3095）：直接子级在其第一个模型步骤之前获取许可，
    // 以便超出限制的扇出爆发可见地排队，而非一次性全部执行。
    // 许可在任务的生命周期内持有。排队时的取消由 `run_subagent` 自己的第一步取消检查处理。
    let mut _launch_permit = None;
    if let Some(gate) = task.launch_gate.as_ref() {
        match Arc::clone(gate).try_acquire_owned() {
            Ok(permit) => _launch_permit = Some(permit),
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                _launch_permit = acquire_queued_launch_permit(&task, Arc::clone(gate)).await;
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                crate::logging::warn(format!(
                    "sub-agent launch gate closed for {}; proceeding without backpressure",
                    task.agent_id
                ));
            }
        }
    }

    let result = run_subagent(
        &task.runtime,
        task.agent_id.clone(),
        task.agent_type,
        task.prompt,
        task.assignment,
        task.allowed_tools,
        task.fork_context,
        task.started_at,
        task.max_steps,
        task.token_budget,
        task.input_rx,
    )
    .await;

    // 同时发出人类友好的摘要（在父级侧边栏/单元格中渲染）和模型在下一轮中可以识别的结构化哨兵。
    // 格式：第一行人类摘要，第二行哨兵。
    // 哨兵使用不透明标签（`codewhale:subagent.done`）以避免与普通用户文本冲突。
    let model_id = task.runtime.model.clone();
    let (summary, sentinel) = match &result {
        Ok(res) => {
            // Issue #2652: 子级的自由文本结果是其自我报告，而非经过验证的证据。
            // 使用来源标记进行标记：简短时加一个柔和的"重新验证"说明，
            // 超出线缆预算时进行头部+尾部截断（复用工具输出词汇）。
            // 结果 `truncated` 标志携带在哨兵中，以便父级模型可以根据 `summary_kind` 分支。
            let raw = summarize_subagent_result(res);
            let (summary, truncated) = stamp_subagent_summary(&raw);
            let sentinel = match &res.status {
                SubAgentStatus::Failed(_) | SubAgentStatus::BudgetExhausted => {
                    subagent_failed_sentinel(&task.agent_id, &raw)
                }
                _ => subagent_done_sentinel(&task.agent_id, res, truncated),
            };
            (summary, sentinel)
        }
        Err(err) => {
            crate::logging::warn(format!(
                "sub-agent {} model request failed: {err:#}",
                task.agent_id
            ));
            let annotated = annotate_child_model_error(
                &subagent_failure_message(err),
                &model_id,
                task.runtime.client.api_provider(),
                &task.runtime.worker_profile.model,
            );
            (
                format!("Failed: {annotated}"),
                subagent_failed_sentinel(&task.agent_id, &annotated),
            )
        }
    };

    if let Some(mb) = task.runtime.mailbox.as_ref() {
        let envelope = match &result {
            Ok(res) => match &res.status {
                SubAgentStatus::Failed(_) | SubAgentStatus::BudgetExhausted => {
                    MailboxMessage::Failed {
                        agent_id: task.agent_id.clone(),
                        error: summary.clone(),
                    }
                }
                _ => MailboxMessage::Completed {
                    agent_id: task.agent_id.clone(),
                    summary: summary.clone(),
                },
            },
            Err(err) => MailboxMessage::Failed {
                agent_id: task.agent_id.clone(),
                error: annotate_child_model_error(
                    &subagent_failure_message(err),
                    &model_id,
                    task.runtime.client.api_provider(),
                    &task.runtime.worker_profile.model,
                ),
            },
        };
        let _ = mb.send(envelope);
    }

    let payload = format!("{summary}\n{sentinel}");
    let agent_id = task.agent_id.clone();

    // 如果这是引擎的直接子级之一（issue #756），则唤醒引擎的父级轮次循环。
    // Issue #1961 也要求发出的时间在标记管理器终止状态之前，
    // 以便父级在其"运行中子级"门控仍打开时可以观察到完成。
    // 如果我们先更新，父级可能在完成事件到达之前就终结了。
    emit_parent_completion(&task.runtime, &agent_id, &payload);

    let mut manager = task.manager_handle.write().await;
    match &result {
        Ok(res) => manager.update_from_result(&agent_id, res.clone()),
        Err(err) => {
            manager.update_failed(
                &agent_id,
                annotate_child_model_error(
                    &subagent_failure_message(err),
                    &model_id,
                    task.runtime.client.api_provider(),
                    &task.runtime.worker_profile.model,
                ),
            );
        }
    }

    if let Some(event_tx) = task.runtime.event_tx {
        let _ = event_tx.try_send(Event::AgentComplete {
            id: agent_id.clone(),
            result: payload,
        });
    }
}

async fn acquire_queued_launch_permit(
    task: &SubAgentTask,
    gate: Arc<Semaphore>,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    record_queued_launch_progress(task).await;
    tokio::select! {
        biased;
        () = task.runtime.cancel_token.cancelled() => {
            record_agent_progress(
                &task.runtime,
                &task.agent_id,
                "cancelled while queued for a sub-agent launch slot".to_string(),
            );
            None
        }
        permit = Arc::clone(&gate).acquire_owned() => {
            permit.ok()
        }
    }
}

async fn record_queued_launch_progress(task: &SubAgentTask) {
    {
        let mut manager = task.runtime.manager.write().await;
        manager.touch(&task.agent_id);
        manager.record_worker_event(
            &task.agent_id,
            AgentWorkerStatus::Queued,
            Some(SUBAGENT_QUEUED_LAUNCH_REASON.to_string()),
            None,
            None,
        );
    }
    emit_agent_progress(
        task.runtime.event_tx.as_ref(),
        &task.agent_id,
        SUBAGENT_QUEUED_LAUNCH_REASON.to_string(),
        task.runtime.parent_agent_id.clone(),
        task.runtime.spawn_depth,
    );
    if let Some(mailbox) = task.runtime.mailbox.as_ref() {
        let _ = mailbox.send(MailboxMessage::progress(
            &task.agent_id,
            SUBAGENT_QUEUED_LAUNCH_REASON,
        ));
    }
}

/// 通知此运行时的直接父级子级已完成（issue #756）。
/// 根生成的子级发送到引擎轮次循环。嵌套子级发送到父级子代理的本地收件箱，
/// 该收件箱被交换到父级 `agent` 工具使用的运行时中。
/// 如果尝试发送则返回 `true`，如果这是引擎本身或没有通道连接则返回 `false`。
/// 当通道发送方没有接收方时静默跳过——接收方可能因为父级轮次/代理已完成而结束。
pub(crate) fn emit_parent_completion(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    payload: &str,
) -> bool {
    if runtime.spawn_depth == 0 {
        return false;
    }
    let Some(tx) = runtime.parent_completion_tx.as_ref() else {
        return false;
    };
    let _ = tx.send(SubAgentCompletion {
        agent_id: agent_id.to_string(),
        payload: payload.to_string(),
    });
    true
}

pub(crate) fn subagent_completion_from_result(result: &SubAgentResult) -> SubAgentCompletion {
    let raw = summarize_subagent_result(result);
    let mut evidence_truncated = false;
    let evidence_block = match &result.status {
        SubAgentStatus::Failed(_)
        | SubAgentStatus::BudgetExhausted
        | SubAgentStatus::Cancelled
        | SubAgentStatus::Interrupted(_) => None,
        _ => result
            .result
            .as_deref()
            .and_then(extract_evidence_block)
            .map(|block| {
                let (clipped, ev_trunc) = clip_evidence_block(&block);
                evidence_truncated = ev_trunc;
                clipped
            })
            .filter(|evidence| !evidence.trim().is_empty()),
    };
    let summary_source = evidence_block
        .as_ref()
        .map(|_| strip_evidence_block(&raw))
        .unwrap_or(raw);
    let (summary, truncated) = stamp_subagent_summary(&summary_source);
    let summary_truncated = truncated || evidence_truncated;
    let sentinel = match &result.status {
        SubAgentStatus::Failed(error) => subagent_failed_sentinel(&result.agent_id, error),
        _ => subagent_done_sentinel(&result.agent_id, result, summary_truncated),
    };
    let payload = match evidence_block {
        Some(evidence) => format!("{summary}\n{evidence}\n{sentinel}"),
        None => format!("{summary}\n{sentinel}"),
    };
    SubAgentCompletion {
        agent_id: result.agent_id.clone(),
        payload,
    }
}

const SUBAGENT_EVIDENCE_CHAR_BUDGET: usize = 4_000;

fn clip_evidence_block(block: &str) -> (String, bool) {
    let total = block.chars().count();
    if total <= SUBAGENT_EVIDENCE_CHAR_BUDGET {
        return (block.to_string(), false);
    }
    let clipped: String = block.chars().take(SUBAGENT_EVIDENCE_CHAR_BUDGET).collect();
    (format!("{clipped}…"), true)
}

fn extract_evidence_block(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let markers = ["### evidence", "## evidence", "evidence:"];
    for marker in markers {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        let block = &text[start..];
        let tail = &block[marker.len()..];
        let end = tail
            .find("\n### ")
            .or_else(|| tail.find("\n## "))
            .or_else(|| tail.to_ascii_lowercase().find("\ngaps"))
            .or_else(|| tail.to_ascii_lowercase().find("\nnext"))
            .unwrap_or(tail.len());
        let extracted = format!("{}{}", &block[..marker.len()], &tail[..end])
            .trim()
            .to_string();
        if !extracted.is_empty() {
            return Some(extracted);
        }
    }
    None
}

fn strip_evidence_block(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let markers = ["### evidence", "## evidence", "evidence:"];
    for marker in markers {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        let block = &text[start..];
        let tail = &block[marker.len()..];
        let end = tail
            .find("\n### ")
            .or_else(|| tail.find("\n## "))
            .or_else(|| tail.to_ascii_lowercase().find("\ngaps"))
            .or_else(|| tail.to_ascii_lowercase().find("\nnext"))
            .unwrap_or(tail.len());
        let mut without = format!("{}{}", &text[..start], &block[marker.len() + end..]);
        without = without.trim().to_string();
        return without;
    }
    text.trim().to_string()
}

/// 为成功的子级构建 `<codewhale:subagent.done>` JSON 哨兵。
/// 旨在出现在父级的转录中，以便模型识别子级完成。
///
/// 有意保持此有效载荷精简。人类摘要紧接在哨兵之前发出；
/// 在此处复述它会使下一个父级请求的缓存未命中尾部膨胀。
/// 挂钟持续时间是有用的 UI 遥测数据，但它是易变的，对模型协调没有用处。
///
/// `truncated` 反映前一行摘要是否被 [`stamp_subagent_summary`]（issue #2652）
/// 长度限制；它以 `summary_kind` 形式呈现，以便父级模型可以区分完整自我报告
/// 和截断版本，并相应地验证实质性声明。
fn subagent_done_sentinel(agent_id: &str, res: &SubAgentResult, truncated: bool) -> String {
    let mut payload = json!({
        "agent_id": agent_id,
        // 鲸鱼名称——编排者可以在自己的推理/输出中引用此子级的稳定、人类友好的句柄。
        "name": res.nickname,
        "agent_type": res.agent_type.as_str(),
        "status": subagent_status_name(&res.status),
        "summary_location": "previous_line",
        // issue #2652: 让父级根据前一行摘要是完整子级报告还是头部+尾部摘录进行分支。
        "summary_kind": if truncated { "truncated" } else { "complete" },
    });
    if let Some(needs_input) = res.needs_input.clone() {
        payload["needs_input"] = json!(needs_input);
    }
    format!("<codewhale:subagent.done>{payload}</codewhale:subagent.done>")
}

/// 为失败的子级构建 `<codewhale:subagent.done>` 哨兵。
///
/// 保持精简：（带注释的）错误在前一行（`error_location`），
/// 因此哨兵仅信号完成状态，而非重新嵌入错误文本。
fn subagent_failed_sentinel(agent_id: &str, _err: &str) -> String {
    let payload = json!({
        "agent_id": agent_id,
        "status": "failed",
        "error_location": "previous_line",
    });
    format!("<codewhale:subagent.done>{payload}</codewhale:subagent.done>")
}

fn response_was_truncated(response: &MessageResponse) -> bool {
    response.stop_reason.as_deref() == Some("length")
}

fn truncated_response_tool_results(tool_uses: &[(String, String, Value)]) -> Vec<ContentBlock> {
    tool_uses
        .iter()
        .map(|(tool_id, tool_name, _)| ContentBlock::ToolResult {
            tool_use_id: tool_id.clone(),
            content: format!(
                "Error: the model response was truncated by max_tokens before the tool call arguments for '{tool_name}' could be fully generated. Split large content into smaller writes and retry."
            ),
            is_error: Some(true),
            content_blocks: None,
        })
        .collect()
}

fn truncated_response_text_retry_message() -> Vec<ContentBlock> {
    vec![ContentBlock::Text {
        text: "Error: the model response was truncated by max_tokens. No complete tool call was available, so the partial response was not accepted as the sub-agent result. Retry with a shorter response or split the work into smaller steps.".to_string(),
        cache_control: None,
    }]
}

fn record_truncated_subagent_response(consecutive: &mut u32) -> Result<()> {
    *consecutive = consecutive.saturating_add(1);
    if *consecutive > MAX_CONSECUTIVE_TRUNCATED_SUBAGENT_RESPONSES {
        return Err(anyhow!(
            "Sub-agent response was truncated by max_tokens {count} consecutive times; stopping to avoid an unbounded retry loop.",
            count = *consecutive
        ));
    }
    Ok(())
}

fn reset_truncated_subagent_responses(consecutive: &mut u32) {
    *consecutive = 0;
}

#[allow(clippy::too_many_arguments)]
async fn insert_subagent_full_transcript_handle(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    agent_type: &SubAgentType,
    assignment: &SubAgentAssignment,
    status: &SubAgentStatus,
    result: Option<&String>,
    checkpoint: Option<&SubAgentCheckpoint>,
    messages: &[Message],
    steps_taken: u32,
    duration_ms: u64,
    fork_context: bool,
) -> VarHandle {
    // 字节限制保留的转录（#3882）：句柄存储使此有效载荷常驻于每个代理，
    // 且检查点已经携带自己的有界消息尾部——逐字嵌入它将在一个有效载荷内复制该尾部。
    // 保留检查点元数据，丢弃其消息，并记录有界尾部省略了多少真实历史。
    let (bounded_messages, omitted_messages) =
        bounded_tail_messages(messages, SUBAGENT_TRANSCRIPT_MESSAGE_BUDGET_BYTES);
    let checkpoint_meta = checkpoint.map(|checkpoint| SubAgentCheckpoint {
        omitted_messages: checkpoint.message_count,
        messages: Vec::new(),
        ..checkpoint.clone()
    });
    let payload = json!({
        "kind": "subagent_full_transcript",
        "agent_id": agent_id,
        "agent_type": agent_type.as_str(),
        "status": subagent_status_name(status),
        "context_mode": if fork_context { "forked" } else { "fresh" },
        "fork_context": fork_context,
        "result": result,
        "steps_taken": steps_taken,
        "duration_ms": duration_ms,
        "assignment": assignment,
        "checkpoint": checkpoint_meta,
        "message_count": messages.len(),
        "omitted_messages": omitted_messages,
        "messages": bounded_messages,
    });
    let mut store = runtime.context.runtime.handle_store.lock().await;
    store.insert_json(format!("agent:{agent_id}"), "full_transcript", payload)
}

/// 在子代理工具结果进入 `messages`（#3882）之前对其进行限制。
///
/// 根引擎在 `turn_loop.rs` 中应用溢出；子代理循环绕过了它，
/// 因此一个多 MB 的构建日志变成了子级消息、检查点、转录句柄和持久化中的多个常驻副本——
/// Fleet 扇出内存爆炸。超过阈值的内容（成功**和**错误：子代理错误输出通常是完整的构建日志，
/// 因此根循环的"错误通过"原则在此处不成立）被写入共享溢出目录，
/// 并内联替换为有界的头部加上命名磁盘路径的页脚。
///
/// 返回（可能受限的）内容和（写入时）的溢出路径。
/// 溢出写入失败降级为传递原始内容，镜像 `apply_spillover`。
fn bound_subagent_tool_result(
    agent_id: &str,
    tool_id: &str,
    content: String,
) -> (String, Option<PathBuf>) {
    if content.len() <= SPILLOVER_THRESHOLD_BYTES {
        return (content, None);
    }
    let spill_id = format!("sa_{agent_id}_{tool_id}");
    match maybe_spillover(
        &spill_id,
        &content,
        SPILLOVER_THRESHOLD_BYTES,
        SPILLOVER_HEAD_BYTES,
    ) {
        Ok(Some((head, path))) => {
            let footer = format!(
                "\n\n[Sub-agent tool output truncated: {head_kib} KiB of {total_kib} KiB shown. \
                 Full output saved to {path}. Use `read_file` on that path if you need the \
                 elided output.]",
                head_kib = head.len() / 1024,
                total_kib = content.len() / 1024,
                path = path.display(),
            );
            (format!("{head}{footer}"), Some(path))
        }
        Ok(None) => (content, None),
        Err(err) => {
            tracing::warn!(
                target: "subagent",
                ?err,
                agent_id,
                tool_id,
                "sub-agent spillover write failed; passing original content through"
            );
            (content, None)
        }
    }
}

/// 一条消息的粗略序列化大小，用于检查点/转录字节预算。
/// 通过 serde 获取精确 JSON 大小；不可序列化的消息（不应发生）计为 1 KiB，以便它们仍然消耗预算。
fn approximate_message_bytes(message: &Message) -> usize {
    serde_json::to_string(message).map_or(1024, |s| s.len())
}

/// 保留组合近似大小适合 `budget_bytes` 的最近消息。
/// 始终至少保留最后一条消息（即使它单独超出预算），以便非空历史保持可继续。
/// 返回保留的尾部和跳过了多少条旧消息。
fn bounded_tail_messages(messages: &[Message], budget_bytes: usize) -> (Vec<Message>, usize) {
    let mut kept_rev: Vec<Message> = Vec::new();
    let mut used = 0usize;
    for message in messages.iter().rev() {
        let size = approximate_message_bytes(message);
        if !kept_rev.is_empty() && used.saturating_add(size) > budget_bytes {
            break;
        }
        used = used.saturating_add(size);
        kept_rev.push(message.clone());
    }
    kept_rev.reverse();
    let omitted = messages.len().saturating_sub(kept_rev.len());
    (kept_rev, omitted)
}

fn build_subagent_checkpoint(
    agent_id: &str,
    reason: impl Into<String>,
    messages: &[Message],
    steps_taken: u32,
    continuable: bool,
) -> SubAgentCheckpoint {
    let created_at_ms = epoch_millis_now();
    let checkpoint_id = format!("{agent_id}:step:{steps_taken}:ts:{created_at_ms}");
    let (bounded_messages, omitted_messages) =
        bounded_tail_messages(messages, SUBAGENT_CHECKPOINT_MESSAGE_BUDGET_BYTES);
    SubAgentCheckpoint {
        checkpoint_id: checkpoint_id.clone(),
        agent_id: agent_id.to_string(),
        continuation_handle: format!("agent:{agent_id}:checkpoint:{checkpoint_id}"),
        reason: reason.into(),
        continuable,
        steps_taken,
        message_count: messages.len(),
        created_at_ms,
        messages: bounded_messages,
        omitted_messages,
    }
}

async fn checkpoint_subagent_progress(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    reason: impl Into<String>,
    messages: &[Message],
    steps_taken: u32,
    continuable: bool,
) -> SubAgentCheckpoint {
    let checkpoint =
        build_subagent_checkpoint(agent_id, reason, messages, steps_taken, continuable);
    let mut manager = runtime.manager.write().await;
    manager.update_checkpoint(agent_id, checkpoint.clone());
    checkpoint
}

fn needs_input_for_interrupted_checkpoint(
    reason: &str,
    checkpoint: &SubAgentCheckpoint,
) -> SubAgentNeedsInput {
    SubAgentNeedsInput {
        question: format!(
            "Sub-agent interrupted before completion ({reason}). Re-dispatch this worker or provide explicit follow-up using checkpoint {}.",
            checkpoint.continuation_handle
        ),
    }
}

#[derive(Debug)]
enum SubAgentApiRequestFailure {
    Fatal(anyhow::Error),
    Interrupted {
        reason: String,
        checkpoint_reason: &'static str,
    },
}

fn subagent_transient_provider_retry_delay(retry_number: u32) -> Duration {
    let multiplier = 1u32
        .checked_shl(retry_number.saturating_sub(1))
        .unwrap_or(4);
    SUBAGENT_TRANSIENT_PROVIDER_INITIAL_BACKOFF.saturating_mul(multiplier.min(4))
}

#[derive(Debug, Clone, Copy)]
struct RetryableSubAgentProviderFailure {
    label: &'static str,
    checkpoint_reason: &'static str,
    delay: Duration,
}

fn retryable_subagent_provider_failure(
    error: &anyhow::Error,
    retry_number: u32,
) -> Option<RetryableSubAgentProviderFailure> {
    if let Some(LlmError::RateLimited { retry_after, .. }) = error.downcast_ref::<LlmError>() {
        return Some(RetryableSubAgentProviderFailure {
            label: "rate-limited provider response",
            checkpoint_reason: "api_rate_limited",
            delay: retry_after
                .unwrap_or_else(|| subagent_transient_provider_retry_delay(retry_number)),
        });
    }

    if is_transient_subagent_provider_error(error) {
        return Some(RetryableSubAgentProviderFailure {
            label: "transient provider failure",
            checkpoint_reason: "api_transient_provider_failure",
            delay: subagent_transient_provider_retry_delay(retry_number),
        });
    }

    None
}

fn is_transient_subagent_provider_error(error: &anyhow::Error) -> bool {
    if let Some(LlmError::RateLimited { .. }) = error.downcast_ref::<LlmError>() {
        return true;
    }

    let message = format!("{error:#}").to_ascii_lowercase();
    [
        "did not receive response headers",
        "response headers",
        "stream request",
        "request timed out",
        "operation timed out",
        "deadline has elapsed",
        "connection reset",
        "connection closed",
        "connection aborted",
        "temporarily unavailable",
        "bad gateway",
        "gateway timeout",
        "service unavailable",
        "rate limited",
        "rate_limit",
        "rate_limited",
        "too many requests",
        "429",
        "502",
        "503",
        "504",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

async fn request_subagent_model_response_with_retries(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    steps: u32,
    max_steps: u32,
    request: MessageRequest,
) -> std::result::Result<MessageResponse, SubAgentApiRequestFailure> {
    let mut transient_failures = 0u32;

    loop {
        match tokio::time::timeout(
            runtime.step_api_timeout,
            runtime.client.create_message(request.clone()),
        )
        .await
        {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(err)) => {
                let retry_number = transient_failures.saturating_add(1);
                let Some(retryable) = retryable_subagent_provider_failure(&err, retry_number)
                else {
                    return Err(SubAgentApiRequestFailure::Fatal(err));
                };

                if transient_failures >= SUBAGENT_TRANSIENT_PROVIDER_MAX_RETRIES {
                    let attempts = transient_failures.saturating_add(1);
                    return Err(SubAgentApiRequestFailure::Interrupted {
                        reason: format!(
                            "{} after {attempts} API attempt(s): {err}; checkpoint preserved for continuation",
                            retryable.label
                        ),
                        checkpoint_reason: retryable.checkpoint_reason,
                    });
                }

                transient_failures = transient_failures.saturating_add(1);
                let delay = retryable.delay;
                record_agent_progress(
                    runtime,
                    agent_id,
                    format!(
                        "{}: {}; retrying API request {}/{} in {}ms ({err})",
                        format_step_counter(steps, max_steps),
                        retryable.label,
                        transient_failures,
                        SUBAGENT_TRANSIENT_PROVIDER_MAX_RETRIES,
                        delay.as_millis(),
                    ),
                );
                tokio::time::sleep(delay).await;
            }
            Err(_) => {
                return Err(SubAgentApiRequestFailure::Interrupted {
                    reason: format!(
                        "API call timed out after {}ms; checkpoint preserved for continuation",
                        runtime.step_api_timeout.as_millis()
                    ),
                    checkpoint_reason: "api_timeout",
                });
            }
        }
    }
}

fn record_agent_progress(runtime: &SubAgentRuntime, agent_id: &str, message: impl Into<String>) {
    let message = message.into();
    if let Ok(mut manager) = runtime.manager.try_write() {
        manager.touch(agent_id);
        manager.record_worker_progress(agent_id, message.clone());
    }
    emit_agent_progress(
        runtime.event_tx.as_ref(),
        agent_id,
        message,
        runtime.parent_agent_id.clone(),
        runtime.spawn_depth,
    );
}

fn runtime_for_nested_agent_tools(
    runtime: &SubAgentRuntime,
    parent_agent_id: &str,
    fork_context: SubAgentForkContext,
) -> (SubAgentRuntime, mpsc::UnboundedReceiver<SubAgentCompletion>) {
    let (child_completion_tx, child_completion_rx) =
        mpsc::unbounded_channel::<SubAgentCompletion>();
    let runtime_for_tools = runtime
        .clone()
        .with_parent_completion_tx(child_completion_tx)
        .with_fork_context(fork_context);
    let runtime_for_tools = SubAgentRuntime {
        parent_agent_id: Some(parent_agent_id.to_string()),
        ..runtime_for_tools
    };
    (runtime_for_tools, child_completion_rx)
}

fn drain_child_completion_events(
    child_completion_rx: &mut mpsc::UnboundedReceiver<SubAgentCompletion>,
) -> Vec<SubAgentCompletion> {
    let mut completions = Vec::new();
    while let Ok(completion) = child_completion_rx.try_recv() {
        completions.push(completion);
    }
    completions
}

fn child_completion_runtime_message(completions: &[SubAgentCompletion]) -> Message {
    let mut text = String::from(
        "<codewhale:runtime_event kind=\"child_subagent_completion\" visibility=\"internal\">\n\
This is an internal runtime event, not user input. One or more child sub-agents \
you spawned have finished. Treat each child summary as an unverified self-report: \
if you rely on it, cite the child agent_id and the EVIDENCE lines it provided, \
and distinguish that from evidence you personally verified.\n",
    );
    for completion in completions {
        text.push_str("\n--- child sub-agent completion ---\n");
        text.push_str("agent_id: ");
        text.push_str(&completion.agent_id);
        text.push('\n');
        text.push_str(&completion.payload);
        text.push('\n');
    }
    text.push_str("</codewhale:runtime_event>");

    Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text,
            cache_control: None,
        }],
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_subagent(
    runtime: &SubAgentRuntime,
    agent_id: String,
    agent_type: SubAgentType,
    prompt: String,
    assignment: SubAgentAssignment,
    allowed_tools: Option<Vec<String>>,
    fork_context: bool,
    started_at: Instant,
    max_steps: u32,
    token_budget: Option<u64>,
    mut input_rx: mpsc::UnboundedReceiver<SubAgentInput>,
) -> Result<SubAgentResult> {
    let system_prompt = build_subagent_system_prompt(&agent_type, &assignment);
    let fork_context_enabled = fork_context;
    let fork_context = fork_context_enabled
        .then_some(runtime.fork_context.as_ref())
        .flatten();
    let request_system = subagent_request_system_prompt(&system_prompt, fork_context);
    let mut messages =
        build_initial_subagent_messages(&prompt, &assignment, &agent_type, fork_context);
    let (runtime_for_tools, mut child_completion_rx) = runtime_for_nested_agent_tools(
        runtime,
        &agent_id,
        SubAgentForkContext {
            system: Some(request_system.clone()),
            messages: messages.clone(),
            structured_state_block: None,
        },
    );
    let tool_registry = SubAgentToolRegistry::new_with_owner(
        runtime_for_tools,
        agent_type.clone(),
        agent_id.clone(),
        assignment
            .role
            .as_deref()
            .filter(|role| !role.trim().is_empty())
            .unwrap_or(agent_type.as_str())
            .to_string(),
        allowed_tools.clone(),
        // 共享父级的待办列表，以便子级清单更新在 Work 侧边栏中实时可见。
        // 以前每个子级都获得一个新的隔离 TodoList——父级直到完成才能看到子级进度。
        runtime.todos.clone(),
        Arc::new(Mutex::new(PlanState::default())),
    );
    let unavailable_tools = tool_registry.unavailable_allowed_tools();
    if !unavailable_tools.is_empty() {
        return Err(anyhow!(
            "Sub-agent requested unavailable tools: {}",
            unavailable_tools.join(", ")
        ));
    }
    let tools = tool_registry.tools_for_model(&agent_type);
    if let Some(mb) = runtime.mailbox.as_ref() {
        let _ = mb.send(MailboxMessage::started(&agent_id, agent_type.clone()));
    }
    record_agent_progress(
        runtime,
        &agent_id,
        format!("started ({})", agent_type.as_str()),
    );

    let mut steps = 0;
    let mut final_result: Option<String> = None;
    let mut pending_inputs: VecDeque<SubAgentInput> = VecDeque::new();
    let mut consecutive_truncated_responses = 0;
    let mut latest_checkpoint: Option<SubAgentCheckpoint> = None;
    let mut tokens_used: u64 = 0;
    // #4050: 区分模型选择停止的真实退出（下面的 `break`）与循环耗尽
    // （仍然在工具调用中用尽 `max_steps`）。只有前者，带有非空最终摘要，
    // 才是真正的成功；其他所有情况必须暴露其停止原因，
    // 而不是报告一个无有效载荷的已完成子级。
    let mut stopped_naturally = false;

    for _step in 0..max_steps {
        // 协作取消：如果此会话的令牌在步骤之间被取消，则退出。
        // 顶级模型可见的子代理使用分离的令牌，因此父级轮次取消不会停止它们。
        if runtime.cancel_token.is_cancelled() {
            record_agent_progress(
                runtime,
                &agent_id,
                format!("{}: cancelled", format_step_counter(steps, max_steps)),
            );
            if let Some(mb) = runtime.mailbox.as_ref() {
                let _ = mb.send(MailboxMessage::Cancelled {
                    agent_id: agent_id.clone(),
                });
            }
            let status = SubAgentStatus::Cancelled;
            let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            insert_subagent_full_transcript_handle(
                runtime,
                &agent_id,
                &agent_type,
                &assignment,
                &status,
                None,
                latest_checkpoint.as_ref(),
                &messages,
                steps,
                duration_ms,
                fork_context_enabled,
            )
            .await;
            return Ok(SubAgentResult {
                name: agent_id.clone(),
                agent_id: agent_id.clone(),
                context_mode: if fork_context_enabled {
                    "forked"
                } else {
                    "fresh"
                }
                .to_string(),
                fork_context: fork_context_enabled,
                workspace: Some(runtime.context.workspace.clone()),
                git_branch: current_git_branch(&runtime.context.workspace),
                agent_type: agent_type.clone(),
                assignment: assignment.clone(),
                model: runtime.model.clone(),
                nickname: None,
                status,
                worker_status: None,
                parent_run_id: runtime.parent_agent_id.clone(),
                spawn_depth: runtime.spawn_depth,
                result: None,
                steps_taken: steps,
                checkpoint: latest_checkpoint.clone(),
                needs_input: None,
                duration_ms,
                from_prior_session: false,
            });
        }

        steps += 1;
        record_agent_progress(
            runtime,
            &agent_id,
            format!(
                "{}: requesting model response",
                format_step_counter(steps, max_steps)
            ),
        );

        while let Ok(input) = input_rx.try_recv() {
            if input.interrupt {
                pending_inputs.clear();
            }
            pending_inputs.push_back(input);
        }

        while let Some(input) = pending_inputs.pop_front() {
            if !input.text.trim().is_empty() {
                messages.push(Message {
                    role: "user".to_string(),
                    content: vec![ContentBlock::Text {
                        text: input.text,
                        cache_control: None,
                    }],
                });
            }
        }

        let child_completions = drain_child_completion_events(&mut child_completion_rx);
        if !child_completions.is_empty() {
            let count = child_completions.len();
            record_agent_progress(
                runtime,
                &agent_id,
                format!(
                    "{}: received {count} child sub-agent completion(s)",
                    format_step_counter(steps, max_steps)
                ),
            );
            messages.push(child_completion_runtime_message(&child_completions));
        }

        let request = MessageRequest {
            model: runtime.model.clone(),
            messages: messages.clone(),
            max_tokens: SUBAGENT_RESPONSE_MAX_TOKENS,
            system: Some(request_system.clone()),
            tools: Some(tools.clone()),
            tool_choice: Some(json!({ "type": "auto" })),
            metadata: None,
            thinking: None,
            reasoning_effort: runtime.reasoning_effort.clone(),
            stream: Some(false),
            temperature: None,
            top_p: None,
        };
        latest_checkpoint = Some(
            checkpoint_subagent_progress(
                runtime,
                &agent_id,
                "before_api_request",
                &messages,
                steps,
                true,
            )
            .await,
        );

        // 将 API 调用与取消令牌竞速，以便在长时间思考轮次中父级取消
        // 不必等待步骤超时。
        let response = tokio::select! {
            biased;
            () = runtime.cancel_token.cancelled() => {
                record_agent_progress(
                    runtime,
                    &agent_id,
                    format!("{}: cancelled mid-request", format_step_counter(steps, max_steps)),
                );
                if let Some(mb) = runtime.mailbox.as_ref() {
                    let _ = mb.send(MailboxMessage::Cancelled {
                        agent_id: agent_id.clone(),
                    });
                }
                let status = SubAgentStatus::Cancelled;
                let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
                insert_subagent_full_transcript_handle(
                    runtime,
                    &agent_id,
                    &agent_type,
                    &assignment,
                    &status,
                    None,
                    latest_checkpoint.as_ref(),
                    &messages,
                    steps,
                    duration_ms,
                    fork_context_enabled,
                )
                .await;
                return Ok(SubAgentResult {
                    name: agent_id.clone(),
                    agent_id: agent_id.clone(),
                    context_mode: if fork_context_enabled { "forked" } else { "fresh" }.to_string(),
                    fork_context: fork_context_enabled,
                    workspace: Some(runtime.context.workspace.clone()),
                    git_branch: current_git_branch(&runtime.context.workspace),
                    agent_type: agent_type.clone(),
                    assignment: assignment.clone(),
                    model: runtime.model.clone(),
                    nickname: None,
                    status,
                    worker_status: None,
                    parent_run_id: runtime.parent_agent_id.clone(),
                    spawn_depth: runtime.spawn_depth,
                    result: None,
                    steps_taken: steps,
                    checkpoint: latest_checkpoint.clone(),
                    needs_input: None,
                    duration_ms,
                    from_prior_session: false,
                });
            }
            api = request_subagent_model_response_with_retries(
                runtime,
                &agent_id,
                steps,
                max_steps,
                request,
            ) => {
                match api {
                    Ok(response) => response,
                    Err(SubAgentApiRequestFailure::Fatal(err)) => return Err(err),
                    Err(SubAgentApiRequestFailure::Interrupted { reason, checkpoint_reason }) => {
                        let checkpoint = checkpoint_subagent_progress(
                            runtime,
                            &agent_id,
                            checkpoint_reason,
                            &messages,
                            steps,
                            true,
                        )
                        .await;
                        record_agent_progress(
                            runtime,
                            &agent_id,
                            format!("{}: interrupted; {reason}", format_step_counter(steps, max_steps)),
                        );
                        let status = SubAgentStatus::Interrupted(reason.clone());
                        let duration_ms =
                            u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
                        insert_subagent_full_transcript_handle(
                            runtime,
                            &agent_id,
                            &agent_type,
                            &assignment,
                            &status,
                            Some(&reason),
                            Some(&checkpoint),
                            &messages,
                            steps,
                            duration_ms,
                            fork_context_enabled,
                        )
                        .await;
                        let needs_input =
                            needs_input_for_interrupted_checkpoint(&reason, &checkpoint);
                        let interrupted_snapshot = {
                            let mut manager = runtime.manager.write().await;
                            manager.interrupt_with_checkpoint(
                                &agent_id,
                                reason.clone(),
                                checkpoint.clone(),
                                Some(needs_input.clone()),
                            )?
                        };
                        record_agent_progress(
                            runtime,
                            &agent_id,
                            format!(
                                "{}: waiting for user; {}",
                                format_step_counter(steps, max_steps),
                                needs_input.question
                            ),
                        );
                        if let Some(mb) = runtime.mailbox.as_ref() {
                            let _ = mb.send(MailboxMessage::Interrupted {
                                agent_id: agent_id.clone(),
                                reason: reason.clone(),
                            });
                        }
                        return Ok(interrupted_snapshot);
                    }
                }
            }
        };

        let mut tool_uses = Vec::new();

        // 报告 token 使用情况，以便父级的成本计数器实时更新。
        if let Some(mb) = runtime.mailbox.as_ref() {
            let _ = mb.send(MailboxMessage::token_usage(
                &agent_id,
                response.model.clone(),
                response.usage.clone(),
            ));
        }
        {
            let mut manager = runtime.manager.write().await;
            manager.record_worker_usage(&agent_id, &response.usage);
        }

        // 每工作者 token 预算执行（#3321）：一旦单个失控工作者的累积模型 token 超过其自身上限，则停止。
        // 这补充——且不重复计数——作用域级别的准入门控（#3319），后者限制同级间的聚合扇出。
        // 本地累加器镜像管理器的 `record.usage.total_tokens`
        // （两者都源自 `response.usage`），因此作用域记账保持一致，且从不会被此检查夸大。
        tokens_used = tokens_used.saturating_add(usage_total_tokens(&response.usage));
        if let Some(budget) = token_budget
            && tokens_used > budget
        {
            record_agent_progress(
                runtime,
                &agent_id,
                format!(
                    "{}: token budget exhausted ({tokens_used}/{budget})",
                    format_step_counter(steps, max_steps)
                ),
            );
            if let Some(mb) = runtime.mailbox.as_ref() {
                let _ = mb.send(MailboxMessage::Cancelled {
                    agent_id: agent_id.clone(),
                });
            }
            let status = SubAgentStatus::BudgetExhausted;
            let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            latest_checkpoint = Some(
                checkpoint_subagent_progress(
                    runtime,
                    &agent_id,
                    "token_budget_exhausted",
                    &messages,
                    steps,
                    true,
                )
                .await,
            );
            insert_subagent_full_transcript_handle(
                runtime,
                &agent_id,
                &agent_type,
                &assignment,
                &status,
                final_result.as_ref(),
                latest_checkpoint.as_ref(),
                &messages,
                steps,
                duration_ms,
                fork_context_enabled,
            )
            .await;
            return Ok(SubAgentResult {
                name: agent_id.clone(),
                agent_id: agent_id.clone(),
                context_mode: if fork_context_enabled {
                    "forked"
                } else {
                    "fresh"
                }
                .to_string(),
                fork_context: fork_context_enabled,
                workspace: Some(runtime.context.workspace.clone()),
                git_branch: current_git_branch(&runtime.context.workspace),
                agent_type: agent_type.clone(),
                assignment: assignment.clone(),
                model: runtime.model.clone(),
                nickname: None,
                status,
                worker_status: None,
                parent_run_id: runtime.parent_agent_id.clone(),
                spawn_depth: runtime.spawn_depth,
                result: final_result.clone(),
                steps_taken: steps,
                checkpoint: latest_checkpoint.clone(),
                needs_input: None,
                duration_ms,
                from_prior_session: false,
            });
        }

        for block in &response.content {
            match block {
                ContentBlock::Text { text, .. } if !text.trim().is_empty() => {
                    final_result = Some(text.clone());
                }
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    tool_uses.push((id.clone(), name.clone(), input.clone()));
                }
                _ => {}
            }
        }

        messages.push(Message {
            role: "assistant".to_string(),
            content: response.content.clone(),
        });
        latest_checkpoint = Some(
            checkpoint_subagent_progress(
                runtime,
                &agent_id,
                "after_model_response",
                &messages,
                steps,
                true,
            )
            .await,
        );

        if response_was_truncated(&response) {
            final_result = None;
            record_truncated_subagent_response(&mut consecutive_truncated_responses)?;
            let progress = if tool_uses.is_empty() {
                "response truncated, returning retry instruction".to_string()
            } else {
                format!(
                    "response truncated, returning {} tool error(s)",
                    tool_uses.len()
                )
            };
            record_agent_progress(
                runtime,
                &agent_id,
                format!("{}: {progress}", format_step_counter(steps, max_steps)),
            );
            messages.push(Message {
                role: "user".to_string(),
                content: if tool_uses.is_empty() {
                    truncated_response_text_retry_message()
                } else {
                    truncated_response_tool_results(&tool_uses)
                },
            });
            latest_checkpoint = Some(
                checkpoint_subagent_progress(
                    runtime,
                    &agent_id,
                    "after_truncated_response_retry_message",
                    &messages,
                    steps,
                    true,
                )
                .await,
            );
            continue;
        }
        reset_truncated_subagent_responses(&mut consecutive_truncated_responses);

        if tool_uses.is_empty() {
            let child_completions = drain_child_completion_events(&mut child_completion_rx);
            if !child_completions.is_empty() {
                let count = child_completions.len();
                record_agent_progress(
                    runtime,
                    &agent_id,
                    format!(
                        "{}: resuming with {count} child sub-agent completion(s)",
                        format_step_counter(steps, max_steps)
                    ),
                );
                messages.push(child_completion_runtime_message(&child_completions));
                latest_checkpoint = Some(
                    checkpoint_subagent_progress(
                        runtime,
                        &agent_id,
                        "after_tail_child_subagent_completion",
                        &messages,
                        steps,
                        true,
                    )
                    .await,
                );
                continue;
            }
            while let Ok(input) = input_rx.try_recv() {
                if input.interrupt {
                    pending_inputs.clear();
                }
                pending_inputs.push_back(input);
            }
            if pending_inputs.is_empty() {
                record_agent_progress(
                    runtime,
                    &agent_id,
                    format!("{}: complete", format_step_counter(steps, max_steps)),
                );
                stopped_naturally = true;
                break;
            }
            continue;
        }

        record_agent_progress(
            runtime,
            &agent_id,
            format!(
                "{}: executing {} tool call(s)",
                format_step_counter(steps, max_steps),
                tool_uses.len()
            ),
        );
        let mut tool_results: Vec<ContentBlock> = Vec::new();
        for (tool_id, tool_name, tool_input) in tool_uses {
            let tool_display_name = subagent_progress_tool_display_name(&tool_name);
            record_agent_progress(
                runtime,
                &agent_id,
                format!(
                    "{}: running tool '{tool_display_name}'",
                    format_step_counter(steps, max_steps)
                ),
            );
            if let Some(mb) = runtime.mailbox.as_ref() {
                let _ = mb.send(MailboxMessage::ToolCallStarted {
                    agent_id: agent_id.clone(),
                    tool_name: tool_name.clone(),
                    step: steps,
                });
            }
            let result = match tokio::time::timeout(runtime.tool_timeout, async {
                tool_registry
                    .execute(&agent_id, &tool_name, tool_input)
                    .await
            })
            .await
            {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => format!("Error: {e}"),
                Err(_) => format!("Error: Tool {tool_name} timed out"),
            };
            let tool_ok = !result.starts_with("Error:");
            let (result, spilled_to) = bound_subagent_tool_result(&agent_id, &tool_id, result);
            if let Some(path) = spilled_to.as_ref() {
                record_agent_progress(
                    runtime,
                    &agent_id,
                    format!(
                        "{}: tool '{tool_display_name}' output spilled to {}",
                        format_step_counter(steps, max_steps),
                        path.display()
                    ),
                );
            }
            record_agent_progress(
                runtime,
                &agent_id,
                format!(
                    "{}: finished tool '{tool_display_name}'",
                    format_step_counter(steps, max_steps)
                ),
            );
            if let Some(mb) = runtime.mailbox.as_ref() {
                let _ = mb.send(MailboxMessage::ToolCallCompleted {
                    agent_id: agent_id.clone(),
                    tool_name: tool_name.clone(),
                    step: steps,
                    ok: tool_ok,
                });
            }

            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: tool_id,
                content: result,
                is_error: None,
                content_blocks: None,
            });
        }

        if !tool_results.is_empty() {
            messages.push(Message {
                role: "user".to_string(),
                content: tool_results,
            });
            latest_checkpoint = Some(
                checkpoint_subagent_progress(
                    runtime,
                    &agent_id,
                    "after_tool_results",
                    &messages,
                    steps,
                    true,
                )
                .await,
            );
        }
    }

    release_resident_leases_for(&agent_id);
    let has_final_summary = final_result
        .as_deref()
        .map(|text| !text.trim().is_empty())
        .unwrap_or(false);
    // #4050: 只有带有最终摘要的自然停止才是真正的成功。
    let status = if stopped_naturally {
        if has_final_summary {
            SubAgentStatus::Completed
        } else {
            SubAgentStatus::Failed(
                "child stopped without returning a final summary (its last turn produced no assistant text)".to_string(),
            )
        }
    } else {
        SubAgentStatus::Failed(format!(
            "child reached its step limit ({steps} steps) without returning a final summary"
        ))
    };
    let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    latest_checkpoint = Some(build_subagent_checkpoint(
        &agent_id,
        subagent_status_name(&status),
        &messages,
        steps,
        false,
    ));
    insert_subagent_full_transcript_handle(
        runtime,
        &agent_id,
        &agent_type,
        &assignment,
        &status,
        final_result.as_ref(),
        latest_checkpoint.as_ref(),
        &messages,
        steps,
        duration_ms,
        fork_context_enabled,
    )
    .await;

    Ok(SubAgentResult {
        name: agent_id.clone(),
        agent_id,
        context_mode: if fork_context_enabled {
            "forked"
        } else {
            "fresh"
        }
        .to_string(),
        fork_context: fork_context_enabled,
        workspace: Some(runtime.context.workspace.clone()),
        git_branch: current_git_branch(&runtime.context.workspace),
        agent_type,
        assignment,
        model: runtime.model.clone(),
        nickname: None,
        status,
        worker_status: None,
        parent_run_id: runtime.parent_agent_id.clone(),
        spawn_depth: runtime.spawn_depth,
        result: final_result,
        steps_taken: steps,
        checkpoint: latest_checkpoint,
        needs_input: None,
        duration_ms,
        from_prior_session: false,
    })
}

fn optional_input_str<'a>(input: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .filter_map(|key| input.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
}

fn parse_text_or_items(
    input: &Value,
    text_keys: &[&str],
    items_key: &str,
    required_field: &str,
) -> Result<String, ToolError> {
    let text = optional_input_str(input, text_keys).map(str::to_string);
    let items = parse_items_text(input, items_key)?;
    match (text, items) {
        (Some(_), Some(_)) => Err(ToolError::invalid_input(format!(
            "Provide either {required_field} text or {items_key}, but not both"
        ))),
        (Some(text), None) => Ok(text),
        (None, Some(items)) => Ok(items),
        (None, None) => Err(ToolError::missing_field(required_field)),
    }
}

fn parse_items_text(input: &Value, key: &str) -> Result<Option<String>, ToolError> {
    let Some(items) = input.get(key) else {
        return Ok(None);
    };
    let array = items
        .as_array()
        .ok_or_else(|| ToolError::invalid_input(format!("'{key}' must be an array")))?;
    if array.is_empty() {
        return Err(ToolError::invalid_input(format!("'{key}' cannot be empty")));
    }

    let mut lines = Vec::new();
    for item in array {
        let object = item
            .as_object()
            .ok_or_else(|| ToolError::invalid_input("each item must be an object"))?;
        let item_type = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("text")
            .trim();
        let rendered = match item_type {
            "text" => object
                .get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .ok_or_else(|| ToolError::invalid_input("text item requires non-empty text"))?,
            "mention" => {
                let name = object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::invalid_input("mention item requires name"))?;
                let path = object
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::invalid_input("mention item requires path"))?;
                format!("[mention:${name}]({path})")
            }
            "skill" => {
                let name = object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::invalid_input("skill item requires name"))?;
                let path = object
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::invalid_input("skill item requires path"))?;
                format!("[skill:${name}]({path})")
            }
            "local_image" => {
                let path = object
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::invalid_input("local_image item requires path"))?;
                format!("[local_image:{path}]")
            }
            "image" => {
                let url = object
                    .get("image_url")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::invalid_input("image item requires image_url"))?;
                format!("[image:{url}]")
            }
            _ => object
                .get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| "[input]".to_string()),
        };
        lines.push(rendered);
    }

    Ok(Some(lines.join("\n")))
}

fn parse_spawn_request(input: &Value) -> Result<SpawnRequest, ToolError> {
    let prompt = parse_text_or_items(
        input,
        &["prompt", "message", "objective"],
        "items",
        "prompt",
    )?;
    let session_name = optional_input_str(input, &["name", "session_name"])
        .map(validate_session_name)
        .transpose()?;

    let type_input = optional_input_str(input, &["type", "agent_type", "agent_name"]);
    let role_input = optional_input_str(input, &["role", "agent_role"]);

    let parsed_type = type_input
        .map(|kind| {
            SubAgentType::from_str(kind).ok_or_else(|| {
                ToolError::invalid_input(format!(
                    "Invalid sub-agent type '{kind}'. Use: {VALID_SUBAGENT_TYPES}"
                ))
            })
        })
        .transpose()?;

    // Role 可以是 SubAgentType 别名（reviewer → Review）或舰队名册角色/成员 id（scout, release_lead）。
    // 类型别名仍然设置 agent_type；非别名角色遵循舰队配置文件解析（#4177）。
    let parsed_role_type = role_input.and_then(SubAgentType::from_str);

    if let (Some(type_kind), Some(role_kind)) = (&parsed_type, &parsed_role_type)
        && type_kind != role_kind
    {
        return Err(ToolError::invalid_input(
            "Conflicting type/agent_type and role/agent_role values".to_string(),
        ));
    }

    let agent_type_explicit = parsed_type.is_some() || parsed_role_type.is_some();
    let agent_type = parsed_type
        .or(parsed_role_type)
        .unwrap_or(SubAgentType::General);

    let role_alias = role_input
        .and_then(normalize_role_alias)
        .or_else(|| type_input.and_then(normalize_role_alias))
        .map(str::to_string);

    // 舰队角色 token：要么是非类型别名的原始角色，要么是用作名册查找键的别名形式（例如 implementer）。
    let fleet_role_token = match role_input {
        Some(raw) => {
            let token = validate_role_name(raw)?;
            Some(token)
        }
        None => None,
    };

    let role = role_alias.or_else(|| fleet_role_token.clone()).or_else(|| {
        type_input
            .and_then(normalize_role_alias)
            .map(str::to_string)
    });

    let mut profile = optional_input_str(input, &["profile", "fleet_profile", "roster_profile"])
        .map(validate_profile_name)
        .transpose()?;
    // 当调用方仅声明了舰队角色时，将其用作配置文件键，以便 `apply_spawn_profile` 是单一的名册解析路径（#4177）。
    if profile.is_none() {
        profile = fleet_role_token.clone();
    }

    let allowed_tools = input
        .get("allowed_tools")
        .and_then(|v| v.as_array())
        .map(|items| {
            let mut tools = Vec::new();
            for item in items {
                if let Some(tool) = item.as_str() {
                    let trimmed = tool.trim();
                    if !trimmed.is_empty() && !tools.iter().any(|existing| existing == trimmed) {
                        tools.push(trimmed.to_string());
                    }
                }
            }
            tools
        });

    let cwd = parse_optional_cwd(input)?;
    let worktree = parse_optional_worktree_request(input)?;
    let model = parse_optional_subagent_model(input, "model")?;
    let explicit_model_strength = optional_input_str(input, &["model_strength", "modelStrength"])
        .map(SubAgentModelStrength::parse)
        .transpose()?;
    let model_strength_explicit = explicit_model_strength.is_some();
    let model_strength = explicit_model_strength.unwrap_or_else(|| {
        // 默认模型强度。`type: "explore"` 默认为 Faster，用于有限的只读查找/搜索/状态工作——
        // 廉价、快速的同族兄弟模型正是子级应该运行的有损广度任务。
        // 其他所有角色（以及提供显式 `model` 的任何调用）保持保守的 Same。
        // 上面的显式 model_strength 已通过 `.parse()` 获胜；显式 `model` 在下游的 assignment_model_route 中获胜，无论强度如何。
        if agent_type == SubAgentType::Explore && model.is_none() {
            SubAgentModelStrength::Faster
        } else {
            SubAgentModelStrength::Same
        }
    });
    let thinking = optional_input_str(input, &["thinking", "reasoning_effort", "reasoningEffort"])
        .map(SubAgentThinking::parse)
        .transpose()?
        .unwrap_or(SubAgentThinking::Inherit);
    let resident_file = input
        .get("resident_file")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    let fork_context =
        parse_optional_bool(input, &["fork_context", "forkContext", "inherit_context"])
            .unwrap_or(false);
    let max_depth = input
        .get("max_depth")
        .or_else(|| input.get("maxDepth"))
        .or_else(|| input.get("max_spawn_depth"))
        .and_then(Value::as_u64)
        .map(|depth| {
            let ceiling = codewhale_config::MAX_SPAWN_DEPTH_CEILING;
            u32::try_from(depth)
                .map_err(|_| {
                    ToolError::invalid_input(format!("max_depth must be between 0 and {ceiling}"))
                })
                .and_then(|depth| {
                    if depth <= ceiling {
                        Ok(depth)
                    } else {
                        Err(ToolError::invalid_input(format!(
                            "max_depth must be between 0 and {ceiling}"
                        )))
                    }
                })
        })
        .transpose()?;
    let token_budget =
        parse_optional_positive_u64(input, &["token_budget", "tokenBudget", "max_tokens"])?;

    // #4042: 可选的调用方提供的工具拒绝列表（与父级的继承拒绝列表合并）和继承退出标志（默认继承）。
    let disallowed_tools = parse_disallowed_tools(input)?;
    let inherit_disallowed_tools = parse_optional_bool(
        input,
        &["inherit_disallowed_tools", "inheritDisallowedTools"],
    )
    .unwrap_or(true);

    Ok(SpawnRequest {
        session_name,
        prompt: prompt.clone(),
        agent_type,
        agent_type_explicit,
        profile,
        assignment: SubAgentAssignment::new(prompt, role),
        allowed_tools,
        model,
        model_strength,
        model_strength_explicit,
        thinking,
        cwd,
        worktree,
        resident_file,
        fork_context,
        max_depth,
        token_budget,
        disallowed_tools,
        inherit_disallowed_tools,
    })
}

fn validate_session_name(name: &str) -> Result<String, ToolError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid_input("name cannot be blank"));
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(ToolError::invalid_input(
            "name must not contain whitespace; use letters, numbers, '-', '_', or '.'",
        ));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(ToolError::invalid_input(
            "name may only contain ASCII letters, numbers, '-', '_', or '.'",
        ));
    }
    Ok(trimmed.to_string())
}

/// 验证并标准化 `profile` 生成参数：裸名册成员 id token
/// （与舰队模型/配置文件 token 相同规则——可见字符，无空格、引号、反引号或 '='），
/// 小写化以进行名册的不区分大小写查找。
fn validate_profile_name(value: &str) -> Result<String, ToolError> {
    validate_roster_token(value, "profile")
}

fn validate_role_name(value: &str) -> Result<String, ToolError> {
    validate_roster_token(value, "role")
}

fn validate_roster_token(value: &str, field: &str) -> Result<String, ToolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid_input(format!("{field} cannot be blank")));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_graphic() && !matches!(ch, '"' | '\'' | '`' | '='))
    {
        return Err(ToolError::invalid_input(format!(
            "{field} must be a bare roster member id without whitespace, quotes, backticks, or '='"
        )));
    }
    Ok(trimmed.to_ascii_lowercase())
}

/// 根据舰队名册解析 `profile` 生成参数，并将成员合并到请求中：
/// 代理类型（未显式给出时）、分配角色以及配置文件指令覆盖到子级提示词上。
///
/// 在生成时运行——`parse_spawn_request` 没有运行时访问权限。
/// 返回已解析的成员，以便生成路径可以应用其模型路由和委托边界。
/// 成员的 `permissions` 块有意不在此处使用：它默认为下限（无 shell、无信任、需要批准），
/// 而子级的能力姿态由成员的 `SubAgentType` 通过 `WorkerRuntimeProfile::for_role` 控制——
/// 在此处应用该块只会扩大姿态。
fn apply_spawn_profile(
    request: &mut SpawnRequest,
    roster: &crate::fleet::roster::FleetRoster,
) -> Result<Option<crate::fleet::profile::AgentProfile>, ToolError> {
    let Some(profile_id) = request.profile.as_deref() else {
        return Ok(None);
    };
    let Some(member) = resolve_roster_member(roster, profile_id) else {
        let available = roster
            .members()
            .iter()
            .map(|member| member.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(ToolError::invalid_input(format!(
            "Unknown fleet role/profile '{profile_id}'. Available fleet roster members: {available}. \
             Type aliases: {VALID_ROLE_ALIASES}. See /fleet."
        )));
    };

    let member_type = crate::fleet::worker_runtime::roster_member_agent_type(member);
    if request.agent_type_explicit && request.agent_type != member_type {
        return Err(ToolError::invalid_input(format!(
            "profile '{}' implies type {}; conflicting explicit type '{}'",
            member.id,
            member_type.as_str(),
            request.agent_type.as_str()
        )));
    }
    request.agent_type = member_type;
    // 在角色→配置文件解析后记录规范配置文件 id。
    request.profile = Some(member.id.clone());

    // 在提示词和分类账记录中展示成员的角色。
    let role_name = member.profile.role.name.trim();
    request.assignment.role = Some(if role_name.is_empty() {
        member.id.clone()
    } else {
        role_name.to_string()
    });

    if let Some(overlay) = spawn_profile_prompt_overlay(member) {
        request.prompt.push_str(&overlay);
    }

    Ok(Some(member.clone()))
}

/// 根据名册解析舰队角色或配置文件 token（#4177）。
///
/// 查找顺序：
/// 1. 成员 id（不区分大小写）
/// 2. 成员角色名称
/// 3. 常见的 stopship 别名（`implementer` → `builder`，`release_lead` → `manager`）
fn resolve_roster_member<'a>(
    roster: &'a crate::fleet::roster::FleetRoster,
    id_or_role: &str,
) -> Option<&'a crate::fleet::profile::AgentProfile> {
    let key = id_or_role.trim();
    if key.is_empty() {
        return None;
    }
    if let Some(member) = roster.get(key) {
        return Some(member);
    }
    if let Some(member) = roster
        .members()
        .iter()
        .find(|member| member.profile.role.name.trim().eq_ignore_ascii_case(key))
    {
        return Some(member);
    }
    let alias = match key.to_ascii_lowercase().as_str() {
        "implementer" | "implement" | "implementation" => Some("builder"),
        "release_lead" | "release-lead" | "releaselead" => Some("manager"),
        "scout" | "explore" | "explorer" | "exploration" => Some("scout"),
        _ => None,
    };
    alias.and_then(|id| roster.get(id))
}

/// 附加到子级提示词的紧凑配置文件块，镜像舰队调度器的 `fleet_task_prompt_with_profile` 覆盖。
/// 当成员没有描述或指令时（内置项：仅姿态通过类型系统提示词表达），为 `None`。
fn spawn_profile_prompt_overlay(member: &crate::fleet::profile::AgentProfile) -> Option<String> {
    let description = member.description.as_deref().map(str::trim);
    let instructions = member.profile.role.instructions.as_deref().map(str::trim);
    if description.is_none_or(str::is_empty) && instructions.is_none_or(str::is_empty) {
        return None;
    }
    let mut overlay = String::new();
    overlay.push_str("\n\nFleet profile: ");
    overlay.push_str(&member.id);
    if let Some(display_name) = member.display_name.as_deref() {
        overlay.push_str(" (");
        overlay.push_str(display_name);
        overlay.push(')');
    }
    if let Some(description) = description.filter(|text| !text.is_empty()) {
        overlay.push_str("\nProfile description:\n");
        overlay.push_str(description);
    }
    if let Some(instructions) = instructions.filter(|text| !text.is_empty()) {
        overlay.push_str("\nProfile instructions:\n");
        overlay.push_str(instructions);
    }
    Some(overlay)
}

/// 生成的请求模型路由，遵循舰队配置文件优先级：
/// 显式 `model` 参数 > 显式 `model_strength` 参数 > 成员模型固定（Fixed）> 成员装载 > 解析时默认值。
/// 显式的 `model` 仍然通过配置模型路径（将其转换为 Fixed 路由）在下游获胜，
/// `role_models` 覆盖（如果匹配）也是如此。
fn spawn_model_route(
    request: &SpawnRequest,
    member: Option<&crate::fleet::profile::AgentProfile>,
) -> ModelRoute {
    let Some(member) = member else {
        return request.model_strength.model_route();
    };
    if request.model.is_some() || request.model_strength_explicit {
        return request.model_strength.model_route();
    }
    if let Some(model) = member
        .profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("auto"))
    {
        return ModelRoute::Fixed(model.to_string());
    }
    match member.profile.loadout {
        codewhale_config::FleetLoadout::Fast => ModelRoute::Faster,
        // Inherit 和更丰富的装载类（strong/balanced/...）都
        // 在此处继承父级模型。舰队调度器将这些类映射到 Auto，
        // 但在此接缝中 Auto 路由到廉价兄弟模型——例如对"strong"成员的静默降级。
        // 使用 Inherit 保持现有的非配置文件默认行为。
        _ => ModelRoute::Inherit,
    }
}

/// 子级的有效绝对 `max_spawn_depth`，结合继承的运行时预算、调用方的 `max_depth` 请求
/// 和舰队配置文件的 `delegation.max_spawn_depth` 提示。
/// 显式请求保持其现有语义（可能扩大到上限）；配置文件提示仅缩小——
/// 要么是请求（min），要么是继承的预算。
fn child_max_spawn_depth_for_spawn(
    inherited: u32,
    child_spawn_depth: u32,
    requested: Option<u32>,
    profile_hint: Option<u32>,
) -> u32 {
    match (requested, profile_hint) {
        (Some(requested), hint) => {
            let depth = hint.map_or(requested, |hint| requested.min(hint));
            clamp_child_max_spawn_depth(child_spawn_depth, depth)
        }
        (None, Some(hint)) => inherited.min(clamp_child_max_spawn_depth(child_spawn_depth, hint)),
        (None, None) => inherited,
    }
}

fn parse_optional_bool(input: &Value, names: &[&str]) -> Option<bool> {
    names
        .iter()
        .find_map(|name| input.get(*name))
        .and_then(Value::as_bool)
}

/// 解析可选的调用方提供的 `disallowed_tools` 数组（#4042）。镜像 `allowed_tools` 解析：
/// 修剪、去重、仅非空。当键缺失或未产生可用条目时返回 `None`，
/// 以便 `spawn_subagent_from_input` 中的并集合并仅在有内容要添加时运行。
fn parse_disallowed_tools(input: &Value) -> Result<Option<Vec<String>>, ToolError> {
    let Some(array) = input.get("disallowed_tools").and_then(Value::as_array) else {
        return Ok(None);
    };
    let mut tools = Vec::new();
    for item in array {
        let Some(tool) = item.as_str() else {
            continue;
        };
        let trimmed = tool.trim();
        if !trimmed.is_empty() && !tools.iter().any(|existing: &String| existing == trimmed) {
            tools.push(trimmed.to_string());
        }
    }
    if tools.is_empty() {
        Ok(None)
    } else {
        Ok(Some(tools))
    }
}

fn parse_optional_positive_u64(input: &Value, names: &[&str]) -> Result<Option<u64>, ToolError> {
    for name in names {
        let Some(value) = input.get(*name) else {
            continue;
        };
        let Some(parsed) = value.as_u64() else {
            return Err(ToolError::invalid_input(format!(
                "{name} must be a positive integer token count"
            )));
        };
        if parsed == 0 {
            return Err(ToolError::invalid_input(format!(
                "{name} must be greater than zero; omit it to inherit or disable the budget"
            )));
        }
        return Ok(Some(parsed));
    }
    Ok(None)
}

#[cfg(test)]
fn with_default_fork_context(mut input: Value, default: bool) -> Value {
    let Some(object) = input.as_object_mut() else {
        return input;
    };
    if !object.contains_key("fork_context")
        && !object.contains_key("forkContext")
        && !object.contains_key("inherit_context")
    {
        object.insert("fork_context".to_string(), Value::Bool(default));
    }
    input
}

pub(crate) fn normalize_requested_subagent_model(
    value: &str,
    field: &str,
    provider: crate::config::ApiProvider,
) -> Result<String, ToolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid_input(format!("{field} cannot be blank")));
    }
    // #3018: 使用提供商感知验证，以便非 DeepSeek 提供商可以接受自己的模型 id，
    // 而不是以"Expected a DeepSeek model id"失败。
    let normalized =
        crate::config::requested_model_for_provider(provider, trimmed).ok_or_else(|| {
            let valid_names = crate::provider_lake::all_catalog_models_for_provider(provider);
            let valid_hint = if valid_names.is_empty() {
                String::new()
            } else {
                format!(" (accepted: {})", valid_names.join(", "))
            };
            ToolError::invalid_input(format!(
                "Invalid {field} '{trimmed}' for provider {}{valid_hint}",
                provider_name_for_error(provider)
            ))
        })?;
    crate::config::validate_route(provider, &normalized).map_err(ToolError::invalid_input)?;
    Ok(normalized)
}

fn provider_name_for_error(provider: crate::config::ApiProvider) -> &'static str {
    // 复用规范的选取器/状态标签，以便每个提供商都被具体命名（DeepSeek, Sakana, Zhipu, …），
    // 而不是将长尾折叠为"this provider"，并且错误文本与模型选取器标签保持一致（#4049）。
    provider.display_name()
}

pub(crate) fn configured_model_for_role_or_type(
    runtime: &SubAgentRuntime,
    role: Option<&str>,
    agent_type: &SubAgentType,
) -> Result<Option<String>, ToolError> {
    let mut keys = Vec::new();
    if let Some(role) = role.map(str::trim).filter(|role| !role.is_empty()) {
        keys.push(role.to_ascii_lowercase());
    }
    keys.push(agent_type.as_str().to_string());
    keys.push("default".to_string());

    for key in keys {
        if let Some(model) = runtime.role_models.get(&key) {
            return normalize_requested_subagent_model(
                model,
                &format!("subagents.{key}.model"),
                runtime.client.api_provider(),
            )
            .map(Some);
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubAgentResolvedRoute {
    pub(crate) model_route: ModelRoute,
    pub(crate) model: String,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) tuning: RequestTuning,
}

impl SubAgentResolvedRoute {
    fn new(
        model_route: ModelRoute,
        model: String,
        reasoning_effort: Option<String>,
    ) -> SubAgentResolvedRoute {
        let tuning = subagent_request_tuning(reasoning_effort.as_deref());
        SubAgentResolvedRoute {
            model_route,
            model,
            reasoning_effort,
            tuning,
        }
    }
}

pub(crate) async fn resolve_subagent_assignment_route(
    runtime: &SubAgentRuntime,
    configured_model: Option<String>,
    prompt: &str,
    agent_type: &SubAgentType,
    requested_model_route: ModelRoute,
    requested_thinking: SubAgentThinking,
) -> SubAgentResolvedRoute {
    let model_route = assignment_model_route(configured_model.as_deref(), requested_model_route);
    worker_profile_subagent_assignment_route(
        runtime,
        &model_route,
        requested_thinking,
        prompt,
        agent_type,
    )
}

fn assignment_model_route(
    configured_model: Option<&str>,
    requested_model_route: ModelRoute,
) -> ModelRoute {
    if let Some(model) = configured_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        return ModelRoute::Fixed(model.to_string());
    }

    requested_model_route
}

fn subagent_request_tuning(reasoning_effort: Option<&str>) -> RequestTuning {
    RequestTuning {
        reasoning_effort: reasoning_effort.map(ReasoningEffort::from_setting),
        max_output_tokens: Some(SUBAGENT_RESPONSE_MAX_TOKENS),
    }
}

/// 显式子代理强度路由的候选对，源自活跃提供商和已经提供商解析的父级模型。
fn subagent_router_candidates(runtime: &SubAgentRuntime) -> crate::model_routing::RouterCandidates {
    crate::model_routing::provider_router_candidates(runtime.client.api_provider(), &runtime.model)
}

#[cfg(test)]
fn fallback_subagent_assignment_route(
    runtime: &SubAgentRuntime,
    configured_model: Option<String>,
    requested_model_route: ModelRoute,
    requested_thinking: SubAgentThinking,
    prompt: &str,
) -> SubAgentResolvedRoute {
    let model_route = assignment_model_route(configured_model.as_deref(), requested_model_route);
    worker_profile_subagent_assignment_route(
        runtime,
        &model_route,
        requested_thinking,
        prompt,
        &SubAgentType::General,
    )
}

/// 当继承/更快路由不得跨命名空间时，用于活跃提供商的算子可见模型
/// （#3227，子代理路由验证 2026-07-07）。
///
/// 通过目录支持的 [`crate::provider_lake`] 门面枚举，而非原始的遗留
/// `model_completion_names_for_provider` 表（#4116 / #4188）。
/// 门面优先使用实时的 Models.dev，然后是离线捆绑快照，
/// 最后才是仅 CodeWhale/未捆绑提供商的遗留硬编码表。
/// 此消费者仅读取第一个条目。
fn operator_model_for_subagent(runtime: &SubAgentRuntime) -> String {
    let provider = runtime.client.api_provider();
    if crate::config::validate_route(provider, &runtime.model).is_ok() {
        return runtime.model.clone();
    }
    crate::provider_lake::all_catalog_models_for_provider(provider)
        .into_iter()
        .next()
        .unwrap_or_else(|| runtime.model.clone())
}

/// 拒绝或重新映射解析的子代理模型，使其在生成前与运行时提供商匹配。
/// 显式固定固定快速失败；继承/更快/自动回退到算子路由，而非跨接线命名空间。
pub(crate) fn ensure_subagent_model_for_provider(
    runtime: &SubAgentRuntime,
    model_route: &ModelRoute,
    model: String,
) -> Result<String, ToolError> {
    let provider = runtime.client.api_provider();
    if crate::config::validate_route(provider, &model).is_ok() {
        return Ok(model);
    }
    match model_route {
        ModelRoute::Inherit | ModelRoute::Faster | ModelRoute::Auto => {
            Ok(operator_model_for_subagent(runtime))
        }
        ModelRoute::Fixed(_) => Err(ToolError::invalid_input(
            crate::config::validate_route(provider, &model).unwrap_err(),
        )),
    }
}

fn worker_profile_subagent_assignment_route(
    runtime: &SubAgentRuntime,
    model_route: &ModelRoute,
    requested_thinking: SubAgentThinking,
    prompt: &str,
    _agent_type: &SubAgentType,
) -> SubAgentResolvedRoute {
    let candidates = subagent_router_candidates(runtime);
    let mut requested_fast_lane = false;
    let model = match model_route {
        ModelRoute::Fixed(model) => model.clone(),
        ModelRoute::Faster | ModelRoute::Auto => {
            requested_fast_lane = true;
            candidates
                .cheap
                .clone()
                .unwrap_or_else(|| runtime.model.clone())
        }
        ModelRoute::Inherit => runtime.model.clone(),
    };

    let reasoning_effort = subagent_reasoning_effort_for_request(
        runtime,
        prompt,
        requested_fast_lane,
        requested_thinking,
    );

    SubAgentResolvedRoute::new(model_route.clone(), model, reasoning_effort)
}

fn subagent_reasoning_effort_for_request(
    runtime: &SubAgentRuntime,
    prompt: &str,
    requested_fast_lane: bool,
    requested_thinking: SubAgentThinking,
) -> Option<String> {
    match requested_thinking {
        SubAgentThinking::Effort(effort) => Some(effort.as_setting().to_string()),
        SubAgentThinking::Auto => Some(
            auto_subagent_reasoning_effort(prompt)
                .as_setting()
                .to_string(),
        ),
        SubAgentThinking::Inherit if requested_fast_lane => {
            // 更快/探索通道：默认更便宜的推理。OpenAI Codex（GPT-5.5）适配器
            // 在线缆上没有真正的"off"（它将 off 折叠为 low），
            // 因此我们诚实地为该提供商解析 Low，而不是发出一个被静默重写的 off。
            // 调用方传递的显式 thinking 已经通过上面的分支获胜。
            let provider = runtime.client.api_provider();
            let effort = if matches!(provider, crate::config::ApiProvider::OpenaiCodex) {
                ReasoningEffort::Low
            } else {
                ReasoningEffort::Off
            };
            Some(effort.as_setting().to_string())
        }
        SubAgentThinking::Inherit => fallback_subagent_reasoning_effort(runtime, prompt),
    }
}

fn fallback_subagent_reasoning_effort(runtime: &SubAgentRuntime, prompt: &str) -> Option<String> {
    if runtime.reasoning_effort_auto {
        Some(
            auto_subagent_reasoning_effort(prompt)
                .as_setting()
                .to_string(),
        )
    } else {
        runtime.reasoning_effort.clone()
    }
}

fn auto_subagent_reasoning_effort(prompt: &str) -> ReasoningEffort {
    match crate::auto_reasoning::select(false, prompt) {
        ReasoningEffort::Low | ReasoningEffort::Medium => ReasoningEffort::High,
        other => other,
    }
}

fn parse_optional_subagent_model(input: &Value, key: &str) -> Result<Option<String>, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(ToolError::invalid_input(format!("{key} cannot be blank")));
            }
            // #3018: 仅基本解析——提供商感知验证延迟到生成路径中，
            // 其中运行时的 ApiProvider 可用。
            Ok(Some(trimmed.to_string()))
        }
        Some(_) => Err(ToolError::invalid_input(format!("{key} must be a string"))),
    }
}

/// 从生成输入中提取可选的 `cwd: String` 并转换为 `PathBuf`。
/// 空/缺失 → `None`。工作区边界检查在生成时发生（父级的工作区在那里已知，而非在此处）。
fn parse_optional_cwd(input: &Value) -> Result<Option<PathBuf>, ToolError> {
    let raw = input.get("cwd").and_then(|v| v.as_str()).map(str::trim);
    match raw {
        None | Some("") => Ok(None),
        Some(s) => Ok(Some(PathBuf::from(s))),
    }
}

fn parse_optional_worktree_request(
    input: &Value,
) -> Result<Option<SubAgentWorktreeRequest>, ToolError> {
    let worktree_flag =
        parse_optional_bool_strict(input, &["worktree", "isolate_worktree", "isolateWorktree"])?;
    let isolation = optional_input_str(input, &["isolation"])
        .map(|value| value.trim().to_ascii_lowercase().replace(['_', '-'], ""));
    let isolation_wants_worktree = match isolation.as_deref() {
        None | Some("") | Some("none") | Some("shared") => false,
        Some("worktree") | Some("gitworktree") => true,
        Some(other) => {
            return Err(ToolError::invalid_input(format!(
                "isolation must be 'worktree' or 'none' (got '{other}')"
            )));
        }
    };

    let branch = optional_input_str(
        input,
        &[
            "worktree_branch",
            "worktreeBranch",
            "branch_name",
            "branchName",
            "branch",
        ],
    )
    .map(str::to_string);
    let path = optional_input_str(
        input,
        &[
            "worktree_path",
            "worktreePath",
            "worktree_dir",
            "worktreeDir",
        ],
    )
    .map(PathBuf::from);
    let base_ref = optional_input_str(
        input,
        &["worktree_base", "worktreeBase", "base_ref", "baseRef"],
    )
    .map(str::to_string);

    let has_worktree_details = branch.is_some() || path.is_some() || base_ref.is_some();
    if worktree_flag == Some(false) && (isolation_wants_worktree || has_worktree_details) {
        return Err(ToolError::invalid_input(
            "worktree=false conflicts with worktree isolation options".to_string(),
        ));
    }
    if worktree_flag.unwrap_or(false) || isolation_wants_worktree || has_worktree_details {
        Ok(Some(SubAgentWorktreeRequest {
            branch,
            path,
            base_ref,
        }))
    } else {
        Ok(None)
    }
}

fn parse_optional_bool_strict(input: &Value, names: &[&str]) -> Result<Option<bool>, ToolError> {
    for name in names {
        let Some(value) = input.get(*name) else {
            continue;
        };
        return value.as_bool().map(Some).ok_or_else(|| {
            ToolError::invalid_input(format!("{name} must be a boolean when provided"))
        });
    }
    Ok(None)
}

fn prepare_child_workspace(
    parent_workspace: &Path,
    request: &SpawnRequest,
) -> Result<Option<PathBuf>, ToolError> {
    let discovery_anchor = if let Some(requested_cwd) = request.cwd.as_ref() {
        validate_existing_child_cwd(parent_workspace, requested_cwd)?
    } else {
        parent_workspace
            .canonicalize()
            .unwrap_or_else(|_| parent_workspace.to_path_buf())
    };

    if let Some(worktree) = request.worktree.as_ref() {
        return create_isolated_worktree(
            &discovery_anchor,
            worktree,
            request.session_name.as_deref(),
            &request.agent_type,
        )
        .map(Some);
    }

    if request.cwd.is_some() {
        return Ok(Some(discovery_anchor));
    }

    Ok(None)
}

fn validate_existing_child_cwd(
    parent_workspace: &Path,
    requested_cwd: &Path,
) -> Result<PathBuf, ToolError> {
    let resolved = if requested_cwd.is_absolute() {
        requested_cwd.to_path_buf()
    } else {
        parent_workspace.join(requested_cwd)
    };
    let canonical = resolved.canonicalize().map_err(|e| {
        ToolError::invalid_input(format!(
            "Invalid cwd '{}': {e} (path may not exist yet — use worktree=true to let CodeWhale create an isolated checkout)",
            requested_cwd.display()
        ))
    })?;
    let workspace_canonical = parent_workspace
        .canonicalize()
        .unwrap_or_else(|_| parent_workspace.to_path_buf());
    if !canonical.starts_with(&workspace_canonical) {
        return Err(ToolError::invalid_input(format!(
            "cwd must be inside the parent workspace: {} is not under {}",
            canonical.display(),
            workspace_canonical.display()
        )));
    }
    Ok(canonical)
}

fn create_isolated_worktree(
    parent_workspace: &Path,
    request: &SubAgentWorktreeRequest,
    session_name: Option<&str>,
    agent_type: &SubAgentType,
) -> Result<PathBuf, ToolError> {
    let repo_root = git_repo_root(parent_workspace)?;
    let branch = request
        .branch
        .clone()
        .unwrap_or_else(|| default_worktree_branch(session_name, agent_type));
    validate_git_branch_name(&repo_root, &branch)?;

    let base_ref = request
        .base_ref
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("HEAD")
        .to_string();
    let worktree_path = resolve_worktree_path(&repo_root, &branch, request.path.as_ref())?;
    if let Some(parent) = worktree_path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            ToolError::execution_failed(format!(
                "Failed to create worktree parent '{}': {err}",
                parent.display()
            ))
        })?;
    }

    let path_arg = worktree_path.to_string_lossy().to_string();
    let args = vec![
        "worktree".to_string(),
        "add".to_string(),
        "-b".to_string(),
        branch,
        path_arg,
        base_ref,
    ];
    run_git_checked(&repo_root, &args, "create sub-agent worktree")?;
    worktree_path.canonicalize().map_err(|err| {
        ToolError::execution_failed(format!(
            "Created worktree path '{}' could not be resolved: {err}",
            worktree_path.display()
        ))
    })
}

fn git_repo_root(workspace: &Path) -> Result<PathBuf, ToolError> {
    const MAX_PARENT_LEVELS: usize = 4;
    let start = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut paths_tried = Vec::new();
    let mut current = Some(start.as_path());
    let mut levels = 0usize;

    while let Some(dir) = current {
        paths_tried.push(dir.display().to_string());

        if let Some(root) = try_git_toplevel(dir) {
            return Ok(root);
        }

        if let Ok(entries) = fs::read_dir(dir) {
            let mut nested_roots = Vec::new();
            for entry in entries.flatten() {
                let child = entry.path();
                if !child.is_dir() || !path_looks_like_git_checkout(&child) {
                    continue;
                }
                if child
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'))
                {
                    continue;
                }
                if let Some(root) = try_git_toplevel(&child) {
                    nested_roots.push(root);
                }
            }
            match nested_roots.len() {
                0 => {}
                1 => return Ok(nested_roots.into_iter().next().expect("single nested root")),
                _ => {
                    let repos = nested_roots
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(ToolError::invalid_input(format!(
                        "Multiple git repositories found under {}. Specify cwd to disambiguate: {repos}",
                        dir.display()
                    )));
                }
            }
        }

        levels += 1;
        if levels > MAX_PARENT_LEVELS {
            break;
        }
        current = dir.parent();
    }

    Err(ToolError::invalid_input(format!(
        "worktree=true requires a git repository. Tried: {}",
        paths_tried.join(", ")
    )))
}

fn path_looks_like_git_checkout(path: &Path) -> bool {
    let git_path = path.join(".git");
    git_path.is_dir() || git_path.is_file()
}

fn try_git_toplevel(path: &Path) -> Option<PathBuf> {
    let output = Git::output(&["rev-parse", "--show-toplevel"], path).ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        None
    } else {
        Some(PathBuf::from(root))
    }
}

fn validate_git_branch_name(repo_root: &Path, branch: &str) -> Result<(), ToolError> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(ToolError::invalid_input(
            "worktree_branch cannot be blank".to_string(),
        ));
    }
    run_git_checked(
        repo_root,
        &[
            "check-ref-format".to_string(),
            "--branch".to_string(),
            branch.to_string(),
        ],
        "validate sub-agent worktree branch",
    )
    .map(|_| ())
    .map_err(|err| ToolError::invalid_input(format!("Invalid worktree_branch '{branch}': {err}")))
}

fn default_worktree_branch(session_name: Option<&str>, agent_type: &SubAgentType) -> String {
    let seed = session_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| agent_type.as_str());
    format!(
        "codex/agent-{}-{}",
        sanitize_worktree_slug(seed),
        &Uuid::new_v4().to_string()[..8]
    )
}

fn resolve_worktree_path(
    repo_root: &Path,
    branch: &str,
    requested_path: Option<&PathBuf>,
) -> Result<PathBuf, ToolError> {
    let default_root = default_worktree_root(repo_root);
    let path = match requested_path {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => {
            let resolved = normalize_path_lexically(&default_root.join(path));
            if !resolved.starts_with(&default_root) {
                return Err(ToolError::invalid_input(format!(
                    "relative worktree_path '{}' must stay under {}",
                    path.display(),
                    default_root.display()
                )));
            }
            resolved
        }
        None => default_root.join(sanitize_worktree_slug(branch)),
    };
    let normalized = normalize_path_lexically(&path);
    let repo_canonical = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    if normalized.starts_with(&repo_canonical) {
        return Err(ToolError::invalid_input(format!(
            "worktree_path must not be inside the parent checkout: {} is under {}",
            normalized.display(),
            repo_canonical.display()
        )));
    }
    Ok(normalized)
}

fn default_worktree_root(repo_root: &Path) -> PathBuf {
    let repo_name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(sanitize_worktree_slug)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "repo".to_string());
    let parent = repo_root.parent().unwrap_or(repo_root);
    normalize_path_lexically(&parent.join(SUBAGENT_WORKTREE_ROOT_DIR).join(repo_name))
}

fn sanitize_worktree_slug(input: &str) -> String {
    let mut slug = String::new();
    for ch in input.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else if matches!(ch, '-' | '_' | '.') {
            ch
        } else {
            '-'
        };
        if normalized == '-' && slug.ends_with('-') {
            continue;
        }
        slug.push(normalized);
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches(['-', '.', '_']).to_string();
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn run_git_checked(workspace: &Path, args: &[String], action: &str) -> Result<String, ToolError> {
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = Git::output(&arg_refs, workspace).map_err(|err| {
        ToolError::execution_failed(format!("Failed to {action}: could not run git: {err}"))
    })?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("git exited with status {}", output.status)
    };
    Err(ToolError::execution_failed(format!(
        "Failed to {action}: {detail}"
    )))
}

/// 将用户提供的 role/agent_role 值解析为规范角色字符串。
///
/// 这必须接受 [`SubAgentType::from_str`] 接受的完整集合，
/// 加上仅角色的别名（`worker`、`default`、`awaiter`）。
/// 在 #2649 之前，它只覆盖了一个子集，因此 `role: "reviewer"`（被 `from_str` 接受）
/// 在此处被第二次验证传递拒绝，并带有误导性的四个值提示。
fn normalize_role_alias(input: &str) -> Option<&'static str> {
    match input.to_ascii_lowercase().as_str() {
        "default" => Some("default"),
        "worker" | "general" | "general-purpose" | "general_purpose" => Some("worker"),
        "explorer" | "explore" | "exploration" => Some("explorer"),
        "awaiter" | "plan" | "planner" | "planning" => Some("awaiter"),
        "reviewer" | "review" | "code-review" | "code_review" => Some("reviewer"),
        "implementer" | "implement" | "implementation" | "builder" => Some("implementer"),
        "verifier" | "verify" | "verification" | "validator" | "tester" => Some("verifier"),
        "custom" => Some("custom"),
        _ => None,
    }
}

fn build_assignment_prompt(
    prompt: &str,
    assignment: &SubAgentAssignment,
    agent_type: &SubAgentType,
) -> String {
    let role = assignment.role.as_deref().unwrap_or("default");
    format!(
        "Assignment metadata:\n- objective: {}\n- role: {}\n- resolved_type: {}\n\nTask:\n{}",
        assignment.objective,
        role,
        agent_type.as_str(),
        prompt
    )
}

fn worker_status_from_subagent_status(status: &SubAgentStatus) -> AgentWorkerStatus {
    match status {
        SubAgentStatus::Running => AgentWorkerStatus::Running,
        SubAgentStatus::Completed => AgentWorkerStatus::Completed,
        SubAgentStatus::Failed(_) => AgentWorkerStatus::Failed,
        SubAgentStatus::Cancelled => AgentWorkerStatus::Cancelled,
        SubAgentStatus::BudgetExhausted => AgentWorkerStatus::Failed,
        SubAgentStatus::Interrupted(_) => AgentWorkerStatus::Interrupted,
    }
}

pub fn agent_worker_status_name(status: AgentWorkerStatus) -> &'static str {
    match status {
        AgentWorkerStatus::Queued => "queued",
        AgentWorkerStatus::Starting => "starting",
        AgentWorkerStatus::Running => "running",
        AgentWorkerStatus::WaitingForUser => "waiting_for_user",
        AgentWorkerStatus::ModelWait => "model_wait",
        AgentWorkerStatus::RunningTool => "running_tool",
        AgentWorkerStatus::Completed => "completed",
        AgentWorkerStatus::Failed => "failed",
        AgentWorkerStatus::Cancelled => "cancelled",
        AgentWorkerStatus::Interrupted => "interrupted",
    }
}

fn worker_status_from_subagent_result(result: &SubAgentResult) -> AgentWorkerStatus {
    if subagent_checkpoint_is_continuable(result) {
        AgentWorkerStatus::WaitingForUser
    } else {
        worker_status_from_subagent_status(&result.status)
    }
}

fn worker_progress_event_parts(message: &str) -> (AgentWorkerStatus, Option<u32>, Option<String>) {
    let step = parse_progress_step(message);
    let lower = message.to_ascii_lowercase();
    let status = if lower.contains("queued") {
        AgentWorkerStatus::Queued
    } else if lower.contains("waiting for user") || lower.contains("waiting for follow-up") {
        AgentWorkerStatus::WaitingForUser
    } else if lower.contains("requesting model response")
        || lower.contains(SUBAGENT_MODEL_WAIT_REASON)
    {
        AgentWorkerStatus::ModelWait
    } else if lower.contains("running tool") || lower.contains("executing") {
        AgentWorkerStatus::RunningTool
    } else if lower.contains("cancelled") {
        AgentWorkerStatus::Cancelled
    } else if lower.contains("interrupted") || lower.contains("timed out") {
        AgentWorkerStatus::Interrupted
    } else if lower.contains("complete") {
        AgentWorkerStatus::Completed
    } else if lower.contains("started") {
        AgentWorkerStatus::Starting
    } else {
        AgentWorkerStatus::Running
    };
    (status, step, parse_progress_tool_name(message))
}

fn parse_progress_step(message: &str) -> Option<u32> {
    let rest = message.strip_prefix("step ")?;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    (!digits.is_empty())
        .then(|| digits.parse::<u32>().ok())
        .flatten()
}

fn parse_progress_tool_name(message: &str) -> Option<String> {
    let marker = "tool '";
    let start = message.find(marker)? + marker.len();
    let rest = &message[start..];
    let end = rest.find('\'')?;
    let tool = rest[..end].trim();
    (!tool.is_empty()).then(|| tool.to_string())
}

fn subagent_progress_tool_display_name(name: &str) -> &str {
    match name {
        "exec_shell"
        | "exec_shell_wait"
        | "exec_shell_interact"
        | "exec_wait"
        | "exec_interact"
        | "task_shell_start"
        | "task_shell_wait" => "Bash",
        _ => name,
    }
}

fn emit_agent_progress(
    event_tx: Option<&mpsc::Sender<Event>>,
    agent_id: &str,
    status: String,
    parent_run_id: Option<String>,
    spawn_depth: u32,
) {
    if let Some(event_tx) = event_tx {
        if event_tx.max_capacity() > MIN_EVENT_CHANNEL_HEADROOM_FOR_ROUTINE_PROGRESS
            && event_tx.capacity() <= MIN_EVENT_CHANNEL_HEADROOM_FOR_ROUTINE_PROGRESS
            && routine_agent_progress_can_preserve_event_headroom(&status)
        {
            return;
        }
        let _ = event_tx.try_send(Event::AgentProgress {
            id: agent_id.to_string(),
            status,
            parent_run_id,
            spawn_depth,
        });
    }
}

fn routine_agent_progress_can_preserve_event_headroom(status: &str) -> bool {
    matches!(
        worker_progress_event_parts(status).0,
        AgentWorkerStatus::Running | AgentWorkerStatus::ModelWait | AgentWorkerStatus::RunningTool
    )
}

// === 工具注册表辅助 ===

/// 每个子代理的工具注册表。
///
/// 两种模式：
/// - **完整继承**（`allowed_tools = None`）：子级看到与父级 Agent 模式相同的工具表面，
///   但移除了遗留的子代理生命周期工具。仅当配置的深度预算允许另一个子级时，
///   单一的 `agent` 启动器才保持可见。需要批准的工具仅在父运行时自动批准时，
///   或对于显式可写角色（`implementer`、`custom`）当工具的批准要求为 `Suggest` 时才可调用。
/// - **显式窄化**（`allowed_tools = Some(list)`）：遗留/自定义路径。
///   注册表仍然构建完整的表面，但只有列出的工具名称对模型可见且可调用。
///
/// 纯每角色姿态检查（#3217），独立于任何运行时：一个角色是否可以调用给定批准级别的工具。
///
/// - 读取（`Auto`）工具始终允许。
/// - 写入/编辑/补丁（`Suggest`）工具需要可写姿态，因此只读角色（`explore`/`review`/`plan`/`verifier`）被拒绝。
/// - Shell（`Required`）工具需要 `Full` shell 姿态，因此只有 `verifier`/`implementer`/`general` 可以使用 shell；
///   `explore`/`review`（只读 shell）和 `plan`（无 shell）被拒绝，
///   因为只读 shell 的执行层尚未接入。
///
/// `custom` 由其显式的 `allowed_tools` 列表控制，因此姿态检查在此处允许它
/// （允许列表是该角色的权威来源）。
fn role_posture_permits(agent_type: &SubAgentType, approval: ApprovalRequirement) -> bool {
    if matches!(agent_type, SubAgentType::Custom) {
        return true;
    }
    let profile = WorkerRuntimeProfile::for_role(agent_type.clone());
    match approval {
        ApprovalRequirement::Auto => true,
        ApprovalRequirement::Suggest => profile.permissions.write,
        ApprovalRequirement::Required => {
            matches!(profile.shell, crate::worker_profile::ShellPolicy::Full)
        }
    }
}

struct SubAgentToolRegistry {
    /// `None` → 完整继承（不应用允许列表过滤）。`Some(list)` →
    /// 只有列出的工具对模型可见且可调用。
    allowed_tools: Option<Vec<String>>,
    /// 从父运行时的 `worker_profile`（#4042）继承的工具拒绝列表。
    /// 拒绝始终优先于允许，即使工具同时在允许列表和此列表中。
    /// 通配符匹配镜像会话端的 `command_denies_tool`（精确 + `prefix*`，不区分大小写）。
    disallowed_tools: Vec<String>,
    auto_approve: bool,
    /// Workflow 生成的子级自动接受 Suggest 级别的文件编辑。
    accept_edits: bool,
    /// 此注册表所属的子代理的角色/类型。用于决定 `Suggest` 级别工具
    /// （写入/编辑/补丁）是否可以在父运行时未自动批准的情况下在子级内部运行（#1828, #1833）。
    agent_type: SubAgentType,
    /// 已为此子级派生的能力信封。这捕获了父级姿态交集，
    /// 因此 Plan 父级可以暴露委托，而不会意外授予子级写入或 shell 工具。
    runtime_profile: WorkerRuntimeProfile,
    can_spawn_child: bool,
    owner_agent_id: String,
    owner_agent_name: String,
    registry: ToolRegistry,
}

impl SubAgentToolRegistry {
    #[cfg(test)]
    fn new(
        runtime: SubAgentRuntime,
        agent_type: SubAgentType,
        explicit_allowed_tools: Option<Vec<String>>,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> Self {
        Self::new_with_owner(
            runtime,
            agent_type,
            "agent_unknown".to_string(),
            "sub-agent".to_string(),
            explicit_allowed_tools,
            todo_list,
            plan_state,
        )
    }

    fn new_with_owner(
        runtime: SubAgentRuntime,
        agent_type: SubAgentType,
        owner_agent_id: String,
        owner_agent_name: String,
        explicit_allowed_tools: Option<Vec<String>>,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> Self {
        // 构建完整的代理表面——与父级的 Agent 模式相同。
        // 子级继承 shell、file、patch、search、web、git、diagnostics、
        // review 和 RLM，加上每个子级的新待办/计划状态。
        // 仅当深度预算仍有剩余时，`agent` 才被保留。
        let can_spawn_child = !runtime.would_exceed_depth();
        let context = runtime.context.clone();
        let mut surface_options = runtime.agent_tool_surface_options.clone();
        surface_options.shell_policy = ShellPolicy::from_legacy_allow_shell(runtime.allow_shell);
        let mut registry = ToolRegistryBuilder::new().with_full_agent_surface_options(
            Some(runtime.client.clone()),
            runtime.model.clone(),
            runtime.manager.clone(),
            runtime.clone(),
            surface_options,
            todo_list,
            plan_state,
        );

        if let Some(pool) = runtime.mcp_pool.as_ref() {
            registry = registry.with_mcp_tools(std::sync::Arc::clone(pool));
        }

        let registry = registry.build(context);

        Self {
            allowed_tools: explicit_allowed_tools,
            disallowed_tools: runtime.worker_profile.denied_tools.clone(),
            auto_approve: runtime.context.auto_approve,
            accept_edits: runtime.accept_edits,
            agent_type,
            runtime_profile: runtime.worker_profile,
            can_spawn_child,
            owner_agent_id,
            owner_agent_name,
            registry,
        }
    }

    /// 此角色是否允许在父运行时未自动批准的情况下使用 `Suggest` 级别工具（write_file、
    /// edit_file、apply_patch, ...）。只读姿态（`explore`、`plan`、`review`、
    /// `verifier`）保持阻止，因此它们不能在非自动批准的父级委托有限调查时静默修改工作区。
    /// `Required` 级别工具（shell 等）无论角色如何仍然需要父级自动批准（#1828, #1833）。
    fn role_can_delegate_writes(agent_type: &SubAgentType) -> bool {
        matches!(agent_type, SubAgentType::Implementer | SubAgentType::Custom)
    }

    /// 角色姿态是否允许给定的已注册工具，独立于父级自动批准。委托给纯 `role_posture_permits`。
    /// 未注册的名称通过（允许列表/可用性检查分别处理它们）。
    fn posture_permits_tool(&self, name: &str) -> bool {
        // 委托（`agent`）由深度预算和允许列表（`can_spawn_child` / `is_tool_allowed`）控制，
        // 而非写入/shell 姿态——只读角色可能仍然扇出子级工作。
        if name == "agent" {
            return true;
        }
        match self.registry.get(name) {
            Some(spec) => match spec.approval_requirement() {
                ApprovalRequirement::Auto => true,
                ApprovalRequirement::Suggest => {
                    self.runtime_profile.permissions.write
                        && role_posture_permits(&self.agent_type, ApprovalRequirement::Suggest)
                }
                ApprovalRequirement::Required => {
                    matches!(self.runtime_profile.shell, ShellPolicy::Full)
                        && role_posture_permits(&self.agent_type, ApprovalRequirement::Required)
                }
            },
            None => true,
        }
    }

    /// 检查工具名称是否被 `disallowed_tools` 列表拒绝，
    /// 使用与会话端 `command_denies_tool` 相同的匹配逻辑：
    /// 精确匹配 + `prefix*` 通配符，不区分大小写（#4042, #3027）。
    fn is_tool_denied(&self, name: &str) -> bool {
        if self.disallowed_tools.is_empty() {
            return false;
        }
        let tool_name = name.to_ascii_lowercase();
        self.disallowed_tools.iter().any(|rule| {
            let rule = rule.to_ascii_lowercase();
            if let Some(prefix) = rule.strip_suffix('*') {
                tool_name.starts_with(prefix)
            } else {
                tool_name == rule
            }
        })
    }

    /// 给定工具名称在此子级的过滤下是否被允许。
    /// `None` 过滤 = 所有内容被允许。
    fn is_tool_allowed(&self, name: &str) -> bool {
        if name == "agent" && !self.can_spawn_child {
            return false;
        }
        // 拒绝始终优先于允许——先检查拒绝列表，以便同时在允许列表和拒绝列表中的工具仍然被阻止（#4042）。
        if self.is_tool_denied(name) {
            return false;
        }
        match &self.allowed_tools {
            None => true,
            Some(list) => list.iter().any(|t| t == name),
        }
    }

    fn tools_for_model(&self, agent_type: &SubAgentType) -> Vec<Tool> {
        let _ = agent_type;
        let api_tools = self.registry.to_api_tools();
        let filtered = match &self.allowed_tools {
            None => api_tools,
            Some(list) => api_tools
                .into_iter()
                .filter(|tool| list.contains(&tool.name))
                .collect::<Vec<_>>(),
        };
        filtered
            .into_iter()
            .filter(|tool| tool.name != "agent" || self.can_spawn_child)
            // #4042: 隐藏显式禁止的工具，以便模型在函数调用 schema 中永远看不到它们
            // （与 `is_tool_allowed` / `execute` 守卫构成纵深防御）。
            .filter(|tool| !self.is_tool_denied(&tool.name))
            // #3217: 隐藏角色姿态禁止的工具，以便模型甚至从未看到写入/编辑/补丁
            // （只读角色）或 shell（无 shell 角色）。与下面的 `execute` 守卫构成纵深防御。
            .filter(|tool| self.posture_permits_tool(&tool.name))
            .collect()
    }

    fn unavailable_allowed_tools(&self) -> Vec<String> {
        match &self.allowed_tools {
            None => Vec::new(),
            Some(list) => list
                .iter()
                .filter(|name| !self.registry.contains(name))
                .cloned()
                .collect(),
        }
    }

    async fn execute(&self, _agent_id: &str, name: &str, input: Value) -> Result<String> {
        if !self.is_tool_allowed(name) {
            return Err(anyhow!("Tool {name} not allowed for this sub-agent"));
        }
        // #3217: 权威的每角色姿态——只读角色不能修改，非 `Full` shell 角色不能运行 shell，
        // 无论父级会话是否自动批准。这堵住了自动批准绕过漏洞，
        // 即只读子级可以静默写入或使用 shell 的问题。
        if !self.posture_permits_tool(name) {
            return Err(anyhow!(
                "Tool {name} is not permitted for the read-only `{role}` sub-agent role. Use an `implementer` or `general` role (or a `custom` role with an explicit allowed_tools list) to mutate the workspace or run shell commands.",
                role = self.agent_type.as_str()
            ));
        }
        if !self.auto_approve {
            let Some(spec) = self.registry.get(name) else {
                return Err(anyhow!("Tool {name} is not registered"));
            };
            match spec.approval_requirement() {
                ApprovalRequirement::Auto => {}
                ApprovalRequirement::Suggest => {
                // 写入/编辑/补丁工具落在此处。显式可写角色（`implementer`、`custom`）
                // 可以在没有父级自动批准的情况下运行它们（#1828, #1833）。
                // Workflow 生成的子级也接受任何可写姿态（包括 general）的 Suggest 编辑。
                // 只读角色仍然被拒绝。
                    let may_write = self.runtime_profile.permissions.write
                        && (self.accept_edits || Self::role_can_delegate_writes(&self.agent_type));
                    if !may_write {
                        return Err(anyhow!(
                            "Tool {name} requires approval and is not delegated to {role} sub-agents; rerun the parent with auto approval or pick a write-capable role",
                            role = self.agent_type.as_str()
                        ));
                    }
                }
                ApprovalRequirement::Required => {
                    return Err(anyhow!(
                        "Tool {name} requires approval and cannot run inside this sub-agent unless the parent session is auto-approved"
                    ));
                }
            }
        }
        reject_subagent_terminal_takeover(name, &input)?;
        let context = self
            .registry
            .context()
            .clone()
            .with_owner_agent(self.owner_agent_id.clone(), self.owner_agent_name.clone());
        self.registry
            .execute_full_with_context(name, input, Some(&context))
            .await
            .map(|result| result.content)
            .map_err(|e| anyhow!(e))
    }
}

fn reject_subagent_terminal_takeover(name: &str, input: &Value) -> Result<()> {
    let wants_interactive_shell = name == "exec_shell"
        && input
            .get("interactive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if wants_interactive_shell {
        return Err(anyhow!(
            "Sub-agents run in the background and cannot use exec_shell with interactive=true \
             because that would take over the parent TUI terminal. Use non-interactive \
             exec_shell, background=true, tty=true, or task_shell_start instead."
        ));
    }
    Ok(())
}

/// 解析子级的有效允许工具列表。
///
/// **v0.6.6 默认值：完整继承。** 返回 `Ok(None)` 意味着子级看到与父级 Agent 模式
/// 相同的工具表面——每个工具族包括 `with_subagent_tools` 以便它可以递归。
/// 窄化路径（`Ok(Some(list))`）仅由以下使用：
/// - `Custom` 代理类型（需要显式列表）。
/// - 传递 `explicit_tools` 的调用方（高级/遗留使用）。
///
/// `allow_shell = false` 不再窄化工具列表——子级的注册表只是不注册 shell 工具，
/// 这具有相同效果，且无需用拒绝列表掩盖父级的选择。
fn build_allowed_tools(
    agent_type: &SubAgentType,
    explicit_tools: Option<Vec<String>>,
    _allow_shell: bool,
) -> Result<Option<Vec<String>>> {
    if let Some(tools) = explicit_tools {
        let mut deduped = Vec::new();
        for tool in tools {
            let name = tool.trim();
            if !name.is_empty() && !deduped.iter().any(|existing: &String| existing == name) {
                deduped.push(name.to_string());
            }
        }
        if matches!(agent_type, SubAgentType::Custom) && deduped.is_empty() {
            return Err(anyhow!(
                "Custom sub-agent requires a non-empty allowed_tools list"
            ));
        }
        return Ok(Some(deduped));
    }

    if matches!(agent_type, SubAgentType::Custom) {
        return Err(anyhow!(
            "Custom sub-agent requires a non-empty allowed_tools list"
        ));
    }

    // 默认值：从父级完整继承注册表。子级看到父级拥有的每个工具，包括子代理管理工具族。
    // 注册表执行守卫仍然阻止需要批准的工具，除非父运行时被自动批准。
    Ok(None)
}

/// 呈现子代理模型失败及其完整错误链。anyhow 错误上的 `to_string()` 仅打印最外层上下文
/// （对于 Codex 子级，那是裸的"Responses API request failed"），丢弃了 HTTP 状态、
/// 清理后的正文片段和源 `LlmError` 携带的错误类——正是 #3884 中报告的错误掩盖。
/// 替代格式遍历链，downcast 前缀了一个稳定的类标签，以便失败记录一眼区分
/// auth/rate-limit/invalid-request/model/server/network 失败。
fn subagent_failure_message(err: &anyhow::Error) -> String {
    let class = match err.downcast_ref::<LlmError>() {
        Some(LlmError::RateLimited { .. }) => Some("rate_limited"),
        Some(LlmError::ServerError { .. }) => Some("server"),
        Some(LlmError::NetworkError(_)) | Some(LlmError::Timeout(_)) => Some("network"),
        Some(LlmError::AuthenticationError(_)) | Some(LlmError::AuthorizationError(_)) => {
            Some("auth")
        }
        Some(LlmError::InvalidRequest { .. }) => Some("invalid_request"),
        Some(LlmError::ModelError(_)) => Some("model"),
        Some(LlmError::ContentPolicyError(_)) => Some("content_policy"),
        Some(LlmError::ContextLengthError(_)) => Some("context_length"),
        Some(LlmError::ParseError(_)) | Some(LlmError::Other(_)) | None => None,
    };
    match class {
        Some(class) => format!("[{class}] {err:#}"),
        None => format!("{err:#}"),
    }
}

/// 子级模型选择方式的人类可读标签，以便启动失败可以指出产生失败模型的路由——
/// 从父级继承、更快的同族兄弟模型或显式 id（#4049）。
fn route_source_label(route: &ModelRoute) -> String {
    match route {
        ModelRoute::Inherit => "inherited from the parent/session model".to_string(),
        ModelRoute::Faster => "faster same-family sibling of the parent model".to_string(),
        ModelRoute::Auto => "auto (legacy route, treated as a faster sibling)".to_string(),
        ModelRoute::Fixed(id) => format!("explicit model id `{id}`"),
    }
}

/// 当子代理因其模型在当前访问配置文件下不可用而失败时，
/// 裸的提供商 403/404（分类为 `Authorization` 或 `State`）是无法操作的。
/// 注释它，以便父级知道哪个提供商和路由产生了失败的模型以及如何恢复（#2653, #4049），
/// 而无需重新分类底层错误。与模型可用性无关的错误保持不变地通过。
fn annotate_child_model_error(
    err: &str,
    model: &str,
    provider: crate::config::ApiProvider,
    route: &ModelRoute,
) -> String {
    let hint = || {
        format!(
            "{err}\n(provider `{}` · requested model `{model}` · route: {} — \
             the model may be unavailable under the current access profile; remove the explicit \
             child model override or adjust child-agent model config before retrying)",
            provider_name_for_error(provider),
            route_source_label(route),
        )
    };
    match crate::error_taxonomy::classify_error_message(err) {
        crate::error_taxonomy::ErrorCategory::Authorization
        | crate::error_taxonomy::ErrorCategory::State => hint(),
        _ => {
            // #3020 (#2653): 诸如"Model Not Exist"或"does not exist or you do not have access"
            // 等提供商拒绝通常被分类为 `Internal` 而非 `Authorization`/`State`。
            // 在原始错误文本中捕获这些模式并无论如何进行注释。
            let lower = err.to_ascii_lowercase();
            if lower.contains("model not exist")
                || lower.contains("model_not_found")
                || lower.contains("does not exist")
                || lower.contains("no such model")
                || lower.contains("invalid model")
            {
                hint()
            } else {
                err.to_string()
            }
        }
    }
}

/// 字符预算，超过此预算的子代理摘要被视为大型转储并进行头部+尾部截断。
/// 镜像 `crates/tui/src/client/chat.rs:702` 中的 `TOOL_RESULT_SENT_CHAR_BUDGET`，
/// 以便子代理摘要使用与常规工具输出相同的阈值。
/// 本地复制以避免子代理模块与线路压缩内部实现耦合。
const SUBAGENT_SUMMARY_CHAR_BUDGET: usize = 12_000;
/// 截断时的头部/尾部切片大小；镜像线路常量
///（`TOOL_RESULT_HEAD_CHARS`/`TOOL_RESULT_TAIL_CHARS`, chat.rs:703-704）。
const SUBAGENT_SUMMARY_HEAD_CHARS: usize = 4_000;
const SUBAGENT_SUMMARY_TAIL_CHARS: usize = 4_000;

/// 一行来源后缀，强调子代理摘要是自我报告（issue #2652）。
/// 仅在摘要未被长度截断时追加，因此每个摘要恰好携带一个边界标记。
const SUBAGENT_SELF_REPORT_NOTE: &str = "\n[Sub-agent self-report — re-verify material claims (read changed files, \
run the relevant tests) before relying on it.]";

/// 用来源/裁剪标记标记子代理摘要（issue #2652）。
///
/// 返回 `(stamped_summary, truncated)`：
/// - 当原始摘要在预算内时，追加柔和的自我报告说明并报告 `truncated: false`。
/// - 当它超出预算时，保留头部+尾部切片并用现有的 `[Output truncated ...]`
///   词汇（从工具输出截断复用）标记，调整为诚实地说明被省略的中间部分不在溢出存储中——
///   子代理摘要没有 `retrieve_tool_result` 句柄。报告 `truncated: true`。
///
/// 因此每个摘要恰好获得一个边界标记，永远不会有两个。
fn stamp_subagent_summary(raw: &str) -> (String, bool) {
    let total = raw.chars().count();
    if total <= SUBAGENT_SUMMARY_CHAR_BUDGET {
        return (format!("{raw}{SUBAGENT_SELF_REPORT_NOTE}"), false);
    }
    let chars: Vec<char> = raw.chars().collect();
    let head: String = chars.iter().take(SUBAGENT_SUMMARY_HEAD_CHARS).collect();
    let tail: String = chars
        .iter()
        .skip(total.saturating_sub(SUBAGENT_SUMMARY_TAIL_CHARS))
        .collect();
    let omitted = total
        .saturating_sub(SUBAGENT_SUMMARY_HEAD_CHARS)
        .saturating_sub(SUBAGENT_SUMMARY_TAIL_CHARS);
    let stamped = format!(
        "{head}\n\n[Sub-agent summary truncated: {SUBAGENT_SUMMARY_HEAD_CHARS} + {SUBAGENT_SUMMARY_TAIL_CHARS} of {total} \
chars shown. This is the child's self-report; the elided middle ({omitted} chars) is not in \
the spillover store and cannot be retrieved via retrieve_tool_result. Re-open the child or \
read changed files directly to verify material claims.]\n\n{tail}",
    );
    (stamped, true)
}

fn summarize_subagent_result(result: &SubAgentResult) -> String {
    if let Some(needs_input) = result.needs_input.as_ref() {
        return format!("Needs input: {}", needs_input.question);
    }
    match (&result.status, result.result.as_ref()) {
        (SubAgentStatus::Completed, Some(text)) => text.clone(),
        (SubAgentStatus::Completed, None) => "Completed (no final summary returned)".to_string(),
        (SubAgentStatus::Interrupted(error), _) => format!("Interrupted: {error}"),
        (SubAgentStatus::Cancelled, _) => "Cancelled".to_string(),
        (SubAgentStatus::BudgetExhausted, Some(text)) => format!(
            "Child token budget exhausted before finishing; partial output preserved below.\n{text}"
        ),
        (SubAgentStatus::BudgetExhausted, None) => {
            "Child token budget exhausted before returning a final summary; retry with a smaller scoped task or split the work.".to_string()
        }
        (SubAgentStatus::Failed(error), _) => format!("Failed: {error}"),
        (SubAgentStatus::Running, _) => "Running".to_string(),
    }
}

fn subagent_status_name(status: &SubAgentStatus) -> &'static str {
    match status {
        SubAgentStatus::Running => "running",
        SubAgentStatus::Completed => "completed",
        SubAgentStatus::Interrupted(_) => "interrupted",
        SubAgentStatus::Failed(_) => "failed",
        SubAgentStatus::Cancelled => "cancelled",
        SubAgentStatus::BudgetExhausted => "budget_exhausted",
    }
}

const SUBAGENT_OUTPUT_FORMAT: &str = include_str!("../../prompts/subagent_output_format.md");

const GENERAL_AGENT_INTRO: &str = concat!(
    "You are a trusted general-purpose sub-agent. Your job is to complete the one task you were given, end-to-end, and report back concisely.\n",
    "Stay inside the assigned scope; put adjacent work under RISKS/BLOCKERS.\n",
    "For genuinely multi-step work, track progress with `work_update` (and `update_plan` for Strategy metadata); skip it for short, focused tasks.\n",
    "**Stop quickly on failure**: if the same tool call fails 2 times in a row, stop retrying and return what you have so far with a one-line note explaining what's missing. Do not loop on impossible queries (e.g. external API unreachable, rate-limited, or returning empty).\n",
    "For implementer or repair-style work, keep going within the assigned scope; checkpoint before broadening the task or after repeated failures instead of forcing a tiny tool-call cap.\n\n"
);

const EXPLORE_AGENT_INTRO: &str = concat!(
    "You are a trusted exploration sub-agent (role: `explore`). Your job is to map the relevant code quickly and stay strictly read-only.\n",
    "Default to `EFFORT: quick`: aim for about 3-5 tool calls unless the brief explicitly asks for more.\n",
    "Orient first: confirm the workspace/project root, read relevant AGENTS.md/README guidance when the tree is unfamiliar, then search only the likely scope.\n",
    "Use list_dir/file_search, grep_files, and read_file; use RLM only for long inputs or many semantic slices, not basic path discovery.\n",
    "Honor QUESTION, SCOPE, ALREADY_KNOWN, and STOP_CONDITION. Do not repeat ALREADY_KNOWN work unless evidence contradicts it; do not broaden once QUESTION is answered.\n",
    "DeepSeek V4 can hold broad evidence, but your value is compressed reconnaissance: cite `path:line-range` for each finding and stop once evidence is sufficient. Return partial findings if the next step would be speculative or duplicative.\n",
    "CHANGES will almost always be \"None.\" for an explorer.\n\n"
);

const PLAN_AGENT_INTRO: &str = concat!(
    "You are a trusted planning sub-agent (role: `plan`). Your job is to produce a grounded, prioritized plan, not patches.\n",
    "Read enough code to avoid guessing; each step names its artifact and verification.\n",
    "Use work_update for concrete To-do progress and update_plan only for Strategy metadata/context/route; explain key trade-offs.\n",
    "CHANGES should list plan artifacts only, not future speculative edits.\n\n"
);

const REVIEW_AGENT_INTRO: &str = concat!(
    "You are an adversarial code review sub-agent (role: `review`). Assume the change is broken until the evidence proves otherwise: actively try to refute the claims made about it, and stay strictly read-only.\n",
    "Read the diff/files, grep sibling patterns/tests, hunt regressions, missing tests, unhandled edge cases, and quiet behavior changes, then order EVIDENCE by severity.\n",
    "Use BLOCKER/MAJOR/MINOR/NIT and include path:line-range plus suggested fix.\n",
    "You may use more tool calls than quick exploration, but stop after decisive evidence instead of widening the review forever.\n",
    "If nothing survives your attack, say plainly in SUMMARY that no MAJOR+ issues exist — a clean verdict earned adversarially is a real result, not a failure.\n",
    "CHANGES will almost always be \"None.\" for a reviewer.\n\n"
);

const CUSTOM_AGENT_INTRO: &str = concat!(
    "You are a trusted custom sub-agent (role: `custom`) with a narrowed tool registry. Your job is to stay tightly scoped to the assigned objective.\n",
    "Use only tools available at runtime; put missing capabilities under BLOCKERS and stop.\n\n"
);

const IMPLEMENTER_AGENT_INTRO: &str = concat!(
    "You are a trusted implementation sub-agent (role: `implementer`). Your job is to land the assigned change with minimal surrounding edits.\n",
    "Read target files before editing; prefer edit_file for narrow changes and apply_patch for hunks.\n",
    "Run relevant verification after edit batches; write needed tests with the implementation.\n",
    "You are not limited to an explorer-style 3-5 tool-call cap. Checkpoint before expanding scope or after repeated failures, then continue only inside the assigned brief.\n",
    "CHANGES is load-bearing: list every modified file with a one-line why.\n\n"
);

const VERIFIER_AGENT_INTRO: &str = concat!(
    "You are a trusted verification sub-agent (role: `verifier`). Your job is to run the requested gates and report results, and stay read-only.\n",
    "Report PASS/FAIL/FLAKY at the top of SUMMARY with exact command evidence.\n",
    "Capture failing assertion and file:line; put obvious fixes under RISKS.\n",
    "You may use more tool calls than quick exploration, but stop after decisive pass/fail evidence.\n",
    "CHANGES will almost always be \"None.\" for a verifier.\n\n"
);

// === 测试 ===

#[cfg(test)]
mod tests;
