//! 由 UI 提交给核心引擎的操作
//!
//! 这些操作通过channel从 TUI 流向引擎，使得引擎处理请求时 UI 能保持响应。

use crate::compaction::CompactionConfig;   // 上下文压缩配置
use crate::config::ApiProvider;            // API 提供商（如 DeepSeek、OpenAI 等）
use crate::models::{Message, SystemPrompt};   // 对话消息, 系统提示词
use crate::tools::goal::GoalStatus;           // 目标状态（活跃/暂停/完成等）
use crate::tui::app::AppMode;              // 应用模式
use crate::tui::approval::ApprovalMode;    // 审批模式
use codewhale_protocol::runtime::DynamicToolSpec;   // 动态工具规格
use std::path::PathBuf;                    // 文件路径

/// 这个常量是用作用户直接在 UI 输入 ! ls 这种 shell 快捷命令时，生成的工具调用 ID 的前缀。
pub const USER_SHELL_TOOL_ID_PREFIX: &str = "user_shell_";

/// 引擎处理 GetSessionSnapshot 请求时，把当前会话的快照打包成这个结构体，通过 
/// oneshot channel 返回给调用者用于保存到磁盘。
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub messages: Vec<Message>,
    pub total_tokens: u64,
    pub model: String,
    pub model_provider: String,
    pub workspace: PathBuf,
    pub system_prompt: Option<SystemPrompt>,
    pub mode: String,
}

/// Provider request runtime state surfaced by `/provider`.
/// Returned by `Op::GetProviderRuntimeStatus` via a oneshot channel.
/// 这个结构体是 /provider 命令的底层返回数据，用于展示提供商的运行时状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRuntimeStatus {
    /// 哪个 API 提供商
    pub provider: ApiProvider,
    /// 并发请求上限（Option 表示可能没限制）
    pub request_concurrency_limit: Option<usize>,
    /// 当前正在进行的请求数
    pub active_provider_requests: usize,
}

/// 以用户角色轮次引入的文本来源。
///
/// 聊天提供方为了兼容性，会通过 `role = "user"` 强制传递许多运行时/控制平面信号，
/// 因此仅凭角色本身并不能作为权威依据。
#[allow(dead_code)] // 某些来源保留给首个门控之后的接入站点使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserInputProvenance {
    /// 真人用户（通过 TUI/CLI 输入） 
    ExternalUser,
    /// 运行时本身（比如定时任务、钩子触发）
    Runtime,
    /// 子代理交还结果时（来自子工作器或子智能体交接的补全/事件文本）。
    SubAgentHandoff,
    /// 从已保存/导入的对话记录中恢复的文本。
    ImportedTranscript,
    /// 从记忆或其他持久化来源中调取的文本。
    MemoryRecall,
    /// 由助手撰写的、形式类似用户回复的文本。(模型自己生成、伪装成用户的消息)
    AssistantGenerated,
}

impl UserInputProvenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExternalUser => "external_user",
            Self::Runtime => "runtime",
            Self::SubAgentHandoff => "subagent_handoff",
            Self::ImportedTranscript => "imported_transcript",
            Self::MemoryRecall => "memory_recall",
            Self::AssistantGenerated => "assistant_generated",
        }
    }

    /// 只有真人输入才有权授权工作，其他来源（子代理交还、记忆调取等）无权。
    pub fn can_authorize_work(self) -> bool {
        matches!(self, Self::ExternalUser)
    }
}

/// Op枚举的每一个变体都是一个提交给引擎的"操作指令"。Rust枚举的变体可以像结构体一样携带命名字段。
#[derive(Debug, Clone)]
pub enum Op {
    /// Send a message to the AI
    SendMessage {
        content: String,  // 用户输入的文本
        mode: AppMode,    // 当前模式（Agent / Plan / YOLO）
        /// Provider route to use for this turn. `None` keeps the session
        /// provider; auto model routing sets this when the inventory selects a
        /// different authenticated provider.
        /// API 提供商路由，None 表示沿用会话默认
        provider: Option<ApiProvider>,
        model: String,  // 模型名称
        goal_objective: Option<String>,  // 目标描述（/goal 设置的目标）
        goal_token_budget: Option<u32>,  // 目标 token 预算
        goal_status: GoalStatus,         // 目标当前状态
        /// 推理深度: `"off" | "low" | "medium" | "high" | "max"`.
        /// `None` lets the provider apply its default.
        reasoning_effort: Option<String>,
        /// True when the user selected auto thinking, even though the UI sends
        /// a concrete per-turn value to the model API.
        /// 用户是否选了"自动推理"
        reasoning_effort_auto: bool,
        /// True when the user selected auto model routing.
        /// 用户是否选了"自动选择模型"
        auto_model: bool,
        allow_shell: bool,    // 是否允许执行 shell 命令
        trust_mode: bool,     // 是否信任模式
        auto_approve: bool,   // 是否自动批准工具调用
        approval_mode: ApprovalMode,   // 审批模式
        translation_enabled: bool,     // 是否开启翻译
        show_thinking: bool,  // 是否显示模型思考过程
        /// Tool restriction from custom slash command frontmatter.
        /// `None` means the current turn may use the normal tool set.
        /// 本次允许的工具列表（None=全部允许）
        allowed_tools: Option<Vec<String>>,
        /// Runtime-supplied tools available only for this turn.
        /// 本次临时可用的动态工具
        dynamic_tools: Vec<DynamicToolSpec>,
        /// Hook executor for control-plane hooks.
        /// `ToolCallBefore` hooks may deny a tool call with exit code 2.
        /// 钩子执行器
        hook_executor: Option<std::sync::Arc<crate::hooks::HookExecutor>>,
        verbosity: Option<String>,    // 详细程度
        /// Structural input origin. This gates whether the turn may inherit
        /// YOLO/auto-approval authority; user-shaped text is not enough.
        /// 输入来源——这是安全门控：只有 ExternalUser 才能继承 YOLO/自动批准权限
        provenance: UserInputProvenance,
    },

    /// Execute a user-submitted composer shell command (`! <command>`) without
    /// sending a model turn. This still routes through `exec_shell`, approval,
    /// sandbox, and command-safety handling.
    /// 这个操作是用户输入!ls 这种快捷shell命令时发送的。注意它不发送模型请求
    /// （不需要 AI 回复），只是执行一条 shell 命令，但仍然经过安全审批流程。
    RunShellCommand {
        command: String,
        mode: AppMode,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: ApprovalMode,
    },

    /// Set the runtime goal status without dispatching a model turn. Used by
    /// `/goal pause`, `/goal resume`, `/goal clear`, etc. so the engine's
    /// `SharedGoalState` learns the new status immediately and a queued
    /// continuation doesn't overwrite it back to Active.
    /// 对应 /goal pause、/goal resume、/goal clear 等命令。不触发模型——只更新
    /// 引擎内部的 SharedGoalState。
    SetGoalStatus {
        status: GoalStatus,
        /// When `true`, clear the objective entirely (`/goal clear`).
        clear: bool,
    },

    /// Cancel the current request
    #[allow(dead_code)]
    CancelRequest,

    /// Approve a tool call that requires permission
    #[allow(dead_code)]
    ApproveToolCall { id: String },

    /// Deny a tool call that requires permission
    #[allow(dead_code)]
    DenyToolCall { id: String },

    /// Spawn a sub-agent
    #[allow(dead_code)]
    SpawnSubAgent { prompt: String },

    /// List current sub-agents and their status
    ListSubAgents,

    /// Cancel a running sub-agent by id or session name.
    CancelSubAgent { agent_id: String },

    /// Change the operating mode
    #[allow(dead_code)]
    ChangeMode {
        mode: AppMode,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: ApprovalMode,
    },

    /// Update the model being used and refresh stable prompt context.
    #[allow(dead_code)]
    SetModel {
        model: String,
        mode: AppMode,
        route_limits: Option<codewhale_config::route::RouteLimits>,
    },

    /// Update auto-compaction settings
    /// 设置自动压缩
    SetCompaction { config: CompactionConfig },

    /// Update the SSE idle timeout used for subsequent streamed turns.
    /// SSE 超时
    SetStreamChunkTimeout { timeout_secs: u64 },

    /// Update sub-agent runtime controls for subsequent turns.
    /// 子代理运行时限制
    SetSubagentRuntimeConfig {
        enabled: bool,
        max_subagents: usize,
        launch_concurrency: usize,
        max_spawn_depth: u32,
        api_timeout_secs: u64,
        heartbeat_timeout_secs: u64,
    },

    /// Sync engine session state (used for resume/load)
    /// 恢复/加载会话
    SyncSession {
        session_id: Option<String>,
        messages: Vec<Message>,
        system_prompt: Option<SystemPrompt>,
        system_prompt_override: bool,
        model: String,
        workspace: PathBuf,
        mode: AppMode,
    },

    /// Run context compaction immediately.
    /// 手动触发压缩
    CompactContext,

    /// Get a snapshot of the current session state (messages, tokens, etc.)
    /// for saving to disk. Returns the result via the oneshot sender so
    /// the caller doesn't have to compete with the SSE event stream.
    GetSessionSnapshot {
        tx: std::sync::Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<SessionSnapshot>>>>,
    },

    /// Get active provider request concurrency state for readiness surfaces.
    GetProviderRuntimeStatus {
        tx: std::sync::Arc<
            std::sync::Mutex<Option<tokio::sync::oneshot::Sender<ProviderRuntimeStatus>>>,
        >,
    },

    /// Run agent-driven context purging.
    /// 清除上下文
    PurgeContext,

    /// Edit the last user message: remove the last user+assistant exchange
    /// from the session, then re-send with the new content.
    /// 编辑上一条消息
    #[allow(dead_code)]
    EditLastTurn { new_message: String },

    /// Shutdown the engine.
    /// 关闭引擎
    Shutdown,
}
