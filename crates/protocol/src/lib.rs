use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod fleet;
pub mod runtime;
pub mod workroom;

/// 协议层中生命周期状态枚举的通用 trait。
///
/// 每个状态枚举——线程、目标、集群运行、工作者和作业状态——
/// 都实现此 trait，以便通用代码无需匹配所有变体即可询问三个通用问题。
pub trait Status {
    /// 当此状态表示最终、不可继续推进的状态时返回 `true`
    /// （例如 Completed、Failed、Cancelled、Archived、Retired）。
    fn is_terminal(&self) -> bool;

    /// 当工作正在执行中时返回 `true`
    /// （例如 Running、Active、Busy、Queued、Pending）。
    fn is_active(&self) -> bool;

    /// 当事项已被用户或系统显式暂停时返回 `true`（例如 Paused）。
    fn is_paused(&self) -> bool;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub body: T,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Running,
    Idle,
    Completed,
    Failed,
    Paused,
    Archived,
}

impl Status for ThreadStatus {
    fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Archived)
    }
    fn is_active(&self) -> bool {
        matches!(self, Self::Running)
    }
    fn is_paused(&self) -> bool {
        matches!(self, Self::Paused)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    Interactive,
    Resume,
    Fork,
    Api,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub preview: String,
    pub ephemeral: bool,
    pub model_provider: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub status: ThreadStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub cwd: PathBuf,
    pub cli_version: String,
    pub source: SessionSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadGoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}

impl Status for ThreadGoalStatus {
    fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete)
    }
    fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }
    fn is_paused(&self) -> bool {
        matches!(self, Self::Paused)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadGoal {
    pub thread_id: String,
    pub goal_id: String,
    pub objective: String,
    pub status: ThreadGoalStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub continuation_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadStartParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub persist_extended_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadResumeParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality: Option<String>,
    #[serde(default)]
    pub persist_extended_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadForkParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
    #[serde(default)]
    pub persist_extended_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadListParams {
    #[serde(default)]
    pub include_archived: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadReadParams {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadSetNameParams {
    pub thread_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadGoalSetParams {
    pub thread_id: String,
    pub objective: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadGoalGetParams {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadGoalClearParams {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadGoalProgressParams {
    pub thread_id: String,
    #[serde(default)]
    pub token_delta: i64,
    #[serde(default)]
    pub time_delta_seconds: i64,
    #[serde(default)]
    pub record_continuation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThreadRequest {
    Create {
        #[serde(default)]
        metadata: Value,
    },
    Start(ThreadStartParams),
    Resume(ThreadResumeParams),
    Fork(ThreadForkParams),
    List(ThreadListParams),
    Read(ThreadReadParams),
    SetName(ThreadSetNameParams),
    GoalSet(ThreadGoalSetParams),
    GoalGet(ThreadGoalGetParams),
    GoalClear(ThreadGoalClearParams),
    GoalRecordProgress(ThreadGoalProgressParams),
    Archive {
        thread_id: String,
    },
    Unarchive {
        thread_id: String,
    },
    Message {
        thread_id: String,
        input: String,
    },
}

/// 对 [`ThreadRequest`] 的响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadResponse {
    /// 此响应所属的线程。
    pub thread_id: String,
    /// 人类可读的状态字符串（例如 `"ok"`、`"error"`）。
    pub status: String,
    /// 返回单个线程时的线程详情。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<Thread>,
    /// 线程列表，由 `List` 请求填充。
    #[serde(default)]
    pub threads: Vec<Thread>,
    /// 由 get/set 目标请求返回的线程目标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<ThreadGoal>,
    /// 线程使用的模型（如果适用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 线程使用的模型供应商。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    /// 线程的工作目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// 活跃的审批策略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    /// 活跃的沙箱配置。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    /// 与此响应关联的流式事件。
    #[serde(default)]
    pub events: Vec<EventFrame>,
    /// 任意附加响应数据。
    #[serde(default)]
    pub data: Value,
}

/// 不绑定到特定线程的应用级请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppRequest {
    /// 查询服务器能力。
    Capabilities,
    /// 按键读取配置值。
    ConfigGet { key: String },
    /// 设置配置键的值。
    ConfigSet { key: String, value: String },
    /// 移除配置键。
    ConfigUnset { key: String },
    /// 列出所有配置条目。
    ConfigList,
    /// 从磁盘重新加载配置并应用到运行时。
    ///
    /// 重新读取 `config.toml` 和同级 `permissions.toml`，
    /// 刷新运行时的 `Runtime.config` 和 `Runtime.exec_policy`，
    /// 以便无头客户端无需重启即可拾取外部配置文件*和*权限规则的编辑。
    ///
    /// 镜像 TUI 的 `reload_runtime_config` 代码路径，覆盖无头 `Runtime`
    /// 可触及的所有内容。MCP 服务器连接不会刷新——更改 `mcp_config_path`
    /// 或引用的 `mcp.json` 仍需要重启，与 TUI 的 `mcp_restart_required`
    /// 行为一致。
    ConfigReload,
    /// 列出可用模型。
    Models,
    /// 列出当前加载到内存中的线程。
    ThreadLoadedList,
    /// 提交对先前 [`EventFrame::UserInputRequest`] 的答案。
    ///
    /// `request_id` 必须与待处理的澄清请求匹配。无头客户端
    /// 使用此请求将用户的选择返回给运行时。
    SubmitUserInput {
        request_id: String,
        answers: Vec<UserInputAnswerEvent>,
    },
}

/// 对 [`AppRequest`] 的响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppResponse {
    /// 请求是否成功。
    pub ok: bool,
    /// 响应负载。
    pub data: Value,
    /// 与此响应关联的流式事件。
    #[serde(default)]
    pub events: Vec<EventFrame>,
}

/// 一个简单的提示请求，向模型发送文本并返回输出。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRequest {
    /// 可选的提示线程上下文。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// 提示文本。
    pub prompt: String,
    /// 模型覆盖，省略时使用默认模型。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// 对 [`PromptRequest`] 的响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptResponse {
    /// 模型的输出文本。
    pub output: String,
    /// 产生输出的模型。
    pub model: String,
    /// 与此响应关联的流式事件。
    #[serde(default)]
    pub events: Vec<EventFrame>,
}

/// 控制代理在执行前必须征求用户审批的策略。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AskForApproval {
    /// 除非操作在可信路径/资源上，否则请求审批。
    UnlessTrusted,
    /// 仅在工具调用失败后询问。
    OnFailure,
    /// 每次请求工具调用时都询问。
    OnRequest,
    /// 不询问直接拒绝操作，并附上被阻止类别的详情。
    Reject {
        sandbox_approval: bool,
        rules: bool,
        mcp_elicitations: bool,
    },
    /// 从不询问；自动审批所有操作。
    Never,
}

/// 工具调用来源的分类。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// 内置函数工具。
    Function,
    /// MCP（模型上下文协议）工具。
    Mcp,
}

/// 执行本地 shell 命令的参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalShellParams {
    /// 要执行的 shell 命令。
    pub command: String,
    /// 命令的工作目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 超时时间（毫秒）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// 工具调用的负载，按工具类型区分。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolPayload {
    /// 带有 JSON 编码参数的内置函数调用。
    Function { arguments: String },
    /// 带有自由格式输入字符串的自定义工具调用。
    Custom { input: String },
    /// 本地 shell 命令执行。
    LocalShell { params: LocalShellParams },
    /// 针对特定服务器和工具的 MCP 工具调用。
    Mcp {
        server: String,
        tool: String,
        raw_arguments: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_tool_call_id: Option<String>,
    },
}

/// 工具调用的结果，按工具类型区分。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutput {
    /// 内置函数调用的结果。
    Function {
        /// 输出主体（如果有的话）。
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
        /// 调用是否成功。
        success: bool,
    },
    /// MCP 工具调用的结果。
    Mcp {
        /// MCP 服务器返回的结果值。
        result: Value,
    },
}

/// 网络策略规则要执行的操作。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicyRuleAction {
    /// 允许网络访问该主机。
    Allow,
    /// 拒绝网络访问该主机。
    Deny,
}

/// 针对特定主机的网络访问策略的提议修正。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkPolicyAmendment {
    /// 要修改策略的主机。
    pub host: String,
    /// 要应用的操作。
    pub action: NetworkPolicyRuleAction,
}

/// 用户对审批请求的决定。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewDecision {
    /// 批准操作。
    Approved,
    /// 批准并同时修改执行策略。
    ApprovedExecpolicyAmendment,
    /// 仅在此会话剩余时间内批准。
    ApprovedForSession,
    /// 批准并附带网络策略修正。
    NetworkPolicyAmendment {
        host: String,
        action: NetworkPolicyRuleAction,
    },
    /// 拒绝操作。
    Denied,
    /// 中止整个轮次。
    Abort,
}

/// MCP 服务器在启动过程中的状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpStartupStatus {
    /// 服务器正在启动中。
    Starting,
    /// 服务器已准备好接收请求。
    Ready,
    /// 服务器启动失败。
    Failed { error: String },
    /// 启动已取消。
    Cancelled,
}

/// 单个 MCP 服务器启动的进度更新。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupUpdateEvent {
    /// MCP 服务器名称。
    pub server_name: String,
    /// 当前启动状态。
    pub status: McpStartupStatus,
}

/// 启动失败的 MCP 服务器的详情。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupFailure {
    /// 启动失败的 MCP 服务器名称。
    pub server_name: String,
    /// 错误描述。
    pub error: String,
}

/// 所有 MCP 服务器启动完成后发出的事件汇总。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupCompleteEvent {
    /// 成功启动的服务器。
    pub ready: Vec<String>,
    /// 启动失败的服务器。
    pub failed: Vec<McpStartupFailure>,
    /// 启动已被取消的服务器。
    pub cancelled: Vec<String>,
}

/// 需要审批的网络访问请求的上下文。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkApprovalContext {
    /// 正在访问的主机。
    pub host: String,
    /// 网络协议（例如 `"https"`、`"tcp"`）。
    pub protocol: String,
}

/// 在澄清问题中向用户展示的可选项。
///
/// `request_user_input` 模型工具的无头序列化形式，
/// 镜像自 TUI 的 `UserInputOption`。由 [`EventFrame::UserInputRequest`]
/// 帧和 [`AppRequest::SubmitUserInput`] 回复路径共享，
/// 以便两个接口在问题模式上保持一致。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserInputOptionEvent {
    /// 选项的简短标签（也是选中时提交的值）。
    pub label: String,
    /// 与标签一起显示的较长描述。
    pub description: String,
}

/// 向用户提出的单个澄清问题。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserInputQuestionEvent {
    /// 作为问题标题显示的简洁标头。
    pub header: String,
    /// 用于将答案关联到此问题的稳定标识符。
    pub id: String,
    /// 问题正文。
    pub question: String,
    /// 2-4 个建议答案。
    pub options: Vec<UserInputOptionEvent>,
    /// 当为 `true` 时，客户端还应提供自由文本回答。
    #[serde(default)]
    pub allow_free_text: bool,
    /// 当为 `true` 时，用户可以选择多个选项。
    #[serde(default)]
    pub multi_select: bool,
}

/// 通过模型工具调用请求结构化用户输入的事件。
///
/// 与 [`ExecApprovalRequestEvent`] 同属澄清问题流程。
/// 当模型在无头上下文中调用 `request_user_input` 时，
/// 由 `Runtime::invoke_tool` 以 fire-and-return 方式发出。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserInputRequestEvent {
    /// 请求输入的工具调用的标识符。
    pub call_id: String,
    /// 发起请求的轮次。
    pub turn_id: String,
    /// 此用户输入请求的唯一标识符（客户端使用它进行回复）。
    pub request_id: String,
    /// 1-3 个要展示的问题。
    pub questions: Vec<UserInputQuestionEvent>,
}

/// 澄清问题的一个答案。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserInputAnswerEvent {
    /// 此答案对应问题的 `id`。
    pub id: String,
    /// 所选选项的标签，或自由文本回答的 `"Other"`。
    pub label: String,
    /// 解析后的值（选项标签或输入的自由文本）。
    pub value: String,
}

/// 请求用户审批命令执行或补丁应用的事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecApprovalRequestEvent {
    /// 请求审批的工具调用的标识符。
    pub call_id: String,
    /// 此审批请求的唯一标识符。
    pub approval_id: String,
    /// 发起请求的轮次。
    pub turn_id: String,
    /// 将要执行的命令。
    pub command: String,
    /// 命令的工作目录。
    pub cwd: String,
    /// 需要审批的人类可读原因。
    pub reason: String,
    /// 与此审批请求匹配的策略规则（如果有）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<Box<str>>,
    /// 如果审批涉及网络访问，则为网络上下文。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_approval_context: Option<NetworkApprovalContext>,
    /// 提议的执行策略规则修正。
    #[serde(default)]
    pub proposed_execpolicy_amendment: Vec<String>,
    /// 提议的网络策略修正。
    #[serde(default)]
    pub proposed_network_policy_amendments: Vec<NetworkPolicyAmendment>,
    /// 正在请求的额外权限。
    #[serde(default)]
    pub additional_permissions: Vec<String>,
    /// 用户可以选择的决定集合。
    #[serde(default)]
    pub available_decisions: Vec<ReviewDecision>,
}

/// 响应增量被写入的信道。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseChannel {
    /// 主要的可见文本输出。
    #[default]
    Text,
    /// 内部推理/思维链输出。
    Reasoning,
}

impl ResponseChannel {
    /// 如果这是 `Text` 信道则返回 `true`。
    pub const fn is_text(&self) -> bool {
        matches!(self, ResponseChannel::Text)
    }
}

/// 用户针对审批请求发出的审批决定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecisionRequest {
    /// 决定标识符（例如 `"approved"`、`"denied"`）。
    pub decision: String,
    /// 是否记住此决定以便用于未来类似请求。
    #[serde(default)]
    pub remember: bool,
}

/// 代理执行期间发出的单个流式事件帧。
///
/// 事件由 `event` 字段标记，涵盖一个轮次的完整生命周期：
/// 响应流式传输、工具调用、MCP 生命周期、命令执行、
/// 补丁应用、审批和错误。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventFrame {
    /// 新的模型响应已开始。
    ResponseStart { response_id: String },
    /// 进行中响应的增量文本。
    ResponseDelta {
        response_id: String,
        delta: String,
        #[serde(default, skip_serializing_if = "ResponseChannel::is_text")]
        channel: ResponseChannel,
    },
    /// 模型响应已完成。
    ResponseEnd { response_id: String },
    /// 工具调用已开始。
    ToolCallStart {
        response_id: String,
        tool_name: String,
        arguments: Value,
    },
    /// 工具调用已完成并产生结果。
    ToolCallResult {
        response_id: String,
        tool_name: String,
        output: Value,
    },
    /// MCP 服务器启动的进度更新。
    McpStartupUpdate { update: McpStartupUpdateEvent },
    /// 所有 MCP 服务器已完成启动。
    McpStartupComplete { summary: McpStartupCompleteEvent },
    /// MCP 工具调用已开始。
    McpToolCallBegin {
        server_name: String,
        tool_name: String,
    },
    /// MCP 工具调用已结束。
    McpToolCallEnd {
        server_name: String,
        tool_name: String,
        ok: bool,
    },
    /// 需要用户审批以执行命令。
    ExecApprovalRequest { request: ExecApprovalRequestEvent },
    /// 需要用户审批以应用补丁。
    ApplyPatchApprovalRequest { request: ExecApprovalRequestEvent },
    /// 模型工具正在请求用户的结构化澄清输入。
    ///
    /// TUI 的 `request_user_input` 模态流程的无头对应物。
    /// `request_id` 与 [`AppRequest::SubmitUserInput`] 回复关联。
    UserInputRequest { request: UserInputRequestEvent },
    /// MCP 服务器正在请求用户输入（引导式询问）。
    ElicitationRequest {
        server_name: String,
        request_id: String,
        prompt: String,
    },
    /// 命令已开始执行。
    ExecCommandBegin { command: String, cwd: String },
    /// 运行中命令的增量输出。
    ExecCommandOutputDelta { command: String, delta: String },
    /// 命令已执行完毕。
    ExecCommandEnd { command: String, exit_code: i32 },
    /// 补丁已开始应用到文件。
    PatchApplyBegin { path: String },
    /// 补丁已应用完毕。
    PatchApplyEnd { path: String, ok: bool },
    /// 线程内已开始新的轮次。
    TurnStarted { turn_id: String },
    /// 轮次已成功完成。
    TurnComplete { turn_id: String },
    /// 轮次在完成前被中止。
    TurnAborted { turn_id: String, reason: String },
    /// 线程目标已设置或更新。
    ThreadGoalUpdated { goal: ThreadGoal },
    /// 线程目标已清除。
    ThreadGoalCleared { thread_id: String },
    /// 处理过程中发生错误。
    Error {
        response_id: String,
        message: String,
    },
}
