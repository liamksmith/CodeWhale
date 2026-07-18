//! 这个模块定义了从核心引擎发送到 UI 的事件。
//!
//! 事件通过一个通道（channel）从引擎流向 TUI，实现非阻塞、实时更新。
//! enabling non-blocking, real-time updates.
//! 这个文件是 CodeWhale 的"神经系统"——定义了引擎和 UI之间所有可能的通信消息类型。整体设计模式是 Rust 中非常经典的：
//! 1. 定义一个大的 enum Event 作为"消息总线"
//! 2. 每个变体携带该场景需要的具体数据
//! 3. 引擎端发送事件（producer），UI 端匹配事件并响应（consumer）

use std::{path::PathBuf, sync::Arc};

use serde_json::Value;

use crate::error_taxonomy::ErrorEnvelope;
// - Message：一条聊天消息（对话记录的基本单元）
// - SystemPrompt：系统提示词
// - Tool：工具定义（名称、描述、参数 schema 等）
// - Usage：token 用量统计
use crate::models::{Message, SystemPrompt, Tool, Usage};
// - GoalSnapshot：目标的快照状态（引擎内部的目标管理系统）。
use crate::tools::goal::GoalSnapshot;
// ToolError / ToolResult：工具执行的错误类型和结果类型。注意 ToolResul
// 是工具成功时的输出，ToolError 是失败时的错误信息。
use crate::tools::spec::{ToolError, ToolResult};
// - SubAgentResult：子代理的执行结果。
use crate::tools::subagent::SubAgentResult;
// - UserInputRequest：向用户请求输入时的请求描述（比如"请确认要删除以下文件"）。
use crate::tools::user_input::UserInputRequest;

/// Final status for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcomeStatus {
    Completed,  // 回合正常完成
    Interrupted,  // 回合被中断（比如用户按了 Ctrl+C）
    Failed,     // 回合失败（比如 API 调用出错）
}

/// Events emitted by the engine to update the UI.
#[derive(Debug, Clone)]
pub enum Event {
    // === 流式事件（Streaming Events） ===
    /// 一个新的 LLM 消息块（message block）开始生成了。LLM的回复会分块流式返回。
    MessageStarted {
        #[allow(dead_code)]
        index: usize,  // 消息块的索引号（第几条消息）。
    },

    /// Incremental text content delta
    /// 消息的增量文本内容。LLM流式输出时，每收到一小段文本就发一个 MessageDelta 事件.
    MessageDelta {
        #[allow(dead_code)]
        index: usize,
        content: String,
    },

    /// 一个消息块生成完毕。
    MessageComplete {
        #[allow(dead_code)]
        index: usize,
    },

    // ThinkingStarted/ThinkingDelta/ThinkingComplete
    //  这三兄弟与上面完全对应，只不过处理的是思考过程（thinking / reasoning）
    // ，而非最终回复文本。DeepSeek 等推理模型会先产出一段"思考"（比如reasoning_content），再产出正式回复。
    // 这些事件让 UI能分开渲染思考区和回复区。
    /// Thinking block started
    ThinkingStarted {
        #[allow(dead_code)]
        index: usize,
    },

    /// Incremental thinking content delta
    ThinkingDelta {
        #[allow(dead_code)]
        index: usize,
        content: String,
    },

    /// Thinking block completed
    ThinkingComplete {
        #[allow(dead_code)]
        index: usize,
    },

    // === 工具事件（Tool Events） ===
    /// LLM 决定调用一个工具。
    ToolCallStarted {
        id: String,      // 工具调用的唯一标识（用来和 ToolCallComplete 配对）。
        name: String,    // 工具名称（比如 "read_file"、"exec_shell"）。
        input: Value,    // 工具的输入参数，用 serde_json::Value表示——因为不同工具的参数结构不同，用通用的 JSON 值来承载。
    },

    /// 工具执行完毕。
    ToolCallComplete {
        id: String,
        name: String,
        result: Result<ToolResult, ToolError>,
    },

    // === 回合生命周期（Turn Lifecycle） ===
    /// 新一轮对话开始。turn_id 是这一轮的标识符。 (user sent a message)
    TurnStarted { turn_id: String },

    /// 本轮完成。包含丰富的元数据： (no more tool calls)
    /// - usage: Usage：本轮的 token 用量（输入/输出/缓存命中等）。
    /// - status: TurnOutcomeStatus：完成/中断/失败。
    /// - error: Option<String>：如果有错误，这里是错误描述。
    /// - tool_catalog: Option<Vec<Tool>>：本轮请求时附带发送的工具列表。可能有工具列表，也可能没有（比如纯对话）。
    /// - base_url: Option<String>：本次请求的 API 基础 URL。
    TurnComplete {
        usage: Usage,
        status: TurnOutcomeStatus,
        error: Option<String>,
        /// Tool catalog sent with this turn's model request.
        tool_catalog: Option<Vec<Tool>>,
        /// API base URL used by this turn's client.
        base_url: Option<String>,
    },

    /// 引擎内部的目标状态发生变化。CodeWhale 有目标管理系统（通常是create_goal / update_goal 工具调用），
    /// 这个事件通知 UI 刷新目标面板。
    GoalUpdated { snapshot: GoalSnapshot },

    /// 上下文压缩开始。当对话历史太长时，CodeWhale会压缩历史记录以节省 token 和缓存空间。
    CompactionStarted {
        id: String,
        auto: bool,    // 是自动触发还是手动触发。
        message: String,
    },

    /// 压缩完成。messages_before/messages_after 表示压缩前后的消息数量。
    /// summary_prompt 保存了压缩摘要（以便引擎重启后恢复）
    CompactionCompleted {
        id: String,
        auto: bool,
        message: String,
        /// Number of messages before compaction.
        #[allow(dead_code)]
        messages_before: Option<usize>,
        /// Number of messages after compaction.
        #[allow(dead_code)]
        messages_after: Option<usize>,
        /// Rendered text of the accumulated compaction summary prompt, if any.
        /// Host layers (e.g. the /v1 runtime) persist this into the thread
        /// record so the summary survives engine reloads — without it the
        /// summary lives only in engine memory and is lost on LRU eviction
        /// or restart (SyncSession re-extracts it from the record prompt).
        summary_prompt: Option<String>,
    },

    /// Context purge started.
    /// Purge（清理）是一组类似的事件。"Purge"的意思比"Compaction"更彻底——直接删除部
    /// 分消息而非总结压缩。removed_count 是删除的消息数，replaced_count是被替换的操作数。
    PurgeStarted {
        /// Status message for display.
        message: String,
    },

    /// 上下文清理已完成。
    PurgeCompleted {
        /// 清理前的消息数量。
        messages_before: usize,
        /// 清理后的消息数量。
        messages_after: usize,
        /// 已移除的消息数量。
        removed_count: usize,
        /// 已应用的替换操作数量。
        replaced_count: usize,
        /// 用于显示的摘要消息。
        message: String,
    },

    /// Context purge failed.
    PurgeFailed { message: String },

    /// 压缩失败。
    CompactionFailed {
        id: String,
        auto: bool,
        message: String,
    },

    // === 子代理事件（Sub-Agent Events） ===
    /// 一个子代理被创建。
    AgentSpawned {
        id: String,
        prompt: String,
        parent_run_id: Option<String>,   // 父代理的 ID。顶层代理这个值为 None。
        spawn_depth: u32,  // 嵌套深度。深度为 0 表示顶层代理，1表示直接子代理，以此类推。u32 是 32 位无符号整数。
    },

    /// 子代理的状态更新（"运行中"、"正在调用工具"等）。
    AgentProgress {
        id: String,
        status: String,
        parent_run_id: Option<String>,
        spawn_depth: u32,
    },

    /// 子代理完成。
    AgentComplete { id: String, result: String },

    /// 子代理列表（用于 /agents 命令等查询场景）。
    AgentList { agents: Vec<SubAgentResult> },

    /// 结构化的子代理邮箱信封。
    /// Structured sub-agent mailbox envelope (issue #128). Carries the
    /// monotonic seq + the typed `MailboxMessage` so the UI can route each
    /// envelope to the correct in-transcript card.
    /// 类型化的邮箱消息（MailboxMessage），配合 issue #128 的设计。UI可以把这个事件路由到对应的对话卡片。
    SubAgentMailbox {
        seq: u64,   // 单调递增的序列号，保证消息顺序。
        message: crate::tools::subagent::MailboxMessage,
    },

    /// Live workflow UI event (#4122). Mirrors a typed `WorkflowUiEvent` JSON
    /// object so the TUI can advance the WorkflowPanel and the compact history
    /// card while a run is still in flight (not only on tool complete).
    /// 实时工作流 UI 事件（#4122）。镜像一个类型化的 `WorkflowUiEvent` JSON
    /// 对象，可以让 UI在运行过程中显示工作流面板的进度（而不仅仅是工具完成后的结果）。
    WorkflowUi {
        run_id: String,
        /// 扁平化的事件 JSON：`{"type":"task_started", "at_ms":…, …}`。
        /// 调用方在可用时将 `run_id` 注入到对象中。
        event: Value,
    },

    // === 系统事件（System Events） ===
    /// 发生了一个错误。envelope 包含分类后的错误信息，recoverable表示是否可恢复。
    Error {
        envelope: ErrorEnvelope,
        #[allow(dead_code)]
        recoverable: bool,
    },

    /// 通用状态消息，供 UI 在状态栏显示。
    Status { message: String },

    /// 暂停终端输入。Pause terminal input events (for interactive subprocesses).
    /// 暂停终端输入事件处理。当有交互式子进程（比如 vim 或 ssh）需要直接接管终端时，TUI 必须释放终端控制权。
    PauseEvents {
        /// Optional one-shot notification fired after the UI has actually
        /// released the terminal to the child process.
        /// 可选的一次性通知（类似"确认收到"信号）。tokio::sync::Notify 是 tokio
        /// 异步运行时提供的轻量通知原语——一个线程通知另一个线程"某事发生了"。用 Arc
        /// 包装是因为要跨线程共享。UI 在释放终端后通过这个 Notify通知引擎"可以开始了"。
        ack: Option<Arc<tokio::sync::Notify>>,
    },

    /// Resume terminal input events after subprocess completion
    /// 恢复终端输入
    /// 子进程结束，恢复终端输入事件。
    ResumeEvents,

    /// Request user approval for a tool call
    /// 工具审批请求。需要用户审批才能执行某个工具（写文件、执行 shell命令等）。
    ApprovalRequired {
        id: String,
        tool_name: String,
        description: String,
        /// Tool parameters for approval display. Carried on the event so the
        /// TUI does not need to reconstruct them from `pending_tool_uses`.
        /// 工具参数，供审批界面展示。
        input: Value,
        /// Exact-argument fingerprint, used to scope *denials* (#1617).
        /// approval_key / approval_grouping_key：用于判断是否可以复用之前的审批决定（"本次会话都允许"）。
        /// 这是两个不同粒度的指纹。approval_key精确匹配——用于拒绝（"拒绝所有相同的参数"）。
        /// approval_grouping_key 松散匹配——用于批准（"允许所有同类型操作"）。
        approval_key: String,
        /// Lossy / arity-aware fingerprint, used to scope *approvals* so an
        /// "approve for session" covers later flag variants (v0.8.37).
        approval_grouping_key: String,
        /// 模型在调用写入工具前对意图的解释 (#2381).
        /// 审批界面展示它可以帮用户理解"为什么要做这个修改"。
        intent_summary: Option<String>,
        /// When true, the UI must show the prompt instead of consuming
        /// session/auto approval shortcuts.
        /// 强制显示审批提示（忽略任何自动/会话级快捷批准）。
        approval_force_prompt: bool,
    },

    /// Request user input for a tool call
    /// 工具需要用户提供输入（比如模型说"这个文件名你想用什么？"）。
    UserInputRequired {
        id: String,
        request: UserInputRequest,
    },

    /// Authoritative API conversation state from the engine session.
    ///
    /// The UI receives granular display events, but those are not always a
    /// lossless representation of the API transcript. DeepSeek can emit
    /// reasoning directly followed by tool calls without a visible assistant
    /// text block, and that assistant message still has to be persisted for
    /// later `reasoning_content` replay.
    /// 会话状态更新：引擎会话的权威 API 对话状态。UI 收到流式事件（MessageDelta 等）
    /// 是增量更新，但这个事件提供了完整的 API级别视图——所有消息、系统提示词、模型名、工作区路径。
    /// 为什么需要它：DeepSeek 可以直接输出 reasoning 后跟 tool call 而不经过可见的
    /// assistant 文本块，UI 需要完整的消息列表来正确渲染。
    SessionUpdated {
        session_id: String,
        messages: Vec<Message>,
        system_prompt: Option<SystemPrompt>,
        model: String,
        workspace: PathBuf,
    },

    /// Request user decision after sandbox denial
    /// 沙箱提权：沙箱拒绝了某个操作，询问用户是否需要提权。比如模型尝试访问网络或写入受
    /// 保护路径。blocked_network 和 blocked_write告诉用户拒绝了什么类型的操作。
    #[allow(dead_code)]
    ElevationRequired {
        tool_id: String,
        tool_name: String,
        command: Option<String>,
        denial_reason: String,
        blocked_network: bool,
        blocked_write: bool,
    },

    /// 可观察的 LSP 自动修复循环更新 (#4107)。当模型写代码后
    /// LSP（语言服务器协议，提供语法检查/代码补全等）报告错误，引擎可能自动修复。
    /// 这个事件让 Turn Inspector面板能看到修复进度。只携带摘要信息（诊断数量、文件数、是否注入），
    /// 不暴露内部提示词。
    /// Carries only summary counts/state — never raw prompt internals.
    LspRepairUpdate {
        diagnostics_found: usize,
        files: usize,
        injected: bool,
    },

    // === 前缀缓存稳定性事件 ===
    /// 前缀（系统提示词 +工具定义）在两轮之间发生了变化，导致 DeepSeek 的 KV 前缀缓存失效。
    /// Carries diagnostics for the TUI to surface.
    PrefixCacheChange {
        /// 变更内容的人类可读描述。
        description: String,
        /// 系统提示词组件是否发生了变更。
        system_prompt_changed: bool,
        /// 工具集组件是否发生了变更。
        tools_changed: bool,
        /// 整体前缀稳定性百分比（100 = 完全稳定）。
        stability_pct: u32,
        /// 当前缀实际发生变更时为 true（缓存失效）。
        /// 对于常规的稳定性检查心跳为 false。
        changed: bool,
        /// 当前固定前缀的组合哈希值（SHA-256，64 位十六进制字符）。
        /// 携带此字段以便 `/cache stats` 能够展示它，而无需深入访问
        /// 引擎内部的 PrefixStabilityManager。
        pinned_combined_hash: String,
    },
}

impl Event {
    /// 从分类后的错误信封创建一个错误事件。信封自身的
    /// recoverable` 标志控制 UI 是否切换到离线模式。
    pub fn error(envelope: ErrorEnvelope) -> Self {
        let recoverable = envelope.recoverable;
        Event::Error {
            envelope,
            recoverable,
        }
    }

    /// 创建一个新的状态事件
    pub fn status(message: impl Into<String>) -> Self {
        Event::Status {
            message: message.into(),
        }
    }
}
