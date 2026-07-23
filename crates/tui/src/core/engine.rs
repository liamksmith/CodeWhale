//! Core engine for `DeepSeek` CLI.
//!
//! 引擎在后台任务中处理所有 AI 交互，通过"通道"与 UI 通信，实现:
//! - API 调用时 UI 不卡顿
//! - 实时的流式更新
//! - 支持取消操作
//! - 工具执行的编排

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};                   // Hash trait，让类型可以被哈希
use std::path::{Path, PathBuf};                  // 文件路径类型
use std::sync::{Arc, Mutex as StdMutex};         // Arc=原子引用计数(多线程共享)
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;                  // anyhow 是 Rust 生态常用的错误处理库
use codewhale_execpolicy::{AskForApproval, ExecPolicyContext};   // 执行策略：询问审批
use codewhale_protocol::runtime::DynamicToolSpec;                // 动态工具规范
use futures_util::StreamExt;                                     // 异步流扩展方法
use futures_util::stream::FuturesUnordered;                      // 无序并发执行多个 Future
use serde_json::{Value, json};                                   // JSON 处理
use tokio::sync::{Mutex as AsyncMutex, RwLock, mpsc};            // tokio = Rust 异步运行时
use tokio_util::sync::CancellationToken;                         // 取消令牌

use crate::client::DeepSeekClient;    // DeepSeek API 客户端
use crate::compaction::{
    CompactionConfig, compact_messages_safe, merge_system_prompts, should_compact,
};                                    // 上下文压缩（长对话时自动缩短）
use crate::config::{ApiProvider, Config, DEFAULT_MAX_SUBAGENTS, DEFAULT_TEXT_MODEL};
use crate::error_taxonomy::{ErrorCategory, ErrorEnvelope, StreamError};
use crate::features::{Feature, Features};
use crate::llm_client::LlmClient;     // LLM 客户端抽象层
use crate::mcp::{McpConfig, McpPool};  // MCP = Model Context Protocol
#[cfg(test)]
use crate::models::ToolCaller;       // 数据模型
use crate::models::{
    ContentBlock, ContentBlockStart, Delta, Message, MessageRequest, StreamEvent, SystemPrompt,
    Tool, Usage,
};
use crate::prompts;
use crate::purge::{emit_purge_completed, emit_purge_failed, emit_purge_started, run_purge};
use crate::resource_telemetry::ResourceTelemetry;
use crate::route_runtime::resolve_runtime_route;
use crate::seam_manager::{SeamConfig, SeamManager};
// 从这些导入可以看出引擎管理着：Goal（目标）、Plan（计划）、Shell（命令行）、SubAgent（子代理）、
// Todo（待办列表）。
use crate::tools::goal::{GoalSnapshot, GoalStatus, SharedGoalState, new_shared_goal_state};
use crate::tools::plan::{PlanSnapshot, SharedPlanState, new_shared_plan_state};
use crate::tools::shell::{SharedShellManager, new_shared_shell_manager};
use crate::tools::spec::RuntimeToolServices;
use crate::tools::spec::{ApprovalRequirement, ToolError, ToolResult};
use crate::tools::subagent::{     // 子代理（让 AI 派生子 AI 去干活）
    Mailbox, MailboxMessage, SharedSubAgentManager, SubAgentCompletion, SubAgentForkContext,
    SubAgentResult, SubAgentRuntime, SubAgentStatus, SubAgentThinking, SubAgentType,
    ensure_subagent_model_for_provider, new_shared_subagent_manager_with_timeout,
    resolve_subagent_assignment_route,
};
use crate::tools::todo::{SharedTodoList, TodoListSnapshot, new_shared_todo_list};
use crate::tools::user_input::{UserInputRequest, UserInputResponse};
use crate::tools::{ToolContext, ToolRegistryBuilder};
use crate::tui::app::AppMode;
use crate::utils::spawn_supervised;
use crate::worker_profile::{ModelRoute, WorkerRuntimeProfile};
use crate::working_set::WorkingSet;

#[cfg(test)]
use super::authority::agent_approval_mode_for_turn;
use super::authority::{TurnAuthority, effective_input_policy, shell_policy_for_mode};
use super::events::{Event, TurnOutcomeStatus};
use super::ops::{
    Op, ProviderRuntimeStatus, SessionSnapshot, USER_SHELL_TOOL_ID_PREFIX, UserInputProvenance,
};
use super::session::Session;
use super::tool_parser;
use super::turn::{TurnContext, post_turn_snapshot, pre_turn_snapshot};

/// Snapshot of parent state that can be passed to forked sub-agents without
/// rewriting the parent transcript.
/// 当主引擎要"复制"自己创建一个子代理时，会拍一张快照——当前模式、工作目录、待办事项、计划、
/// 还有哪些子代理在跑，全部打包传过去。这样子代理就知道"爸爸"在做什么。
#[derive(Debug, Clone, Default)]   // Default：有默认值（字符串默认 ""，Option 默认 None，Vec 默认空列表）
struct StructuredState {
    mode_label: String,                        // 模式标签（如 "agent", "plan"）
    workspace: PathBuf,                        // 工作区路径
    cwd: Option<PathBuf>,                      // 当前工作目录（可选）
    working_set_summary: Option<String>,       // 工作集摘要
    todo_snapshot: Option<TodoListSnapshot>,   // 待办列表快照
    plan_snapshot: Option<PlanSnapshot>,       // 计划快照
    subagent_snapshots: Vec<SubAgentResult>,   // 子代理快照列表
}

impl StructuredState {
    /// 这是一个异步函数（async fn），它捕获当前所有状态并生成一个 StructuredState。
    async fn capture(
        mode_label: impl Into<String>,         // Into<String> 表示接受任何能转为 String 的类型
        workspace: PathBuf,
        cwd: Option<PathBuf>,
        working_set: &WorkingSet,
        todos: &SharedTodoList,
        plan_state: &SharedPlanState,
        subagents: Option<&SharedSubAgentManager>,
    ) -> Self {
        let working_set_summary = working_set.summary_block(&workspace);

        let todo_snapshot = {
            let guard = todos.lock().await;  // 先获取锁（因为 SharedTodoList 是多线程共享的）
            let snap = guard.snapshot();             // 拍快照
            if snap.items.is_empty() {
                None                                                   // 空就返回 None
            } else {
                Some(snap)                                             // 否则包在 Some 里
            }
        };

        let plan_snapshot = {
            let guard = plan_state.lock().await;
            if guard.is_empty() {
                None
            } else {
                Some(guard.snapshot())
            }
        };

        let subagent_snapshots = if let Some(handle) = subagents {
            let mut guard = handle.write().await;
            guard.cleanup(Duration::from_secs(60 * 60));  // 清理超过1小时的旧子代理
            guard
                .list()
                .into_iter()
                .filter(|s| matches!(s.status, SubAgentStatus::Running))
                .collect()  // filter:只保留正在运行的 collect:收集成 Vec
        } else {
            Vec::new()
        };

        Self {
            mode_label: mode_label.into(),
            workspace,
            cwd,
            working_set_summary,
            todo_snapshot,
            plan_snapshot,
            subagent_snapshots,
        }
    }

    /// 把快照转成一段 Markdown 格式的文本，作为"分叉状态"注入到子代理的系统提示里。比如
    /// ## Fork State
    /// - Mode: `agent`
    /// - Workspace: `/path/to/project`
    /// ### Work
    /// - [x] 已完成任务A
    /// - [~] 正在进行任务B
    /// - [ ] 待完成任务C
    #[must_use]  // must_use这个属性表示返回值不能被忽略（编译器会警告）
    fn to_system_block(&self) -> Option<String> {
        let mut out = String::new();
        out.push_str("## Fork State\n\n");
        out.push_str(&format!("- Mode: `{}`\n", self.mode_label));
        out.push_str(&format!("- Workspace: `{}`\n", self.workspace.display()));
        if let Some(cwd) = self.cwd.as_ref() {
            out.push_str(&format!("- Cwd: `{}`\n", cwd.display()));
        }

        if self.todo_snapshot.is_some() || self.plan_snapshot.is_some() {
            out.push_str("\n### Work\n");
        }

        if let Some(todos) = self.todo_snapshot.as_ref() {
            out.push_str(&format!(
                "\nChecklist ({}% complete)\n",
                todos.completion_pct
            ));
            for item in &todos.items {
                let marker = match item.status {
                    crate::tools::todo::TodoStatus::Pending => "[ ]",
                    crate::tools::todo::TodoStatus::InProgress => "[~]",
                    crate::tools::todo::TodoStatus::Completed => "[x]",
                };
                out.push_str(&format!("- {marker} {}\n", item.content));
            }
        }

        if let Some(plan) = self.plan_snapshot.as_ref() {
            out.push_str("\nStrategy metadata\n");
            append_plan_field(&mut out, "Title", plan.title.as_deref());
            append_plan_field(&mut out, "Objective", plan.objective.as_deref());
            append_plan_field(&mut out, "Context", plan.context_summary.as_deref());
            append_plan_field(&mut out, "Explanation", plan.explanation.as_deref());
            append_plan_list(&mut out, "Source", &plan.sources_used);
            append_plan_list(&mut out, "Critical file", &plan.critical_files);
            append_plan_list(&mut out, "Constraint", &plan.constraints);
            append_plan_field(
                &mut out,
                "Recommended approach",
                plan.recommended_approach.as_deref(),
            );
            append_plan_field(
                &mut out,
                "Verification plan",
                plan.verification_plan.as_deref(),
            );
            append_plan_field(
                &mut out,
                "Risks and unknowns",
                plan.risks_and_unknowns.as_deref(),
            );
            append_plan_field(&mut out, "Handoff packet", plan.handoff_packet.as_deref());
            for item in &plan.items {
                let marker = match item.status {
                    crate::tools::plan::StepStatus::Pending => "[ ]",
                    crate::tools::plan::StepStatus::InProgress => "[~]",
                    crate::tools::plan::StepStatus::Completed => "[x]",
                };
                out.push_str(&format!("- {marker} {}\n", item.step));
            }
        }

        if !self.subagent_snapshots.is_empty() {
            out.push_str("\n### Open Sub-Agents\n");
            for s in &self.subagent_snapshots {
                let role = s.assignment.role.as_deref().unwrap_or("-");
                let goal = if s.assignment.objective.is_empty() {
                    "(no objective set)"
                } else {
                    s.assignment.objective.as_str()
                };
                out.push_str(&format!("- `{}` (role: {}) - {}\n", s.agent_id, role, goal));
            }
        }

        if let Some(working_set) = self.working_set_summary.as_deref() {
            out.push('\n');
            out.push_str(working_set);
            out.push('\n');
        }

        Some(out)
    }
}

fn append_plan_field(out: &mut String, label: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        out.push_str(&format!("- {label}: {value}\n"));
    }
}

fn append_plan_list(out: &mut String, label: &str, values: &[String]) {
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            out.push_str(&format!("- {label}: {value}\n"));
        }
    }
}

// === Types ===

/// 这是引擎的配置结构体，极其重要。
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// 使用的模型 ID
    pub model: String,
    /// Route/offering limits for the active provider+model, when the runtime
    /// route resolver had concrete catalog facts.
    /// 活动提供者+模型的路由/服务限制，当运行时路由解析器具有具体的目录事实时。
    pub active_route_limits: Option<codewhale_config::route::RouteLimits>,
    /// 工作区根目录Workspace root for tool execution and file operations.
    pub workspace: PathBuf,
    /// 是否允许执行 shell 命令
    pub allow_shell: bool,
    /// 信任模式（跳过审批）
    pub trust_mode: bool,
    /// notes 工具使用的笔记文件路径。
    pub notes_path: PathBuf,
    /// MCP 配置文件路径。
    pub mcp_config_path: PathBuf,
    /// 包含可发现技能的目录。
    pub skills_dir: PathBuf,
    /// 将技能发现限制为 CodeWhale 拥有的根目录加上显式的 `skills_dir` 配置。
    pub skills_scan_codewhale_only: bool,
    /// 作为 `<instructions source="…">` 块注入系统提示的源（#454）。
    /// 每个条目可以是磁盘路径（在渲染时读取）或内联字符串。
    /// 按用户 `instructions = [...]` 配置的声明顺序加载，或由嵌入器构造。
    ///
    /// 从 `Vec<PathBuf>` 泛化而来，以便嵌入器可以注入内联内容而无需暂存磁盘文件。
    /// `From<PathBuf>` impl 保持现有调用者在调用点使用 `.into()` 正常工作。
    pub instructions: Vec<crate::prompts::InstructionSource>,
    pub project_context_pack_enabled: bool,
    /// 当 true 时，模型被指示以当前语言环境响应，并且后处理翻译层替换剩余的英文输出。
    pub translation_enabled: bool,
    /// 用户可见的转录渲染是否显示思考块。
    /// 提示组装使用此选项来避免本地化隐藏的推理内容。
    pub show_thinking: bool,
    pub verbosity: Option<String>,
    /// 每轮最多执行多少步（默认1000）。
    pub max_steps: u32,
    /// 最大并发子代理数。
    pub max_subagents: usize,
    /// 此引擎会话允许排队的最大子代理数（排队 + 运行中）。
    pub max_admitted_subagents: usize,
    /// 在进一步启动排队等待启动槽之前，可同时执行的直接（深度-1）子代理数量（#3095）。
    /// 从 `[subagents] launch_concurrency` 解析。
    pub launch_concurrency: usize,
    /// 在应用功能标志和 `[subagents]` 选择退出控制后，模型面向的 `agent` 工具是否可用。
    pub subagents_enabled: bool,
    /// 功能开关（哪些工具可用）。
    pub features: Features,
    /// 工具调用的确定性自动审查策略。
    pub auto_review_policy: crate::tui::auto_review::AutoReviewPolicy,
    /// 长对话的自动压缩设置。
    pub compaction: CompactionConfig,
    /// 共享的待办列表
    pub todos: SharedTodoList,
    /// 共享的计划状态
    pub plan_state: SharedPlanState,
    /// 共享的目标状态
    pub goal_state: SharedGoalState,
    /// 子代理最大递归深度（默认3层）。参见 `SubAgentRuntime::max_spawn_depth`。
    /// 可通过 `~/.codewhale/config.toml` 中的 `[subagents] max_depth = N` 覆盖。
    pub max_spawn_depth: u32,
    /// 每个根子代理运行的可选聚合 token 预算。
    /// 后代代理继承根池，除非子代理使用显式的每次调用覆盖启动新的预算范围。
    pub subagent_token_budget: Option<u64>,
    /// 每个域的网络策略决策器（#135）。在整个会话中共享，
    /// 以便会话范围的审批（`/network allow <host>`）在运行剩余时间内持续有效。
    pub network_policy: Option<crate::network_policy::NetworkPolicyDecider>,
    /// 是否每轮拍 git 快照。
    pub snapshots_enabled: bool,
    /// 在首次初始化时快照自动禁用前的最大工作区大小（字节）。`0` 禁用上限。
    /// 在引擎构造时从 `[snapshots] max_workspace_gb` × 1 GB 解析。
    pub snapshots_max_workspace_bytes: u64,
    /// LSP 诊断配置。编辑后 LSP 诊断注入（#136）。当 `None` 时，引擎
    /// 构造一个已禁用的管理器，因此该字段始终存在。
    pub lsp_config: Option<crate::lsp::LspConfig>,
    /// 暴露给模型可见工具的持久运行时服务。
    pub runtime_services: RuntimeToolServices,
    /// 已从配置解析的按角色/类型的子代理模型覆盖。
    pub subagent_model_overrides: HashMap<String, String>,
    /// Fleet 成员名册。合并的舰队名册（内置 + `[fleet.profiles]` + 工作区代理文件），
    /// 由模型生成的子代理和舰队调度共享（#fleet-roster 切换，v0.8.67）。
    /// 默认为仅内置；引擎配置构造站点每个会话加载一次完整名册。
    pub fleet_roster: std::sync::Arc<crate::fleet::roster::FleetRoster>,
    /// 记忆功能是否启用。用户记忆功能是否启用（#489）。当 `true` 时，引擎
    /// 在每次提示组装时读取 `memory_path`，并在系统提示前添加 `<user_memory>` 块。
    pub memory_enabled: bool,
    /// 当 `true` 时，遗留的 `memory.rs` 推送/注入路径被弃用，
    /// 改用 Moraine MCP 召回。`compose_block` 返回 `None`，
    /// 无论 `memory_enabled` 为何值，`remember` 工具不被注册，
    /// `# foo` 快速添加回退。
    pub moraine_fallback: bool,
    /// 记忆文件路径。用户记忆文件路径（#489）。始终填充；仅在 `memory_enabled` 为 `true` 时使用。
    pub memory_path: PathBuf,
    /// Xiaomi MiMo 语音/TTS 工具输出的默认目录。
    pub speech_output_dir: Option<PathBuf>,
    pub vision_config: Option<crate::config::VisionModelConfig>,
    pub goal_objective: Option<String>,
    pub goal_token_budget: Option<u32>,
    pub goal_status: GoalStatus,
    /// 来自自定义斜杠命令前置元数据的工具限制。
    /// `None` 表示当前轮次可以使用正常的工具集。
    pub allowed_tools: Option<Vec<String>>,
    /// 工具拒绝列表。拒绝始终优先于允许（#3027）。
    /// `None` 表示没有工具被显式拒绝。
    pub disallowed_tools: Option<Vec<String>>,
    /// 控制面钩子的钩子执行器。
    /// `ToolCallBefore` 钩子可以以退出码 2 拒绝工具调用。
    pub hook_executor: Option<std::sync::Arc<crate::hooks::HookExecutor>>,
    /// 语言标签（如 "zh-Hans"）。已解析的 BCP-47 语言标签（例如 `"en"`、`"zh-Hans"`、`"ja"`），
    /// 用于系统提示中的 `## Environment` 块。调用者在引擎构造时从 `Settings` 解析一次；
    /// 引擎永远不会为此访问磁盘。
    pub locale_tag: String,
    /// 当 true 时，强制 `tool_choice: "required"` 并将兼容的函数模式加入 DeepSeek beta 严格模式。
    pub strict_tool_mode: bool,
    /// Workshop / 大型工具输出路由（#548）。`None` 禁用路由。
    pub workshop: Option<crate::tools::large_output_router::WorkshopConfig>,
    /// `web_search` 应使用的搜索后端。默认：DuckDuckGo。
    pub search_provider: crate::config::SearchProvider,
    /// Tavily、Bocha、Metaso 或 Baidu 的 API 密钥。`None` 用于 Bing 或 DuckDuckGo。
    /// Metaso 也回退到 `METASO_API_KEY` 环境变量，然后是内置密钥。
    /// Baidu 也回退到 `BAIDU_SEARCH_API_KEY`。
    pub search_api_key: Option<String>,
    /// 可选的 DuckDuckGo 兼容 HTML 端点覆盖。
    pub search_base_url: Option<String>,
    /// 子代理 `create_message` 请求的每步 DeepSeek API 超时。
    /// 在引擎构造时从 `[subagents] api_timeout_secs` 解析一次（限制在 1..=1800），
    /// 然后传递给引擎构建的每个 `SubAgentRuntime`（#1806, #1808）。
    pub subagent_api_timeout: Duration,
    /// 流式模型响应的每个 SSE 块空闲超时。
    /// 从 `[tui].stream_chunk_timeout_secs`（或遗留的 `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS`）解析，
    /// 并通过 `/config` 实时更新。
    pub stream_chunk_timeout: Duration,
    /// 活动子代理的无进展心跳超时。由管理器和父等待循环用于在卡住的子代理
    /// 无限耗尽子代理槽池之前自动取消它们（#2614）。
    pub subagent_heartbeat_timeout: Duration,
    /// 即使在小默认核心表面之外，也应保留在模型可见目录中的原生工具（#2076）。
    pub tools_always_load: HashSet<String>,
    /// 当 true 且 Linux 上存在 `/usr/bin/bwrap` 时，通过 bubblewrap 路由 exec_shell，
    /// 而非仅依赖 Landlock（#2184）。
    #[allow(dead_code)] // 在后续 PR 中通过 ShellManager 连接
    pub prefer_bwrap: bool,
    /// 工具覆盖和插件配置（`config.toml` 中的 `[tools]` 表）。
    /// 在内置工具注册后应用于每轮工具注册表。
    /// 当 `None` 时，不进行覆盖或插件加载。
    pub tools: Option<crate::config::ToolsConfig>,
    /// 工具是否应遵循符号链接。当 `true` 时，基于遍历的工具会遍历符号链接目录，
    /// 且解析到工作区外部的符号链接路径仍然被允许（符号链接本身必须在工作区内）。
    /// 镜像 `workspace_follow_symlinks` 设置。
    pub workspace_follow_symlinks: bool,
    /// 从兄弟 `permissions.toml` 加载的仅询问权限规则。
    pub exec_policy_engine: codewhale_execpolicy::ExecPolicyEngine,
}

impl Default for EngineConfig {   // EngineConfig的Default实现
    fn default() -> Self {
        Self {
            model: DEFAULT_TEXT_MODEL.to_string(),
            active_route_limits: None,
            workspace: PathBuf::from("."),
            allow_shell: true,
            trust_mode: false,
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            skills_dir: crate::skills::default_skills_dir(),
            skills_scan_codewhale_only: false,
            instructions: Vec::new(),
            project_context_pack_enabled: true,
            translation_enabled: false,
            show_thinking: true,
            // High backstop rather than a working ceiling: the in-turn
            // loop_guard that used to brake repetition is gone, so this only
            // exists to terminate a pathological runaway turn via
            // `at_max_steps()`. 1000 stays high enough to never gate real work
            // while still guaranteeing the turn ends.
            max_steps: 1000,
            max_subagents: DEFAULT_MAX_SUBAGENTS,
            max_admitted_subagents: DEFAULT_MAX_SUBAGENTS,
            launch_concurrency: DEFAULT_MAX_SUBAGENTS,
            subagents_enabled: true,
            features: Features::with_defaults(),
            auto_review_policy: crate::tui::auto_review::AutoReviewPolicy::default(),
            compaction: CompactionConfig::default(),
            todos: new_shared_todo_list(),
            plan_state: new_shared_plan_state(),
            goal_state: new_shared_goal_state(),
            max_spawn_depth: crate::tools::subagent::DEFAULT_MAX_SPAWN_DEPTH,
            subagent_token_budget: None,
            network_policy: None,
            snapshots_enabled: true,
            snapshots_max_workspace_bytes:
                crate::snapshot::DEFAULT_MAX_WORKSPACE_BYTES_FOR_SNAPSHOT,
            lsp_config: None,
            runtime_services: RuntimeToolServices::default(),
            subagent_model_overrides: HashMap::new(),
            fleet_roster: std::sync::Arc::new(crate::fleet::roster::FleetRoster::built_ins_only()),
            memory_enabled: false,
            moraine_fallback: false,
            memory_path: PathBuf::from("./memory.md"),
            speech_output_dir: None,
            vision_config: None,
            strict_tool_mode: false,
            goal_objective: None,
            goal_token_budget: None,
            goal_status: GoalStatus::Active,
            allowed_tools: None,
            disallowed_tools: None,
            hook_executor: None,
            locale_tag: "en".to_string(),
            workshop: None,
            search_provider: crate::config::SearchProvider::default(),
            search_api_key: None,
            search_base_url: None,
            subagent_api_timeout: Duration::from_secs(
                crate::config::DEFAULT_SUBAGENT_API_TIMEOUT_SECS,
            ),
            stream_chunk_timeout: Duration::from_secs(
                crate::config::DEFAULT_STREAM_CHUNK_TIMEOUT_SECS,
            ),
            subagent_heartbeat_timeout: Duration::from_secs(
                crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS,
            ),
            tools_always_load: HashSet::new(),
            prefer_bwrap: false,
            verbosity: None,
            tools: None,
            workspace_follow_symlinks: false,
            exec_policy_engine: codewhale_execpolicy::ExecPolicyEngine::new(Vec::new(), Vec::new()),
        }
    }
}

/// Reason the active turn was cancelled. The token from `tokio_util`
/// does not carry a cause, so the engine keeps a sibling latch for
/// approval and user-input waits that need to explain cancellation.
///
/// `External`, `Preempted`, and `Internal` are reserved for the
/// remaining direct cancellation paths tracked in #1541.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CancelReason {
    /// 用户取消（按 Esc等）。User-initiated cancel (Esc, `/cancel`, click cancel on modal). 
    User,
    /// 外部取消（API 调用）。External / runtime-API cancel (HTTP `DELETE /v1/threads/...`,
    /// task manager stop, parent agent cancel).
    External,
    /// 被新请求抢占（用户在新请求完成前又发了一个）。
    /// Cancel triggered when a new turn starts before the previous one
    /// finished — e.g. plain Enter while busy after the queueing path
    /// pre-empts the running turn.
    Preempted,
    /// 引擎内部关闭。Engine internals tore down the turn (drop, channel close,
    /// shutdown). Rare — surfaced as an internal error.
    Internal,
}

impl CancelReason {
    fn describe(self) -> &'static str {
        match self {
            Self::User => "user cancelled the request",
            Self::External => "request cancelled by external caller",
            Self::Preempted => "request was preempted by a new turn",
            Self::Internal => "engine torn down before approval resolved",
        }
    }
}

/// Handle to communicate with the engine
/// 这是理解整个架构最关键的地方。 引擎和 UI 通过 mpsc 通道 通信
/// （mpsc = Multi-Producer, Single-Consumer，多个发送者，一个接收者）。
/// - tx_op：UI → 引擎，发送操作（"用户发了一条消息"）
/// - rx_event：引擎 → UI，推送事件（"AI 开始回复了"）
/// - tx_approval：UI → 引擎，用户点"批准"或"拒绝"
/// - tx_user_input：UI → 引擎，用户填写了表单
#[derive(Clone)]
pub struct EngineHandle {
    /// 发送操作指令给引擎
    pub tx_op: mpsc::Sender<Op>,
    /// 从引擎接收事件
    pub rx_event: Arc<RwLock<mpsc::Receiver<Event>>>,
    /// 取消令牌。Shared pointer to the cancellation token for the current request.
    cancel_token: Arc<StdMutex<CancellationToken>>,
    /// 取消原因。Latched reason for the most recent cancellation. Read by the
    /// approval / user-input handlers to enrich their error strings.
    /// Cleared by the engine when a fresh turn starts.
    cancel_reason: Arc<StdMutex<Option<CancelReason>>>,
    /// 发送审批决定。Send approval decisions to the engine
    tx_approval: mpsc::Sender<ApprovalDecision>,
    /// 发送用户输入。Send user input responses to the engine
    tx_user_input: mpsc::Sender<UserInputDecision>,
    /// 发送转向指令。Send steer input for an in-flight turn.
    tx_steer: mpsc::Sender<String>,
    /// 暂停标志。Shared pause flag set by the TUI and read by the turn loop.
    shared_paused: Arc<StdMutex<bool>>,
}

// `impl EngineHandle { ... }` moved to `engine/handle.rs` so the
// mailbox API can be reviewed independently of the engine internals.

// === Engine ===

/// The core engine that processes operations and emits events
pub struct Engine {
    config: EngineConfig,  // 引擎配置
    api_config: Config,    // API 配置
    deepseek_client: Option<DeepSeekClient>,     // DeepSeek API 客户端
    deepseek_client_error: Option<String>,
    api_key_env_only_recovery: Option<String>,
    session: Session,      // 会话（存储对话历史）
    subagent_manager: SharedSubAgentManager,     // 子代理管理器
    shell_manager: SharedShellManager,           // Shell 管理器
    mcp_pool: Option<Arc<AsyncMutex<McpPool>>>,  // MCP 连接池
    api_provider: ApiProvider,                   // 当前 API 提供商
    active_route_limits: Option<codewhale_config::route::RouteLimits>,
    rx_op: mpsc::Receiver<Op>,                   // 接收操作
    /// Clone of the op-channel sender, so the engine can self-dispatch ops
    /// (e.g. a goal-continuation `SendMessage` after a turn completes).
    tx_op: mpsc::Sender<Op>,                     // 发送操作（自引用）
    rx_approval: mpsc::Receiver<ApprovalDecision>,
    rx_user_input: mpsc::Receiver<UserInputDecision>,
    rx_steer: mpsc::Receiver<String>,
    tx_event: mpsc::Sender<Event>,               // 发送事件
    /// Wakeup channel for the parent turn loop when a direct child sub-agent
    /// terminates (issue #756). Cloned into `SubAgentRuntime` so the runtime
    /// can fan completion events back into the engine.
    tx_subagent_completion: mpsc::UnboundedSender<SubAgentCompletion>,
    /// Receiver paired with `tx_subagent_completion`. Drained at the
    /// turn-loop's empty-tool_uses branch to surface `<codewhale:subagent.done>`
    /// sentinels into the parent's transcript before deciding to end the turn.
    pub(super) rx_subagent_completion: mpsc::UnboundedReceiver<SubAgentCompletion>,
    /// Sub-agent completions already injected into the parent transcript.
    /// Channel delivery and watchdog reconciliation both mark this set so a
    /// dropped event can be synthesized once without duplicating a later
    /// delivery.
    delivered_subagent_completion_ids: HashSet<String>,
    cancel_token: CancellationToken,              // 取消令牌
    shared_cancel_token: Arc<StdMutex<CancellationToken>>,
    /// Latched reason for the current cancellation, mirrored to
    /// `EngineHandle::cancel_reason`. Read by `approval.rs` when
    /// surfacing the "Request cancelled while awaiting …" error so the
    /// user-facing message names a cause.
    pub(super) cancel_reason: Arc<StdMutex<Option<CancelReason>>>,
    tool_exec_lock: Arc<RwLock<()>>,
    /// Append-only layered context manager (#159). Opt-in for v0.7.5 while
    /// cache-hit behavior is audited.
    seam_manager: Option<SeamManager>,
    turn_counter: u64,                            // 对话轮次计数
    /// Post-edit LSP diagnostics injection (#136). Populated unconditionally
    /// — when LSP is disabled in config, this is an inert manager that
    /// always returns `None` from `diagnostics_for`.
    lsp_manager: Arc<crate::lsp::LspManager>,     // LSP 管理器
    /// Session-scoped workshop variable store (#548). Shared across all tool
    /// calls so `last_tool_result` persists within the session and can be
    /// promoted to the parent context via `promote_to_context`.
    workshop_vars: Option<
        std::sync::Arc<tokio::sync::Mutex<crate::tools::large_output_router::WorkshopVariables>>,
    >,
    /// 沙箱后端。External sandbox backend (#516). When `Some`, exec_shell routes commands
    /// through this instead of spawning a local process.
    sandbox_backend: Option<std::sync::Arc<dyn crate::sandbox::backend::SandboxBackend>>,
    /// Diagnostics collected during the current step's tool calls. Drained
    /// and forwarded as a synthetic user message before the next API call.
    pending_lsp_blocks: Vec<crate::lsp::DiagnosticBlock>,
    /// Cached SlopLedger gate block keyed by the ledger file's modified time.
    /// This keeps prompt refreshes cheap while still noticing append/update
    /// writes from slop ledger tools during the same session.
    slop_ledger_gate_cache: Option<(Option<SystemTime>, Option<String>)>,
    /// 当前操作模式(Auto/Yolo/Plan). Updated on `ChangeMode` and `SendMessage`.
    current_mode: AppMode,
    /// Process-local cache for `estimated_input_tokens`. Memoizes the most
    /// recent token estimate keyed on `(session.messages_revision,
    /// system_prompt_fingerprint)`. Five call sites per turn consult this
    /// (engine capacity checkpoints, seam manager, trim budget, etc.) plus
    /// four TUI / command consumers; the cache turns N×O(messages) walks
    /// into a single recompute on a content change.
    token_estimate_cache: TokenEstimateCache,
    /// Shared pause flag set by the TUI and read before tool execution.
    shared_paused: Arc<StdMutex<bool>>,
}

// === Internal tool helpers ===

fn subagent_mailbox_message_is_best_effort(message: &MailboxMessage) -> bool {
    matches!(
        message,
        MailboxMessage::Progress { .. }
            | MailboxMessage::ToolCallStarted { .. }
            | MailboxMessage::ToolCallCompleted { .. }
    )
}

const SUBAGENT_MAILBOX_BEST_EFFORT_MIN_INTERVAL: Duration = Duration::from_millis(100);

fn subagent_mailbox_best_effort_send_permitted(
    last_sent_at: &mut HashMap<String, Instant>,
    message: &MailboxMessage,
    now: Instant,
) -> bool {
    if !subagent_mailbox_message_is_best_effort(message) {
        return true;
    }

    let agent_id = message.agent_id().to_string();
    if last_sent_at
        .get(&agent_id)
        .is_some_and(|last| now.duration_since(*last) < SUBAGENT_MAILBOX_BEST_EFFORT_MIN_INTERVAL)
    {
        return false;
    }

    last_sent_at.insert(agent_id, now);
    true
}

impl Engine {
    /// 根据当前模式（Agent/Yolo/Plan/Operate），返回对应的系统提示词。
    /// 'static 生命周期表示这个字符串在整个程序运行期间都有效。
    fn mode_runtime_instructions(mode: AppMode) -> &'static str {
        match mode {
            AppMode::Agent | AppMode::Auto | AppMode::Yolo => prompts::AGENT_MODE,
            AppMode::Plan => prompts::PLAN_MODE,
            AppMode::Operate => prompts::OPERATE_MODE,
        }
        .trim()
    }

    pub(super) async fn emit_compaction_started(
        &mut self,
        id: String,
        auto: bool,
        message: String,
    ) {
        let _ = self
            .tx_event
            .send(Event::CompactionStarted { id, auto, message })
            .await;
    }

    pub(super) async fn emit_compaction_completed(
        &mut self,
        id: String,
        auto: bool,
        message: String,
        messages_before: Option<usize>,
        messages_after: Option<usize>,
    ) {
        let summary_prompt = self.rendered_compaction_summary();
        let _ = self
            .tx_event
            .send(Event::CompactionCompleted {
                id,
                auto,
                message,
                messages_before,
                messages_after,
                summary_prompt,
            })
            .await;
    }

    /// Render the accumulated compaction summary prompt to plain text so it
    /// can travel in events and be persisted by host layers. All emit sites
    /// run after `merge_compaction_summary`, so this reflects the summary
    /// state the engine will use for subsequent requests.
    fn rendered_compaction_summary(&self) -> Option<String> {
        self.session
            .compaction_summary_prompt
            .as_ref()
            .map(|prompt| match prompt {
                SystemPrompt::Text(text) => text.clone(),
                SystemPrompt::Blocks(blocks) => blocks
                    .iter()
                    .map(|block| block.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            })
            .filter(|text| !text.trim().is_empty())
    }

    pub(super) async fn emit_compaction_failed(&mut self, id: String, auto: bool, message: String) {
        let _ = self
            .tx_event
            .send(Event::CompactionFailed { id, auto, message })
            .await;
    }

    /// 每次新的一轮对话开始前，都要重置取消令牌——上一轮的"取消"不应该影响新一轮。
    /// cancel_token 是 tokio 的取消机制——当用户按 Esc 键时，上一轮的 cancel_token 
    /// 会被触发，导致正在进行的 API 调用被取消。新轮次开始时必须创建一个全新的 token，
    /// 否则新轮次一启动就会立即被"取消"。
    fn reset_cancel_token(&mut self) {
        let token = CancellationToken::new();
        self.cancel_token = token.clone();
        match self.shared_cancel_token.lock() {
            Ok(mut shared) => {
                *shared = token;
            }
            Err(poisoned) => {
                *poisoned.into_inner() = token;
            }
        }
        // Fresh turn → clear any latched cancellation reason from the
        // previous turn so a downstream "request cancelled" message
        // doesn't inherit a stale cause.
        match self.cancel_reason.lock() {
            Ok(mut slot) => *slot = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
        match self.shared_paused.lock() {
            Ok(mut paused) => *paused = false,
            Err(poisoned) => *poisoned.into_inner() = false,
        }
    }

    fn env_only_api_key_recovery_hint(api_config: &Config) -> Option<String> {
        if !crate::config::active_provider_uses_env_only_api_key(api_config) {
            return None;
        }

        let provider = api_config.api_provider();
        let env_var = provider.env_vars_label();

        Some(format!(
            "The rejected key came from {env_var}; no saved config key is present.\n\
             Run `codewhale auth status` to inspect credential sources, then \
             `codewhale auth set --provider {provider}` to save a valid key in ~/.codewhale/config.toml, \
             or remove the stale export and open a fresh shell.",
            provider = provider.as_str()
        ))
    }

    pub(super) fn decorate_auth_error_message(&self, message: String) -> String {
        let Some(hint) = self.api_key_env_only_recovery.as_ref() else {
            return message;
        };
        if crate::error_taxonomy::classify_error_message(&message) != ErrorCategory::Authentication
            || message.contains("no saved config key is present")
        {
            return message;
        }
        format!("{message}\n\n{hint}")
    }

    /// 当用户切换模型或 API 提供商时，这个方法会：
    /// 1) 检查是否需要切换；2) 如果不需要就跳过；3) 如果需要就创建新的客户端连接。
    fn activate_runtime_route(&mut self, provider: ApiProvider, model: &str) -> Result<(), String> {
        if self.api_provider == provider
            && self
                .deepseek_client
                .as_ref()
                .is_some_and(|client| client.api_provider() == provider)
        {
            return Ok(());
        }

        let route =
            resolve_runtime_route(&self.api_config, provider, Some(model)).map_err(|reason| {
                format!(
                    "Failed to resolve provider route {} / {}: {reason}",
                    provider.as_str(),
                    model
                )
            })?;
        let route_config = route.config;
        match DeepSeekClient::from_candidate(&route_config, &route.candidate) {
            Ok(client) => {
                self.api_provider = provider;
                self.api_config = route_config;
                self.active_route_limits =
                    crate::route_budget::known_route_limits(route.candidate.limits);
                self.api_key_env_only_recovery =
                    Self::env_only_api_key_recovery_hint(&self.api_config);
                self.deepseek_client = Some(client.clone());
                self.deepseek_client_error = None;
                self.seam_manager = self
                    .seam_manager
                    .as_ref()
                    .filter(|manager| manager.config().enabled)
                    .map(|manager| SeamManager::new(client, manager.config().clone()));
                Ok(())
            }
            Err(err) => Err(format!(
                "Failed to configure provider route {} / {}: {err}",
                provider.as_str(),
                model
            )),
        }
    }

    /// 这是整个文件中最重要的函数之一。引擎的构造函数。Create a new engine with the given configuration
    /// 返回 (Self, EngineHandle) 是一个元组——同时返回引擎本身和它的控制句柄。
    pub fn new(config: EngineConfig, api_config: &Config) -> (Self, EngineHandle) {
        crate::tls::ensure_rustls_crypto_provider();

        if let Some(objective) = normalized_goal_objective(config.goal_objective.as_deref()) {
            sync_goal_state_from_host(
                &config.goal_state,           // 共享的目标状态（多线程安全）
                Some(&objective),  // 目标描述文本
                config.goal_token_budget,     // token 预算上限
                config.goal_status,           // 目标状态：Active / Paused / Completed / Blocked 
            );
        }

        // mpsc::channel(N) 创建一个通道，N 是缓冲区大小。返回 (发送端, 接收端)。这是 Rust 异步编程的核心模式。
        let (tx_op, rx_op) = mpsc::channel(32);  // 操作通道，缓冲区32
        let (tx_event, rx_event) = mpsc::channel(256);  // 事件通道，缓冲区256
        let (tx_approval, rx_approval) = mpsc::channel(64);
        let (tx_user_input, rx_user_input) = mpsc::channel(32);
        let (tx_steer, rx_steer) = mpsc::channel(64);
        let (tx_subagent_completion, rx_subagent_completion) = mpsc::unbounded_channel();
        // 创建取消令牌，并放在 Arc<Mutex<>> 里，让引擎和句柄都能访问同一个令牌。
        let cancel_token = CancellationToken::new();
        let shared_cancel_token = Arc::new(StdMutex::new(cancel_token.clone()));
        let cancel_reason: Arc<StdMutex<Option<CancelReason>>> = Arc::new(StdMutex::new(None));
        let shared_paused = Arc::new(StdMutex::new(false));
        let tool_exec_lock = Arc::new(RwLock::new(()));

        // Create clients for both providers
        let (deepseek_client, deepseek_client_error) = match DeepSeekClient::new(api_config) {
            Ok(client) => (Some(client), None),
            Err(err) => (None, Some(err.to_string())),
        };
        let api_provider = api_config.api_provider();
        let api_key_env_only_recovery = Self::env_only_api_key_recovery_hint(api_config);

        let mut session = Session::new(
            config.model.clone(),
            config.workspace.clone(),
            config.allow_shell,
            config.trust_mode,
            config.notes_path.clone(),
            config.mcp_config_path.clone(),
        );
        // 使用项目上下文设置稳定的系统提示词（默认为 agent 模式）。
        // 每轮的工作集元数据会在请求时注入到最新的用户消息中，
        // 这样文件变动就不会重写这个前缀。
        let user_memory_block = crate::memory::compose_block(
            config.memory_enabled && !config.moraine_fallback, // TODO(v0.8.71): remove when Moraine recall stable; see #3490, #3495
            &config.memory_path,
        );
        let prompt_goal_objective =
            goal_objective_for_prompt(config.goal_objective.as_deref(), &config.goal_state);
        // 构建系统提示词（System Prompt）——这是发给 AI 的"规则说明书"。包括：工作区信息、技能目录、用户记忆、
        // 项目上下文、语言设置、模型 ID 等。
        let system_prompt =
            prompts::system_prompt_for_mode_with_context_skills_session_and_approval(
                &config.workspace,
                None,
                Some(&config.skills_dir),
                Some(&config.instructions),
                prompts::PromptSessionContext {
                    user_memory_block: user_memory_block.as_deref(),
                    goal_objective: prompt_goal_objective.as_deref(),
                    project_context_pack_enabled: config.project_context_pack_enabled,
                    locale_tag: &config.locale_tag,
                    translation_enabled: config.translation_enabled,
                    model_id: &config.model,
                    context_window_override: Some(
                        crate::route_budget::route_context_window_tokens(
                            api_provider,
                            &config.model,
                            config.active_route_limits,
                        ),
                    ),
                    show_thinking: config.show_thinking,
                    verbosity: config.verbosity.as_deref(),
                    skills_scan_codewhale_only: config.skills_scan_codewhale_only,
                },
            );
        let stable_prompt = Some(system_prompt);
        session.last_system_prompt_hash = Some(system_prompt_hash(stable_prompt.as_ref()));
        session.system_prompt = stable_prompt;

        // Initialize prefix-cache stability monitor (lazy-pin).
        // The system prompt is available now but the tool catalog isn't
        // fully built until the first turn, so we start unpinned. The
        // first `check_and_update` call in the turn loop will pin the
        // fingerprint automatically.
        let _ = session.prefix_stability.get_or_insert_with(|| {
            // Use the tool registry's spec names for fingerprinting.
            // At this point tool spec builders may not be registered yet,
            // so we start with None — fingerprint will pin on first request.
            crate::prefix_cache::PrefixStabilityManager::new_unpinned()
        });
        
        // 创建子代理管理器，告诉它能同时跑几个子代理、心跳超时多久等。
        let subagent_manager = new_shared_subagent_manager_with_timeout(
            config.workspace.clone(),
            config.max_subagents,
            config.max_admitted_subagents,
            config.subagent_heartbeat_timeout,
            config.launch_concurrency,
            config.subagent_token_budget,
        );
        let shell_manager = config
            .runtime_services
            .shell_manager
            .clone()
            .unwrap_or_else(|| new_shared_shell_manager(config.workspace.clone()));
        // Create Flash seam manager for layered context (#159). v0.7.5 keeps
        // this opt-in until the prefix-cache audit proves when seam production
        // is worth the extra request and transcript mutation.
        let seam_manager = deepseek_client.as_ref().map(|main_client| {
            let seam_config = SeamConfig {
                enabled: api_config.context.enabled.unwrap_or(false),
                verbatim_window_turns: api_config
                    .context
                    .verbatim_window_turns
                    .unwrap_or(crate::seam_manager::VERBATIM_WINDOW_TURNS),
                l1_threshold: api_config
                    .context
                    .l1_threshold
                    .unwrap_or(crate::seam_manager::DEFAULT_L1_THRESHOLD),
                l2_threshold: api_config
                    .context
                    .l2_threshold
                    .unwrap_or(crate::seam_manager::DEFAULT_L2_THRESHOLD),
                l3_threshold: api_config
                    .context
                    .l3_threshold
                    .unwrap_or(crate::seam_manager::DEFAULT_L3_THRESHOLD),
                seam_model: api_config
                    .context
                    .seam_model
                    .clone()
                    .unwrap_or_else(|| crate::seam_manager::DEFAULT_SEAM_MODEL.to_string()),
            };
            SeamManager::new(main_client.clone(), seam_config)
        });

        let lsp_manager = Arc::new(match config.lsp_config.clone() {
            Some(cfg) => crate::lsp::LspManager::new(cfg, config.workspace.clone()),
            None => crate::lsp::LspManager::disabled(),
        });

        // Workshop variable store (#548). Created unconditionally so the Arc
        // can be handed to every ToolContext; routing is gated on the router
        // field being Some rather than on the vars Arc being present.
        let workshop_vars: Option<
            std::sync::Arc<
                tokio::sync::Mutex<crate::tools::large_output_router::WorkshopVariables>,
            >,
        > = if config.workshop.is_some() {
            Some(std::sync::Arc::new(tokio::sync::Mutex::new(
                crate::tools::large_output_router::WorkshopVariables::default(),
            )))
        } else {
            None
        };

        // External sandbox backend (#516). Logged but non-fatal: if the
        // backend fails to construct, the engine continues with local
        // execution as the fallback.
        let sandbox_backend = crate::sandbox::backend::create_backend(api_config)
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to create sandbox backend: {e}");
                None
            })
            .map(std::sync::Arc::from);

        let active_route_limits = config.active_route_limits;
        let engine = Engine {
            config,
            api_config: api_config.clone(),
            deepseek_client,
            deepseek_client_error,
            api_key_env_only_recovery,
            session,
            subagent_manager,
            shell_manager,
            mcp_pool: None,
            api_provider,
            active_route_limits,
            rx_op,
            tx_op: tx_op.clone(),
            rx_approval,
            rx_user_input,
            rx_steer,
            tx_event,
            tx_subagent_completion,
            rx_subagent_completion,
            delivered_subagent_completion_ids: HashSet::new(),
            cancel_token: cancel_token.clone(),
            shared_cancel_token: shared_cancel_token.clone(),
            cancel_reason: cancel_reason.clone(),
            tool_exec_lock,
            seam_manager,
            turn_counter: 0,
            lsp_manager,
            pending_lsp_blocks: Vec::new(),
            slop_ledger_gate_cache: None,
            workshop_vars,
            sandbox_backend,
            current_mode: AppMode::Agent,
            token_estimate_cache: TokenEstimateCache::new(),
            shared_paused: shared_paused.clone(),
        };
        let handle = EngineHandle {
            tx_op,
            rx_event: Arc::new(RwLock::new(rx_event)),
            cancel_token: shared_cancel_token,
            cancel_reason,
            tx_approval,
            tx_user_input,
            tx_steer,
            shared_paused,
        };

        (engine, handle)
    }

    /// 这是处理用户直接执行的 shell 命令（如 !git status）的方法。
    async fn handle_run_shell_command(
        &mut self,
        command: String,
        mode: AppMode,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: crate::tui::approval::ApprovalMode,
    ) {
        self.reset_cancel_token();   // 重置取消
        self.turn_counter = self.turn_counter.saturating_add(1);   // 轮次+1（防溢出）saturating_add 和 + 的区别：如果溢出（u64 最大值 + 1），+ 在 debug 模式下会 panic，saturating_add 会停在最大值。

        let turn_id = format!(
            "{}{seq}",
            USER_SHELL_TOOL_ID_PREFIX,
            seq = self.turn_counter
        );
        let tool_id = turn_id.clone();
        let tool_name = "exec_shell".to_string();
        let tool_input = json!({ "command": command, "source": "user" });
        let snapshot_prompt = tool_input["command"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        
        // 根据当前模式构建权限级别（能不能执行命令、需不需要审批）。
        let authority = TurnAuthority::from_effective_fields(
            mode,
            allow_shell,
            trust_mode,
            auto_approve,
            approval_mode,
        );
        self.apply_runtime_mode_policy(&authority);

        let _ = self
            .tx_event
            .send(Event::TurnStarted {
                turn_id: turn_id.clone(),
            })
            .await;

        if self.config.snapshots_enabled {
            let pre_workspace = self.session.workspace.clone();
            let pre_seq = self.turn_counter;
            let pre_cap = self.config.snapshots_max_workspace_bytes;
            let pre_prompt = snapshot_prompt.clone();
            let _ = tokio::task::spawn_blocking(move || {
                pre_turn_snapshot(&pre_workspace, pre_seq, pre_cap, Some(&pre_prompt))
            })
            .await;
        }

        let _ = self
            .tx_event
            .send(Event::ToolCallStarted {
                id: tool_id.clone(),
                name: tool_name.clone(),
                input: tool_input.clone(),
            })
            .await;

        let tool_context = self.build_tool_context(mode, auto_approve);
        let registry = ToolRegistryBuilder::new()
            .with_shell_tools()
            .build(tool_context);

        let result = if mode == AppMode::Plan {
            Err(ToolError::permission_denied(   // Plan 模式下不能执行 shell
                "Tool 'exec_shell' is unavailable in Plan mode".to_string(),
            ))
        } else if !self.config.features.enabled(Feature::ShellTool) {
            Err(ToolError::not_available(       // Shell 功能被禁用了
                "Tool 'exec_shell' is disabled by feature flag".to_string(),
            ))
        } else if let Some(spec) = registry.get(&tool_name) {
            // 尝试从工具注册表中获取该工具，如果存在就执行
            let mut approval_required = spec.approval_requirement_for(&tool_input)
                != ApprovalRequirement::Auto
                && !registry.context().auto_approve;
            let mut approval_description = spec.description().to_string();
            let mut approval_force_prompt = false;
            let ask_rule_decision = exec_shell_ask_rule_decision(
                &self.config,
                &tool_name,
                &tool_input,
                &self.session.workspace,
                self.session.approval_mode,
            );
            if let Some(ToolAskRuleDecision::Prompt(reason)) = ask_rule_decision.as_ref() {
                // YOLO mode (auto_approve) is the explicit "no approvals"
                // contract: a typed ask-rule must not pop a modal in YOLO.
                // A typed deny rule still blocks hard below.
                if !self.session.auto_approve {
                    approval_required = true;
                    approval_description = reason.clone();
                    approval_force_prompt = true;
                }
            }
            if let Some(ToolAskRuleDecision::Block(reason)) = ask_rule_decision {
                Err(ToolError::permission_denied(reason))
            } else if approval_required {
                emit_tool_audit(json!({
                    "event": "tool.approval_required",
                    "tool_id": tool_id.clone(),
                    "tool_name": tool_name.clone(),
                    "source": "composer_bang",
                }));
                let approval_key =
                    crate::tools::approval_cache::build_approval_key(&tool_name, &tool_input).0;
                let approval_grouping_key =
                    crate::tools::approval_cache::build_approval_grouping_key(
                        &tool_name,
                        &tool_input,
                    )
                    .0;
                let _ = self
                    .tx_event
                    .send(Event::ApprovalRequired {
                        id: tool_id.clone(),
                        tool_name: tool_name.clone(),
                        input: tool_input.clone(),
                        description: approval_description,
                        approval_key,
                        approval_grouping_key,
                        intent_summary: None,
                        approval_force_prompt,
                    })
                    .await;

                match self.await_tool_approval(&tool_id).await {
                    Ok(ApprovalResult::Approved) => {
                        emit_tool_audit(json!({
                            "event": "tool.approval_decision",
                            "tool_id": tool_id.clone(),
                            "tool_name": tool_name.clone(),
                            "decision": "approved",
                            "source": "composer_bang",
                        }));
                        let mut result = Self::execute_tool_with_lock(
                            self.tool_exec_lock.clone(),
                            spec.supports_parallel(),
                            false,
                            self.tx_event.clone(),
                            tool_name.clone(),
                            tool_input.clone(),
                            self.session.workspace.clone(),
                            Some(&registry),
                            None,
                            None,
                        )
                        .await;
                        if let Ok(tool_result) = result.as_mut() {
                            stamp_tool_result_approval(
                                tool_result,
                                ToolApprovalStamp::ApprovedByUser,
                            );
                        }
                        result
                    }
                    Ok(ApprovalResult::Denied) => {
                        emit_tool_audit(json!({
                            "event": "tool.approval_decision",
                            "tool_id": tool_id.clone(),
                            "tool_name": tool_name.clone(),
                            "decision": "denied",
                            "source": "composer_bang",
                        }));
                        Err(ToolError::permission_denied(format!(
                            "Tool '{tool_name}' denied by user"
                        )))
                    }
                    Ok(ApprovalResult::RetryWithPolicy(policy)) => {
                        emit_tool_audit(json!({
                            "event": "tool.approval_decision",
                            "tool_id": tool_id.clone(),
                            "tool_name": tool_name.clone(),
                            "decision": "retry_with_policy",
                            "policy": format!("{policy:?}"),
                            "source": "composer_bang",
                        }));
                        let elevated_context = registry
                            .context()
                            .clone()
                            .with_elevated_sandbox_policy(policy);
                        // 审批通过后，真正执行工具
                        // 工具执行锁：Arc<RwLock<()>> 确保同一时刻只有一个工具在执行，避免并发修改文件等冲突。
                        let mut result = Self::execute_tool_with_lock(
                            self.tool_exec_lock.clone(),  // 工具执行锁（同一时刻只执行一个）
                            spec.supports_parallel(),     // 是否支持并行
                            false, 
                            self.tx_event.clone(),        // 事件发送器
                            tool_name.clone(),
                            tool_input.clone(),
                            self.session.workspace.clone(),
                            Some(&registry),
                            None,
                            Some(elevated_context),
                        )
                        .await;
                        if let Ok(tool_result) = result.as_mut() {
                            stamp_tool_result_approval(
                                tool_result,
                                ToolApprovalStamp::ApprovedWithPolicy,
                            );
                        }
                        result
                    }
                    Err(err) => Err(err),
                }
            } else {
                Self::execute_tool_with_lock(
                    self.tool_exec_lock.clone(),
                    spec.supports_parallel(),
                    false,
                    self.tx_event.clone(),
                    tool_name.clone(),
                    tool_input.clone(),
                    self.session.workspace.clone(),
                    Some(&registry),
                    None,
                    None,
                )
                .await
            }
        } else {
            Err(ToolError::not_available(
                "tool 'exec_shell' is not registered".to_string(),
            ))
        };

        let mut result = result;
        if let Ok(tool_result) = result.as_mut()
            && let Some(path) = crate::tools::truncate::apply_spillover_with_artifact(
                tool_result,
                &tool_id,
                &tool_name,
                &self.session.id,
            )
        {
            emit_tool_audit(json!({
                "event": "tool.spillover",
                "tool_id": tool_id.clone(),
                "tool_name": tool_name.clone(),
                "path": path.display().to_string(),
                "source": "composer_bang",
            }));
        }

        let status = if result.is_err() {
            TurnOutcomeStatus::Failed
        } else {
            TurnOutcomeStatus::Completed
        };
        let error = result.as_ref().err().map(ToString::to_string);

        let _ = self
            .tx_event
            .send(Event::ToolCallComplete {
                id: tool_id,
                name: tool_name,
                result,
            })
            .await;

        let _ = self
            .tx_event
            .send(Event::TurnComplete {
                usage: Usage::default(),
                status,
                error,
                tool_catalog: None,
                base_url: None,
            })
            .await;

        if self.config.snapshots_enabled {
            let post_workspace = self.session.workspace.clone();
            let post_seq = self.turn_counter;
            let post_cap = self.config.snapshots_max_workspace_bytes;
            crate::utils::spawn_blocking_supervised("post-shell-turn-snapshot", move || {
                post_turn_snapshot(&post_workspace, post_seq, post_cap, Some(&snapshot_prompt));
            });
        }
    }

    /// 每次新回合开始时，根据权限配置刷新引擎的行为模式。
    /// 把 TurnAuthority 中的 mode、allow_shell、trust_mode、auto_approve
    /// 同步到 engine 和 session 的状态中
    fn apply_runtime_mode_policy(&mut self, authority: &TurnAuthority) {
        self.current_mode = authority.mode;
        self.session.allow_shell = authority.allow_shell;
        self.config.allow_shell = authority.allow_shell;
        self.session.trust_mode = authority.trust_mode;
        self.config.trust_mode = authority.trust_mode;
        self.session.auto_approve = authority.auto_approve;
        self.session.approval_mode = authority.approval_mode_for_session();
    }

    /// Run the engine event loop
    /// 主事件循环
    /// 这是引擎的心脏。是一个无限循环，不断从通道接收操作指令并分发处理。
    #[allow(clippy::too_many_lines)]
    pub async fn run(mut self) {
        enum EngineRunInput {
            Operation(Op),
            SubAgentCompletion(SubAgentCompletion),
        }

        loop {
            // tokio::select! 同时等待多个异步操作，谁先完成就执行谁
            let input = tokio::select! {
                op = self.rx_op.recv() => op.map(EngineRunInput::Operation),
                completion = self.rx_subagent_completion.recv() => {   // 子代理完成任务
                    completion.map(EngineRunInput::SubAgentCompletion)
                }
            };
            let Some(input) = input else {
                break;
            };

            match input {
                EngineRunInput::SubAgentCompletion(completion) => {
                    self.handle_idle_subagent_completion(completion).await;
                }
                EngineRunInput::Operation(op) => match op {
                    Op::SendMessage {   // 用户发送消息（最常用）
                        content,
                        mode,
                        provider,
                        model,
                        goal_objective,
                        goal_token_budget,
                        goal_status,
                        reasoning_effort,
                        reasoning_effort_auto,
                        auto_model,
                        allow_shell,
                        trust_mode,
                        auto_approve,
                        approval_mode,
                        translation_enabled,
                        show_thinking,
                        allowed_tools,
                        dynamic_tools,
                        hook_executor,
                        verbosity,
                        provenance,
                    } => {
                        self.handle_send_message(
                            content,
                            mode,
                            provider,
                            model,
                            goal_objective,
                            goal_token_budget,
                            goal_status,
                            reasoning_effort,
                            reasoning_effort_auto,
                            auto_model,
                            allow_shell,
                            trust_mode,
                            auto_approve,
                            approval_mode,
                            translation_enabled,
                            show_thinking,
                            allowed_tools,
                            dynamic_tools,
                            hook_executor,
                            verbosity,
                            provenance,
                        )
                        .await;
                    }
                    Op::RunShellCommand {  // 用户执行 !command
                        command,
                        mode,
                        allow_shell,
                        trust_mode,
                        auto_approve,
                        approval_mode,
                    } => {
                        self.handle_run_shell_command(
                            command,
                            mode,
                            allow_shell,
                            trust_mode,
                            auto_approve,
                            approval_mode,
                        )
                        .await;
                    }
                    Op::SetGoalStatus { status, clear } => {
                        self.handle_set_goal_status(status, clear).await;
                    }
                    Op::CancelRequest => {  // 取消当前请求
                        self.cancel_token.cancel();
                        self.reset_cancel_token();
                    }
                    Op::ApproveToolCall { id } => {
                        // Tool approval handling will be implemented in tools module
                        let _ = self
                            .tx_event
                            .send(Event::status(format!("Approved tool call: {id}")))
                            .await;
                    }
                    Op::DenyToolCall { id } => {
                        let _ = self
                            .tx_event
                            .send(Event::status(format!("Denied tool call: {id}")))
                            .await;
                    }
                    Op::SpawnSubAgent { prompt } => {   // 生成子代理
                        // 配置子代理运行时、解析模型路由、调用 subagent_manager.spawn()
                        let Some(client) = self.deepseek_client.clone() else {
                            let message = self
                                .deepseek_client_error
                                .as_deref()
                                .map(|err| format!("Failed to spawn sub-agent: {err}"))
                                .unwrap_or_else(|| {
                                    "Failed to spawn sub-agent: API client not configured"
                                        .to_string()
                                });
                            let _ = self
                                .tx_event
                                .send(Event::error(ErrorEnvelope::fatal(message)))
                                .await;
                            continue;
                        };

                        let mcp_pool = if self.config.features.enabled(Feature::Mcp) {
                            self.ensure_mcp_pool().await.ok()
                        } else {
                            None
                        };

                        let mut runtime = SubAgentRuntime::new(
                            client,
                            self.session.model.clone(),
                            // Sub-agents don't inherit YOLO mode - use Agent mode defaults
                            self.build_tool_context(AppMode::Agent, self.session.auto_approve),
                            self.session.allow_shell,
                            Some(self.tx_event.clone()),
                            Arc::clone(&self.subagent_manager),
                        )
                        .with_role_models(self.subagent_role_models())
                        .with_api_config(self.api_config.clone())
                        .with_fleet_roster(self.config.fleet_roster.clone())
                        .with_auto_model(self.session.auto_model)
                        .with_reasoning_effort(
                            self.session.reasoning_effort.clone(),
                            self.session.reasoning_effort_auto,
                        )
                        .with_agent_tool_surface_options(self.agent_tool_surface_options(
                            shell_policy_for_mode(AppMode::Agent, self.session.allow_shell),
                        ))
                        .with_max_spawn_depth(self.config.max_spawn_depth)
                        .with_step_api_timeout(self.config.subagent_api_timeout)
                        .with_speech_output_dir(self.config.speech_output_dir.clone())
                        .with_mcp_pool(mcp_pool)
                        .with_todos(self.config.todos.clone())
                        .with_parent_mode(self.current_mode)
                        .background_runtime();
                        // #4042: thread the session's --disallowed-tools into
                        // the child so tool restrictions flow down to sub-agents.
                        runtime.worker_profile.denied_tools =
                            self.config.disallowed_tools.clone().unwrap_or_default();
                        let route = resolve_subagent_assignment_route(
                            &runtime,
                            None,
                            &prompt,
                            &SubAgentType::General,
                            ModelRoute::Inherit,
                            SubAgentThinking::Inherit,
                        )
                        .await;
                        let effective_model = match ensure_subagent_model_for_provider(
                            &runtime,
                            &route.model_route,
                            route.model,
                        ) {
                            Ok(model) => model,
                            Err(err) => {
                                let _ = self
                                    .tx_event
                                    .send(Event::error(ErrorEnvelope::fatal(format!(
                                        "Failed to spawn sub-agent: {err}"
                                    ))))
                                    .await;
                                continue;
                            }
                        };
                        runtime.model = effective_model;
                        runtime.reasoning_effort = route.reasoning_effort;
                        runtime.reasoning_effort_auto = false;

                        let result = {
                            let mut manager = self.subagent_manager.write().await;
                            manager.spawn_background(
                                Arc::clone(&self.subagent_manager),
                                runtime,
                                SubAgentType::General,
                                prompt.clone(),
                                None,
                            )
                        };

                        match result {
                            Ok(snapshot) => {
                                let _ = self
                                    .tx_event
                                    .send(Event::status(format!(
                                        "Spawned sub-agent {}",
                                        snapshot.agent_id
                                    )))
                                    .await;
                            }
                            Err(err) => {
                                let _ = self
                                    .tx_event
                                    .send(Event::error(ErrorEnvelope::fatal(format!(
                                        "Failed to spawn sub-agent: {err}"
                                    ))))
                                    .await;
                            }
                        }
                    }
                    Op::ListSubAgents => {  // 列出子代理
                        // #3803: the sidebar refresh is a read-only snapshot.
                        // Render from a read lock; only take the write lock to
                        // run cleanup on a bounded cadence, so a UI refresh storm
                        // during a sub-agent fanout no longer contends for the
                        // write lock (against completions/persistence) on every
                        // request. Cleanup still auto-cancels stale agents.
                        let due = {
                            let manager = self.subagent_manager.read().await;
                            manager.cleanup_due(
                                crate::tools::subagent::SUBAGENT_LIST_CLEANUP_MIN_INTERVAL,
                            )
                        };
                        let agents = if due {
                            let mut manager = self.subagent_manager.write().await;
                            manager.cleanup(Duration::from_secs(60 * 60));
                            manager.list()
                        } else {
                            self.subagent_manager.read().await.list()
                        };
                        // #3802: use non-blocking send — this is a refresh event
                        // that can safely be dropped when the channel is full.
                        // The next drain cycle will re-request the list.
                        if let Err(_e) = self.tx_event.try_send(Event::AgentList { agents }) {
                            tracing::debug!(
                                "Event channel full; dropping ListSubAgents refresh (will retry next drain)"
                            );
                        }
                    }
                    Op::CancelSubAgent { agent_id } => {
                        let result = {
                            let mut manager = self.subagent_manager.write().await;
                            match manager.cancel_agent(&agent_id) {
                                Ok(_) => Ok(manager.list()),
                                Err(err) => Err(err),
                            }
                        };
                        match result {
                            Ok(agents) => {
                                if let Err(_e) = self.tx_event.try_send(Event::AgentList { agents })
                                {
                                    tracing::debug!(
                                        "Event channel full; dropping CancelSubAgent refresh"
                                    );
                                }
                            }
                            Err(err) => {
                                let _ =
                                    self.tx_event
                                        .try_send(Event::error(ErrorEnvelope::transient(format!(
                                            "Failed to cancel sub-agent {agent_id}: {err}"
                                        ))));
                            }
                        }
                    }
                    Op::ChangeMode {   // 切换模式
                        mode,
                        allow_shell,
                        trust_mode,
                        auto_approve,
                        approval_mode,
                    } => {
                        let authority = TurnAuthority::from_effective_fields(
                            mode,
                            allow_shell,
                            trust_mode,
                            auto_approve,
                            approval_mode,
                        );
                        self.apply_runtime_mode_policy(&authority);
                        self.emit_session_updated().await;
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Mode changed to: {}",
                                mode.description()
                            )))
                            .await;
                    }
                    Op::SetModel {   // 切换模型
                        model,
                        mode: _,
                        route_limits,
                    } => {
                        self.session.auto_model = model.trim().eq_ignore_ascii_case("auto");
                        self.session.model = model;
                        self.config.model.clone_from(&self.session.model);
                        self.active_route_limits = route_limits;
                        self.refresh_system_prompt();
                        self.emit_session_updated().await;
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Model set to: {}",
                                self.session.model
                            )))
                            .await;
                    }
                    Op::SetCompaction { config } => {
                        // 动态修改压缩配置
                        let enabled = config.enabled;
                        self.config.compaction = config;
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Auto-compaction {}",
                                if enabled { "enabled" } else { "disabled" }
                            )))
                            .await;
                    }
                    Op::SetStreamChunkTimeout { timeout_secs } => {
                        self.config.stream_chunk_timeout = Duration::from_secs(timeout_secs);
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Stream chunk timeout set to {timeout_secs}s"
                            )))
                            .await;
                    }
                    Op::SetSubagentRuntimeConfig {
                        enabled,
                        max_subagents,
                        launch_concurrency,
                        max_spawn_depth,
                        api_timeout_secs,
                        heartbeat_timeout_secs,
                    } => {
                        self.config.subagents_enabled = enabled;
                        self.config.max_subagents =
                            max_subagents.clamp(1, crate::config::MAX_SUBAGENTS);
                        self.config.launch_concurrency =
                            launch_concurrency.clamp(1, self.config.max_subagents);
                        self.config.max_spawn_depth =
                            max_spawn_depth.min(codewhale_config::MAX_SPAWN_DEPTH_CEILING);
                        self.config.subagent_api_timeout = Duration::from_secs(api_timeout_secs);
                        self.config.subagent_heartbeat_timeout =
                            Duration::from_secs(heartbeat_timeout_secs);
                        let launch_gate_applied = {
                            let mut manager = self.subagent_manager.write().await;
                            manager.update_runtime_limits(
                                self.config.max_subagents,
                                self.config.max_admitted_subagents,
                                self.config.subagent_heartbeat_timeout,
                                self.config.launch_concurrency,
                                self.config.subagent_token_budget,
                            )
                        };
                        let launch_note = if launch_gate_applied {
                            ""
                        } else {
                            "; launch_concurrency takes full effect after active sub-agents finish or the session restarts"
                        };
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Sub-agent runtime updated: enabled={enabled}, max_subagents={}, launch_concurrency={}, max_depth={}{}",
                                self.config.max_subagents,
                                self.config.launch_concurrency,
                                self.config.max_spawn_depth,
                                launch_note
                            )))
                            .await;
                    }
                    Op::SyncSession {
                        session_id,
                        messages,
                        system_prompt,
                        system_prompt_override,
                        model,
                        workspace,
                        mode,
                    } => {  // 从外部同步会话消息/系统提示词/工作区
                        if let Some(session_id) = session_id {
                            self.session.id = session_id;
                        } else if messages.is_empty() && system_prompt.is_none() {
                            self.session.id = uuid::Uuid::new_v4().to_string();
                        }
                        self.session.messages = messages.into();
                        self.session.compaction_summary_prompt =
                            extract_compaction_summary_prompt(system_prompt.clone());
                        self.session.system_prompt = system_prompt;
                        self.session.last_system_prompt_hash =
                            Some(system_prompt_hash(self.session.system_prompt.as_ref()));
                        // Host-supplied prompts are persisted prefixes. Keep them
                        // byte-stable; mode/runtime state is projected per request.
                        self.session.system_prompt_override =
                            system_prompt_override && self.session.system_prompt.is_some();
                        self.session.auto_model = model.trim().eq_ignore_ascii_case("auto");
                        self.session.model = model;
                        self.session.workspace = workspace.clone();
                        self.current_mode = mode;
                        self.config.model.clone_from(&self.session.model);
                        self.config.workspace = workspace.clone();
                        let ctx =
                            crate::project_context::load_project_context_with_parents(&workspace);
                        self.session.project_context = if ctx.has_instructions() {
                            Some(ctx)
                        } else {
                            None
                        };
                        self.session.rebuild_working_set();
                        self.emit_session_updated().await;
                        let _ = self
                            .tx_event
                            .send(Event::status("Session context synced".to_string()))
                            .await;
                    }
                    Op::CompactContext => {   // 手动压缩上下文
                        self.handle_manual_compaction().await;
                    }
                    Op::GetSessionSnapshot { tx } => {
                        let total_tokens = self.session.total_usage.input_tokens
                            + self.session.total_usage.output_tokens;
                        let snapshot = SessionSnapshot {
                            messages: self.session.messages.to_vec(),
                            total_tokens,
                            model: self.session.model.clone(),
                            model_provider: self.api_provider.as_str().to_string(),
                            workspace: self.session.workspace.clone(),
                            system_prompt: self.session.system_prompt.clone(),
                            mode: self.current_mode.as_setting().to_string(),
                        };
                        if let Some(tx) = tx.lock().ok().and_then(|mut g| g.take()) {
                            let _ = tx.send(snapshot);
                        }
                    }
                    Op::GetProviderRuntimeStatus { tx } => {
                        let status = if let Some(client) = self.deepseek_client.as_ref() {
                            ProviderRuntimeStatus {
                                provider: client.api_provider(),
                                request_concurrency_limit: client
                                    .provider_request_concurrency_limit(),
                                active_provider_requests: client.active_provider_requests(),
                            }
                        } else {
                            let provider = self.api_config.api_provider();
                            ProviderRuntimeStatus {
                                provider,
                                request_concurrency_limit: self
                                    .api_config
                                    .provider_max_concurrency(provider),
                                active_provider_requests: 0,
                            }
                        };
                        if let Some(tx) = tx.lock().ok().and_then(|mut g| g.take()) {
                            let _ = tx.send(status);
                        }
                    }
                    Op::PurgeContext => {    // 清理上下文
                        self.handle_purge().await;
                    }
                    Op::EditLastTurn { new_message } => {   // 编辑上一轮对话
                        // 截断上一轮对话，替换为用户新输入
                        // #383: /edit — remove the last user+assistant exchange
                        // from the session, then re-send with the new content.
                        // Pop messages from the tail until we've removed the
                        // most recent user message and everything after it.
                        // First, find the last user message index.
                        let mut cut = None;
                        for (idx, msg) in self.session.messages.iter().enumerate().rev() {
                            if msg.role == "user" {
                                cut = Some(idx);
                                break;
                            }
                        }
                        if let Some(idx) = cut {
                            self.session.messages.truncate_to(idx);
                            self.session.bump_messages_revision();
                        }
                        // Now dispatch the new message as a normal send,
                        // reusing the engine's stored mode/model config.
                        let mode = self.current_mode;
                        self.handle_send_message(
                            new_message,
                            mode,
                            Some(self.api_provider),
                            self.session.model.clone(),
                            self.config.goal_objective.clone(),
                            self.config.goal_token_budget,
                            self.config.goal_status,
                            self.session.reasoning_effort.clone(),
                            self.session.reasoning_effort_auto,
                            self.session.auto_model,
                            self.session.allow_shell,
                            self.session.trust_mode,
                            self.session.auto_approve,
                            self.session.approval_mode,
                            self.config.translation_enabled,
                            self.config.show_thinking,
                            self.config.allowed_tools.clone(),
                            Vec::new(),
                            self.config.hook_executor.clone(),
                            self.config.verbosity.clone(),
                            UserInputProvenance::ExternalUser,
                        )
                        .await;
                    }
                    Op::Shutdown => {   // 关闭引擎
                        break;
                    }
                },
            }
        }

        // #freeze: flush any sub-agent checkpoint that the hot-path debounce
        // coalesced away, so a graceful shutdown keeps the latest progress.
        {
            let mut manager = self.subagent_manager.write().await;
            manager.flush_pending_persist();
        }

        // #420: graceful MCP shutdown — send SIGTERM and give stdio servers
        // a brief window to exit before drop fires SIGKILL via kill_on_drop.
        // Best-effort: pool may not exist (no MCP configured) and the lock
        // can fail under contention; either way the kill_on_drop fallback
        // still reaps the children.
        // 关闭 MCP 连接
        if let Some(pool) = self.mcp_pool.as_ref() {
            let mut guard = pool.lock().await;
            guard.shutdown_all().await;
        }
    }

    async fn emit_session_updated(&self) {
        let _ = self
            .tx_event
            .send(Event::SessionUpdated {
                session_id: self.session.id.clone(),
                messages: self.session.messages.clone().into(),
                system_prompt: self.session.system_prompt.clone(),
                model: self.session.model.clone(),
                workspace: self.session.workspace.clone(),
            })
            .await;
    }

    fn goal_snapshot_for_event(&self) -> Option<GoalSnapshot> {
        match self.config.goal_state.lock() {
            Ok(state) => {
                let snapshot = state.snapshot();
                snapshot.objective.is_some().then_some(snapshot)
            }
            Err(err) => {
                tracing::warn!("goal state lock poisoned while emitting goal update: {err}");
                None
            }
        }
    }

    async fn emit_goal_updated(&self) {
        if let Some(snapshot) = self.goal_snapshot_for_event() {
            let _ = self.tx_event.send(Event::GoalUpdated { snapshot }).await;
        }
    }

    fn record_goal_usage_for_turn(&self, usage: &Usage, elapsed: std::time::Duration) {
        let token_delta =
            u64::from(usage.input_tokens).saturating_add(u64::from(usage.output_tokens));
        let time_delta_seconds = elapsed.as_secs();
        if token_delta == 0 && time_delta_seconds == 0 {
            return;
        }
        match self.config.goal_state.lock() {
            Ok(mut state) => state.record_usage(token_delta, time_delta_seconds),
            Err(err) => tracing::warn!("goal state lock poisoned while recording usage: {err}"),
        }
    }

    fn active_input_tokens_with_current_text(&self, current_text: &str) -> usize {
        let mut messages: Vec<Message> = self.session.messages.clone().into();
        if !current_text.trim().is_empty() {
            messages.push(Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: current_text.to_string(),
                    cache_control: None,
                }],
            });
        }
        estimate_input_tokens_conservative(&messages, self.session.system_prompt.as_ref())
    }

    fn append_resource_metadata_lines(
        &self,
        lines: &mut Vec<String>,
        routed_model: &str,
        current_text: &str,
    ) {
        let input_tokens = self.active_input_tokens_with_current_text(current_text);
        if let Some(budget) = route_context_budget_for_route(
            self.api_provider,
            routed_model,
            self.active_route_limits,
            input_tokens,
        ) {
            let usage_percent = budget.usage_percent();
            let escalation = if usage_percent
                >= crate::tui::context_inspector::CONTEXT_CRITICAL_THRESHOLD_PERCENT
            {
                " — CRITICAL: stop expanding scope; run /compact immediately or finish the current task"
            } else if usage_percent
                >= crate::tui::context_inspector::CONTEXT_WARNING_THRESHOLD_PERCENT
            {
                " — ESCALATED: prefer /compact, narrow scope, or finish the current task"
            } else {
                ""
            };
            lines.push(format!(
                "Context pressure: {} ({usage_percent:.1}% used, {} / {} tokens; {} input tokens available){escalation}",
                budget.pressure.label(),
                budget.input_tokens,
                budget.window_tokens,
                budget.available_input_tokens,
            ));
        }

        if let Some(line) = self.session_token_usage_line() {
            lines.push(line);
        }
        if let Some(line) = self.active_goal_resource_line() {
            lines.push(line);
        }
    }

    fn session_token_usage_line(&self) -> Option<String> {
        let usage = &self.session.total_usage;
        let total = usage.input_tokens.saturating_add(usage.output_tokens);
        if total == 0 {
            return None;
        }

        let mut line = format!(
            "Session token usage: {total} total ({} input, {} output)",
            usage.input_tokens, usage.output_tokens,
        );
        if let Some(hit_tokens) = usage.cache_read_input_tokens {
            line.push_str(&format!(", cache hits {hit_tokens}"));
        }
        if let Some(miss_tokens) = usage.cache_creation_input_tokens {
            line.push_str(&format!(", cache misses {miss_tokens}"));
        }
        Some(line)
    }

    fn active_goal_resource_line(&self) -> Option<String> {
        let snapshot = self.config.goal_state.lock().ok()?.snapshot();
        if !snapshot.is_active() {
            return None;
        }

        let mut telemetry =
            ResourceTelemetry::new(snapshot.tokens_used, snapshot.time_used_seconds);
        if let Some(token_budget) = snapshot.token_budget {
            telemetry = telemetry.with_token_budget(u64::from(token_budget));
        }

        let mut line = format!("Active goal resource usage: {}", telemetry.human_summary());
        if snapshot.tokens_used > 0 && snapshot.time_used_seconds > 0 {
            let rate = snapshot.tokens_used as f64 / snapshot.time_used_seconds as f64;
            line.push_str(&format!("; {rate:.1} tok/s"));
        }
        line.push_str(&format!("; {} continuations", snapshot.continuation_count));
        Some(line)
    }

    async fn add_session_message(&mut self, message: Message) {
        self.session.add_message(message);   // 追加到会话历史
        self.emit_session_updated().await;   // 通知 UI 更新
    }

    /// 构建 <turn_meta> XML 块，包含：
    /// - 当前日期
    /// - 工作区路径
    /// - 使用的模型
    /// - 操作模式（Agent/Yolo/Plan）
    /// - 上下文压力（token 使用百分比）
    /// - 会话 token 使用量
    /// - Git 快照信息
    fn turn_metadata_block(
        &self,
        routed_model: &str,
        auto_model: bool,
        reasoning_effort: Option<&str>,
        reasoning_effort_auto: bool,
        provenance: UserInputProvenance,
        current_text: &str,
    ) -> ContentBlock {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let working_set_summary = self
            .session
            .working_set
            .summary_block(&self.config.workspace)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let mut lines = vec![
            format!("Current local date: {today}"),
            // Workspace path moved here from the static `## Environment` block so
            // the static system prefix stays byte-stable across sessions (see
            // `render_environment_block` for the prefix-cache rationale).
            format!("Current workspace: {}", self.config.workspace.display()),
            format!("Current model: {routed_model}"),
            format!("Current mode: {}", self.current_mode.as_setting()),
            "Current mode policy source: runtime".to_string(),
            format!(
                "Current mode policy:\n{}",
                Self::mode_runtime_instructions(self.current_mode)
            ),
            format!("Input provenance: {}", provenance.as_str()),
            format!(
                "Input authority: {}",
                if provenance.can_authorize_work() {
                    "external_current_turn"
                } else {
                    "non_authoritative"
                }
            ),
        ];
        if auto_model {
            lines.push(format!("Auto model route: {routed_model}"));
        }
        if reasoning_effort_auto && let Some(reasoning_effort) = reasoning_effort {
            lines.push(format!("Auto reasoning effort: {reasoning_effort}"));
        }
        self.append_resource_metadata_lines(&mut lines, routed_model, current_text);
        if let Some(working_set_summary) = working_set_summary {
            lines.push(working_set_summary);
        }
        if let Some(git_snapshot) = crate::tui::workspace_context::collect(&self.config.workspace) {
            lines.push(format!("Git workspace: {git_snapshot}"));
        }
        let summary = lines.join("\n");

        ContentBlock::Text {
            text: format!("<turn_meta>\n{summary}\n</turn_meta>"),
            cache_control: None,
        }
    }

    fn user_text_message_with_turn_metadata(&self, text: String) -> Message {
        self.user_text_message_with_turn_metadata_for_route(
            text,
            &self.session.model,
            self.session.auto_model,
            self.session.reasoning_effort.as_deref(),
            self.session.reasoning_effort_auto,
        )
    }

    fn user_text_message_with_turn_metadata_for_route(
        &self,
        text: String,
        routed_model: &str,
        auto_model: bool,
        reasoning_effort: Option<&str>,
        reasoning_effort_auto: bool,
    ) -> Message {
        self.user_text_message_with_turn_metadata_for_route_and_provenance(
            text,
            routed_model,
            auto_model,
            reasoning_effort,
            reasoning_effort_auto,
            UserInputProvenance::ExternalUser,
        )
    }

    fn runtime_text_message_with_turn_metadata(
        &self,
        text: String,
        provenance: UserInputProvenance,
    ) -> Message {
        self.user_text_message_with_turn_metadata_for_route_and_provenance(
            text,
            &self.session.model,
            self.session.auto_model,
            self.session.reasoning_effort.as_deref(),
            self.session.reasoning_effort_auto,
            provenance,
        )
    }

    fn user_text_message_with_turn_metadata_for_route_and_provenance(
        &self,
        text: String,
        routed_model: &str,
        auto_model: bool,
        reasoning_effort: Option<&str>,
        reasoning_effort_auto: bool,
        provenance: UserInputProvenance,
    ) -> Message {
        // 将用户文本放在最前面，turn_meta 放在最后面，这样每条用户消息的起始字节
        // 就不会因为日期、模型路由或工作集的变化而改变。DeepSeek 的 KV 前缀缓存
        // 会从每条消息的开头开始匹配字节序列；当 turn_meta（其中包含当前日期）位
        // 于位置 0 时，整个用户消息前缀在每个日期边界都会失效。将 turn_meta 移到
        // 尾部可以保留用户输入前缀，并将缓存失效的影响限制在末尾的元数据块上。
        let turn_metadata = self.turn_metadata_block(
            routed_model,
            auto_model,
            reasoning_effort,
            reasoning_effort_auto,
            provenance,
            &text,
        );
        Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::Text {
                    text,
                    cache_control: None,
                },
                turn_metadata,
            ],
        }
    }

    /// 这是子代理 → 父代理的通信桥梁。当子代理完成任务时：
    /// 1. 收集所有已完成子代理的结果
    /// 2. 格式化为 <codewhale:subagent.done> 消息
    /// 3. 作为系统消息注入父对话
    /// 4. 重新触发 handle_send_message 让父代理继续
    async fn handle_idle_subagent_completion(&mut self, first: SubAgentCompletion) {
        let mut completions = vec![first];
        while let Ok(completion) = self.rx_subagent_completion.try_recv() {
            completions.push(completion);
        }

        let count = completions.len();
        let content = completions
            .iter()
            .map(|completion| turn_loop::subagent_completion_runtime_text(&completion.payload))
            .collect::<Vec<_>>()
            .join("\n\n");

        let _ = self
            .tx_event
            .send(Event::status(format!(
                "Resuming turn with {count} idle sub-agent completion(s)"
            )))
            .await;

        self.handle_send_message(
            content,
            self.current_mode,
            Some(self.api_provider),
            self.session.model.clone(),
            self.config.goal_objective.clone(),
            self.config.goal_token_budget,
            self.config.goal_status,
            self.session.reasoning_effort.clone(),
            self.session.reasoning_effort_auto,
            self.session.auto_model,
            self.session.allow_shell,
            self.session.trust_mode,
            self.session.auto_approve,
            self.session.approval_mode,
            self.config.translation_enabled,
            self.config.show_thinking,
            self.config.allowed_tools.clone(),
            Vec::new(),
            self.config.hook_executor.clone(),
            self.config.verbosity.clone(),
            UserInputProvenance::SubAgentHandoff,
        )
        .await;
    }

    /// Handle a send message operation
    #[allow(clippy::too_many_arguments)]
    /// Goal Loop（目标循环）：用户设定一个长期目标，每轮结束后引擎自动判断是否需要继续。
    /// 在一个对话轮次完成后，检查是否有活跃的目标（goal）需要继续执行。
    /// 返回一条续传消息以作为新轮次重新分发，如果目标已完成、被阻塞、
    /// 被暂停或超出可选预算，则返回 `None`。
    ///
    /// 续传次数没有上限——目标会一直运行，直到模型自行报告完成/阻塞、
    /// 用户暂停或清除、或者可选的 token/时间预算耗尽为止。
    /// 这个循环是“一直运行到完成”，而不是“运行 N 个轮次”。
    fn goal_continuation_if_active(&self) -> Option<String> {
        let snapshot = self.config.goal_state.lock().ok()?.snapshot();
        if !snapshot.is_active() {
            return None;
        }

        // The snapshot status is a string ("active", "paused", "complete",
        // "blocked"). Map it to the goal-loop decision core's status enum.
        let status = match snapshot.status.as_str() {
            "active" => crate::goal_loop::GoalRunStatus::Active,
            "complete" => crate::goal_loop::GoalRunStatus::Completed,
            // Paused / Blocked / unknown → no continuation.
            _ => return None,
        };

        let decision = crate::goal_loop::decide_continuation(
            status,
            crate::goal_loop::GoalProgress {
                tokens_used: snapshot.tokens_used,
                time_used_seconds: snapshot.time_used_seconds,
                continuations: snapshot.continuation_count,
            },
            crate::goal_loop::GoalBudget {
                token_budget: snapshot.token_budget.map(u64::from),
                time_budget_seconds: None,
            },
        );

        match decision {
            crate::goal_loop::ContinuationDecision::Continue => {
                Some(crate::tools::goal::render_continuation_prompt(
                    &snapshot,
                    snapshot.continuation_count,
                ))
            }
            // All stop reasons → no continuation. The caller (the async turn
            // completion path) emits a status message for budget-exhaustion.
            crate::goal_loop::ContinuationDecision::Stop(reason) => {
                tracing::info!(?reason, "goal continuation stopped");
                None
            }
        }
    }

    /// Handle `/goal pause|resume|clear|complete|blocked` by writing the new
    /// status to `SharedGoalState` so the cross-turn continuation loop respects
    /// it. This does NOT dispatch a model turn — it's a control-plane update.
    async fn handle_set_goal_status(&mut self, status: GoalStatus, clear: bool) {
        match self.config.goal_state.lock() {
            Ok(mut state) => {
                if clear {
                    // `/goal clear` — wipe the objective entirely.
                    state.sync_from_host_status(None, None, GoalStatus::Active);
                } else {
                    // Update only the status; keep the objective and budget.
                    // `sync_from_host_status` resets usage when the objective
                    // changes, but here we pass the existing objective so usage
                    // is preserved (pause/resume shouldn't reset the counter).
                    let objective = state.objective().map(str::to_string);
                    let budget = state.token_budget();
                    state.sync_from_host_status(objective.as_deref(), budget, status);
                }
            }
            Err(err) => {
                tracing::warn!("goal state lock poisoned during SetGoalStatus: {err}");
            }
        }
        let label = if clear {
            "cleared"
        } else {
            match status {
                GoalStatus::Active => "resumed",
                GoalStatus::Paused => "paused",
                GoalStatus::Complete => "complete",
                GoalStatus::Blocked => "blocked",
            }
        };
        let _ = self
            .tx_event
            .send(Event::status(format!("Goal {label}.")))
            .await;
        self.emit_goal_updated().await;
    }

    /// 最核心的消息处理函数
    /// 1. 验证客户端可用（API key 有效？）
    /// 2. 观察工作区文件变化（working set 跟踪）
    /// 3. 将用户消息加入会话历史
    /// 4. 同步目标状态
    /// 5. 应用配置覆盖（每轮都重新读取 .codewhale/config.toml）
    /// 6. 激活 API 路由（选择正确的供应商和模型）
    /// 7. 构建工具注册表（包括子代理运行时、MCP 工具、插件工具）
    /// 8. 刷新系统提示词（用户记忆、目标、SlopLedger 门控）
    /// 9. 检查是否需要上下文压缩
    /// 10. 调用 handle_deepseek_turn 执行 LLM 交互
    /// 11. 记录 token 使用量
    /// 12. 发送 TurnComplete 事件
    /// 13. 如果有活跃目标 → 自动发起下一轮
    /// # Arguments
    /// * `content` 用户输入的文本内容
    /// * `mode` 当前模式（Agent / Plan / Yolo）
    /// * `provider` API 提供商（可选）
    /// * `model` 模型名称
    /// * `goal_objective` 目标描述（可选）
    /// * `goal_token_budget` 目标 token 预算（可选）
    /// * `goal_status` 目标状态（Active / Paused 等）
    /// * `reasoning_effort` 推理深度设置
    /// * `reasoning_effort_auto` 是否自动选择推理深度
    /// * `auto_model` 是否自动选择模型
    /// * `allow_shell` 是否允许 shell 执行
    /// * `trust_mode` 是否信任模式
    /// * `auto_approve` 是否自动批准工具调用
    /// * `approval_mode` 批准模式
    /// * `translation_enabled` 是否启用翻译
    /// * `show_thinking` 是否显示思考过程
    /// * `allowed_tools` 允许的工具白名单
    /// * `dynamic_tools` 动态工具列表
    /// * `hook_executor` Hook 执行器
    /// * `verbosity` 详细程度
    /// * `provenance` 输入来源（用户 / 运行时）
    #[allow(clippy::too_many_arguments)]
    async fn handle_send_message(
        &mut self,
        content: String,
        mode: AppMode,
        provider: Option<ApiProvider>,
        model: String,
        goal_objective: Option<String>,
        goal_token_budget: Option<u32>,
        goal_status: GoalStatus,
        reasoning_effort: Option<String>,
        reasoning_effort_auto: bool,
        auto_model: bool,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: crate::tui::approval::ApprovalMode,
        translation_enabled: bool,
        show_thinking: bool,
        allowed_tools: Option<Vec<String>>,
        dynamic_tools: Vec<DynamicToolSpec>,
        hook_executor: Option<std::sync::Arc<crate::hooks::HookExecutor>>,
        verbosity: Option<String>,
        provenance: UserInputProvenance,
    ) {
        // 调用辅助函数，综合所有参数计算出一个策略对象
        let input_policy = effective_input_policy(
            provenance,
            mode,
            &content,
            allow_shell,
            trust_mode,
            mode == AppMode::Yolo || auto_approve,
            approval_mode,
        );
        
        // 如果策略中包含了要给用户看的状态消息（比如"已切换到 Yolo 模式"），就立即通过事件通道发送出去。
        if let Some(status) = input_policy.status.clone() {
            let _ = self.tx_event.send(Event::status(status)).await;
        }
        // cancel_token 是 tokio 的取消机制——当用户按 Esc 键时，上一轮的 cancel_token 会被触发，
        // 导致正在进行的 API 调用被取消。新轮次开始时必须创建一个全新的 token，否则新轮次一启动就会立即被"取消"。
        self.reset_cancel_token();

        // Track the complete effective mode policy so mid-turn metadata, `/edit`,
        // idle worker resumptions, and approval gates cannot read a stale policy
        // after the UI changed modes (#3568).
        // 记录完整的有效模式策略，防止 UI 切换模式后元数据读取到过期策略
        self.apply_runtime_mode_policy(&input_policy);

        // Drain stale steer messages from previous turns.
        // 排空上一轮残留的转向消息.
        // rx_steer 是一个用于运行时转向（steer）的通道——比如用户在 AI 回复过程中注入新的指令。
        // 新轮次开始前先把旧消息清空，防止污染。
        while self.rx_steer.try_recv().is_ok() {}

        // Create turn context first so start event includes a stable turn id.
        // 创建 TurnContext，包含一个唯一的 turn id
        let mut turn = TurnContext::new(self.config.max_steps);
        self.turn_counter = self.turn_counter.saturating_add(1);  // 递增 turn 计数器

        // 立即发送 TurnStarted 事件——让 UI 知道回合已激活
        // 在 WSL2 等慢文件系统上，快照可能需要 30+ 秒，如果等快照完成再发 TurnStarted，UI 
        // 会长时间卡在"等待中"的状态。所以引擎先告诉 UI"我开始了"，然后再慢慢做快照。
        let _ = self
            .tx_event
            .send(Event::TurnStarted {
                turn_id: turn.id.clone(),
            })
            .await;

        // 在执行任何工具之前对工作区进行快照。将 git 操作放在阻塞线程池中执行，
        // 以保持异步运行时的响应性；失败是非致命的（辅助函数会以 WARN 级别记录）。
        // 快照是什么：用 git 记录工作区在 AI 操作前的状态，之后如果 AI 搞砸了，用户可以用 /restore 恢复。
        if self.config.snapshots_enabled {
            // 现在克隆用户提示 — `content` 随后会被移动到下面的
            // `user_text_message_with_turn_metadata_for_route` 中，因此我们需要
            // 一个副本同时用于轮次前和轮次后的快照标签。该标签会携带截断后的第一行，
            // 使 `/restore` 列表具有可读性。
            let snapshot_prompt = content.clone();
            let pre_workspace = self.session.workspace.clone();
            let pre_seq = self.turn_counter;
            let pre_cap = self.config.snapshots_max_workspace_bytes;
            let _ = tokio::task::spawn_blocking(move || {
                pre_turn_snapshot(&pre_workspace, pre_seq, pre_cap, Some(&snapshot_prompt))
            })
            .await;
        }

        // 确保 UI 底部的"上次操作失败，正在重试..."横幅不会跨轮次残留。 (#499).
        crate::retry_status::clear();

        // 是事后快照的标签——因为 content 马上会被移动到消息构造中（move 语义），所以这里提前克隆一份。
        let snapshot_prompt_post = content.clone();

        // 检查是否有合适的客户端
        // 如果指定了 provider，尝试激活路由；失败则发出错误并返回
        // 这是第一道防线。如果 API 密钥无效、网络不通、或者模型配置错误，这里就会拦截，不会继续执行后续逻辑。
        // 注意返回时一定要发送 TurnComplete 事件——UI 在等着这个事件来解除"加载中"状态。
        if let Some(provider) = provider
            && let Err(message) = self.activate_runtime_route(provider, &model)
        {
            self.deepseek_client_error = Some(message.clone());
            let _ = self
                .tx_event
                .send(Event::error(ErrorEnvelope::fatal_auth(message.clone())))
                .await;
            let _ = self
                .tx_event
                .send(Event::TurnComplete {
                    usage: turn.usage.clone(),
                    status: TurnOutcomeStatus::Failed,
                    error: Some(message),
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
            return;
        }

        if self.deepseek_client.is_none() {
            let message = self
                .deepseek_client_error
                .as_deref()
                .map(|err| format!("Failed to send message: {err}"))
                .unwrap_or_else(|| "Failed to send message: API client not configured".to_string());
            let _ = self
                .tx_event
                .send(Event::error(ErrorEnvelope::fatal_auth(message.clone())))
                .await;
            let _ = self
                .tx_event
                .send(Event::TurnComplete {
                    usage: turn.usage.clone(),
                    status: TurnOutcomeStatus::Failed,
                    error: Some(message),
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
            return;
        }

        // 让 working_set 观察用户消息（用于后续的文件推荐、上下文管理）
        // observe_user_message是上下文智能的一部分——它会分析用户消息中提到哪些文件、哪些路径，之后优先把这些内容保持在上下文中。
        self.session
            .working_set
            .observe_user_message(&content, &self.session.workspace);
        
        // 判断是否需要先更新计划再执行
        // should_force_update_plan_first 检查用户消息是否包含 /plan 或类似的需要先创建/更新计划的指令。
        let force_update_plan_first = should_force_update_plan_first(input_policy.mode, &content);

        // Add user message to session
        // 构造带有 turn 元数据的用户消息
        let user_msg = self.user_text_message_with_turn_metadata_for_route_and_provenance(
            content,
            &model,
            auto_model,
            reasoning_effort.as_deref(),
            reasoning_effort_auto,
            provenance,
        );
        self.session.add_message(user_msg);

        // 保存旧的 goal 配置，用于比较是否有变化
        let previous_goal_objective = self.config.goal_objective.clone();
        let previous_goal_token_budget = self.config.goal_token_budget;
        let previous_goal_status = self.config.goal_status;

        // 更新当前 model 和 goal 配置
        self.session.model = model;
        self.config.model.clone_from(&self.session.model);
        self.config.goal_objective = goal_objective.clone();
        self.config.goal_token_budget = goal_token_budget;
        self.config.goal_status = goal_status;

        // 如果 goal 发生了变化，同步到持久化的 goal_state
        // normalized_goal_objective 做规范化处理，避免仅空格差异触发不必要的同步
        if normalized_goal_objective(previous_goal_objective.as_deref())
            != normalized_goal_objective(goal_objective.as_deref())
            || previous_goal_token_budget != goal_token_budget
            || previous_goal_status != goal_status
        {
            sync_goal_state_from_host(
                &self.config.goal_state,
                normalized_goal_objective(goal_objective.as_deref()).as_deref(),
                goal_token_budget,
                goal_status,
            );
        }
        self.config.allowed_tools = allowed_tools;
        self.config.hook_executor = hook_executor;
        self.session.reasoning_effort = reasoning_effort;
        self.session.reasoning_effort_auto = reasoning_effort_auto;
        self.session.auto_model = auto_model;
        self.config.translation_enabled = translation_enabled;
        self.config.show_thinking = show_thinking;
        self.config.verbosity = verbosity;

        // 刷新系统提示词（因为配置变了），通知 UI 会话已更新（比如会话列表、token 使用量等）。
        self.refresh_system_prompt();
        self.emit_session_updated().await;

        // 这段代码为后续的"工具调用"做准备：
        // Build tool registry and tool list for the current mode
        let todo_list = self.config.todos.clone();
        let plan_state = self.config.plan_state.clone();

        // 构建工具上下文（包含工作区路径、shell 策略等）
        let tool_context = self.build_tool_context(input_policy.mode, input_policy.auto_approve);
        // 在building the tool registry前确保 MCP 连接池已初始化
        // so start_mcp_server can be registered when Feature::Mcp is enabled.
        if self.config.features.enabled(Feature::Mcp) {
            let _ = self.ensure_mcp_pool().await;  // 连接池用于管理外部工具服务器
        }
        // 创建工具注册表构建器
        let builder = self
            .build_turn_tool_registry_builder(input_policy.mode, todo_list, plan_state)
            .with_dynamic_tools(&dynamic_tools);

        // 检查子代理功能是否启用
        let subagents_available =
            self.config.subagents_enabled && self.config.features.enabled(Feature::Subagents);

        // 如果子代理可用，捕获当前会话状态用于 fork 新子代理
        let fork_context_for_runtime = if subagents_available {
            let state = StructuredState::capture(
                input_policy.mode.label(),
                self.config.workspace.clone(),
                std::env::current_dir().ok(),
                &self.session.working_set,
                &self.config.todos,
                &self.config.plan_state,
                Some(&self.subagent_manager),
            )
            .await;
            Some(SubAgentForkContext {
                system: self.session.system_prompt.clone(),
                messages: self.messages_with_turn_metadata(),
                structured_state_block: state.to_system_block(),
            })
        } else {
            None
        };

        // Mailbox for structured sub-agent envelopes (#128/#130). One per
        // turn: the receiver is drained by a short-lived task that converts
        // envelopes into `Event::SubAgentMailbox` so the UI can route them
        // to the matching in-transcript card. The drainer exits naturally
        // when every cloned sender is dropped at turn-end.
        // 为子代理创建邮箱系统，用于子代理向父代理发送结构化消息
        // 这是一个精巧的设计——子代理完成工作后需要向父代理报告，但不能直接写父代理的状态。
        // 于是通过邮箱（Mailbox）传递消息：
        // - 子代理写入 mailbox（sender 端）
        // - drainer 任务从 receiver 端读取，转为 Event::SubAgentMailbox 事件
        // - UI 收到事件后在对应位置渲染子代理结果卡片
        // 消息还区分了"尽力而为"（best-effort）和"必须送达"两种——比如子代理的进度更新属于 
        // best-effort，丢了无所谓；但最终结果必须送达。
        let mailbox_for_runtime = if subagents_available {
            // 子令牌，父被取消时子也取消
            let cancel_token = self.cancel_token.child_token();
            let (mailbox, mut receiver) = Mailbox::new(cancel_token.clone());
            let tx_event_clone = self.tx_event.clone();
            // spawn 一个 drainer 任务，持续接收子代理消息并转为 UI 事件
            spawn_supervised(
                "subagent-mailbox-drainer",
                std::panic::Location::caller(),
                async move {
                    let mut best_effort_sent_at: HashMap<String, Instant> = HashMap::new();
                    while let Some(envelope) = receiver.recv().await {
                        let event = Event::SubAgentMailbox {
                            seq: envelope.seq,
                            message: envelope.message,
                        };
                        // 对"尽力而为"类型消息做频率限制
                        if let Event::SubAgentMailbox { message, .. } = &event
                            && subagent_mailbox_message_is_best_effort(message)
                        {
                            if !subagent_mailbox_best_effort_send_permitted(
                                &mut best_effort_sent_at,
                                message,
                                Instant::now(),
                            ) {
                                continue;  // 频率过高，丢弃
                            }
                            // try_send：如果通道满了就丢弃（不阻塞）
                            match tx_event_clone.try_send(event) {
                                Ok(()) => continue,
                                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => continue,
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                            }
                        }
                        // 非 best_effort 消息，用 send（会等待）
                        if tx_event_clone.send(event).await.is_err() {
                            break;
                        }
                    }
                },
            );
            Some((mailbox, cancel_token))
        } else {
            None
        };

        // MCP 连接池用于管理与外部 MCP 服务器的连接。这里获取一个可克隆的引用，
        // 后续传给子代理运行时，让子代理也能使用 MCP 工具。
        let mcp_pool = if self.config.features.enabled(Feature::Mcp) {
            self.ensure_mcp_pool().await.ok()
        } else {
            None
        };

        let mut tool_registry = if subagents_available {
            // 创建子代理运行时配置
            let runtime = if let Some(client) = self.deepseek_client.clone() {
                let runtime_allow_shell =
                    self.session.allow_shell && !matches!(input_policy.mode, AppMode::Plan);
                let runtime_shell_policy =
                    shell_policy_for_mode(input_policy.mode, runtime_allow_shell);
                let mut rt = SubAgentRuntime::new(
                    client,                               // API 客户端
                    self.session.model.clone(),           // 模型
                    tool_context.clone(),                 // 工具上下文
                    runtime_allow_shell,                  // 是否允许 shell
                    Some(self.tx_event.clone()), // 事件发送通道
                    Arc::clone(&self.subagent_manager), // 子代理管理器
                )
                .with_role_models(self.subagent_role_models())   // 角色模型映射
                .with_api_config(self.api_config.clone())        // API 配置
                .with_fleet_roster(self.config.fleet_roster.clone())    // Fleet 名册
                .with_auto_model(self.session.auto_model)        // 自动模型选择
                .with_reasoning_effort(                                          // 推理深度
                    self.session.reasoning_effort.clone(),
                    self.session.reasoning_effort_auto,
                )
                .with_agent_tool_surface_options(                                // 工具表面选项
                    self.agent_tool_surface_options(runtime_shell_policy),
                )
                .with_max_spawn_depth(self.config.max_spawn_depth)  // 最大嵌套深度
                .with_step_api_timeout(self.config.subagent_api_timeout) // API 超时
                .with_speech_output_dir(self.config.speech_output_dir.clone())  // 语音输出
                .with_mcp_pool(mcp_pool.clone())   // MCP 连接池
                .with_todos(self.config.todos.clone())  // Todo 列表
                .with_parent_completion_tx(self.tx_subagent_completion.clone())   // 父完成通知
                .with_parent_mode(input_policy.mode);
                if matches!(input_policy.mode, AppMode::Plan) {
                    rt.worker_profile = WorkerRuntimeProfile::for_role(SubAgentType::Plan);
                }
                // #4042: stamp the session's --disallowed-tools onto the parent
                // runtime so every model-spawned sub-agent inherits the deny-list
                // (plan-mode role override above is intentionally before this).
                rt.worker_profile.denied_tools =
                    self.config.disallowed_tools.clone().unwrap_or_default();
                if let Some(context) = fork_context_for_runtime.clone() {
                    rt = rt.with_fork_context(context);
                }
                if let Some((mailbox, cancel_token)) = mailbox_for_runtime.as_ref() {
                    rt = rt
                        .with_mailbox(mailbox.clone())
                        .with_cancel_token(cancel_token.clone());
                }
                Some(rt)
            } else {
                None
            };
            
            // 用运行时构建带子代理工具的注册表
            if let Some(subagent_runtime) = runtime {
                Some(
                    builder
                        .with_subagent_tools(self.subagent_manager.clone(), subagent_runtime)
                        .build(tool_context),
                )
            } else {
                // 降级策略：如果 API 客户端不可用（比如未配置），工具注册表仍然会构建，只是不包含子代理工具——引擎不会因为缺少子代理功能就崩溃。
                // 没有 API 客户端则降级为基础工具集
                tracing::warn!(
                    "Sub-agents enabled but no API client available, falling back to basic tool set"
                );
                Some(builder.build(tool_context))
            }
        } else {
            // 子代理禁用时只构建基础工具集
            Some(builder.build(tool_context))
        };

        // 插件工具：用户可以把自己写的脚本放在 ~/.codewhale/tools/ 目录下，引擎会自动发现并注册。
        // 如果 config.toml 中有同名工具的手动配置，手动配置优先。
        // - 工具目录（Tool Catalog）：这是最终发送给 AI 模型的工具列表。
        //   不同的模型有不同的工具承载能力（tool_surface_budget），有的模型只能承载 50 个工具定义，有的能承载 200 个。
        // - 延迟加载（defer_loading）：对于大型工具（如某些 MCP 工具），可以先只发工具名和描述，
        //   等 AI 真正调用时再加载完整定义。但插件工具是用户明确配置的，不延迟。
        // - 门控过滤（filter_tool_catalog_for_gates）：根据 allowed_tools（白名单）和 disallowed_tools（黑名单）筛掉不该出现的工具。
        // - 这里还保存了 tool_catalog_for_event 和 base_url_for_event，在 turn 结束时会随 TurnComplete 事件一起发出。
        let mut plugin_tool_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        if let Some(ref mut tool_registry) = tool_registry {
            plugin_tool_names = configure_plugin_tools(tool_registry, self.config.tools.as_ref());
        }

        // 获取 MCP 工具列表
        let mcp_tools = if self.config.features.enabled(Feature::Mcp) {
            self.mcp_tools().await
        } else {
            Vec::new()
        };

        let tools = tool_registry.as_ref().map(|registry| {
            // 解析模型的工具能力配置
            let capability = crate::model_profile::resolved_capability_profile(
                self.api_config.api_provider(),
                &self.config.model,
            );
            // 如果启用 MCP，强制加载 start_mcp_server 工具
            let mut always_load = self.config.tools_always_load.clone();
            if self.config.features.enabled(Feature::Mcp) {
                always_load.insert("start_mcp_server".to_string());
            }
            // 构建工具目录
            let mut catalog = build_model_tool_catalog_with_surface(
                registry.to_api_tools_with_cache(true),
                mcp_tools,
                input_policy.mode,
                &always_load,
                capability.tool_surface_budget,
            );
            // plugin_tool_names中的插件工具标记为不延迟加载
            for tool in &mut catalog {
                if plugin_tool_names.contains(&tool.name) {
                    tool.defer_loading = Some(false);
                }
            }
            // 根据白名单/黑名单过滤工具
            filter_tool_catalog_for_gates(
                &mut catalog,
                self.config.allowed_tools.as_deref(),
                self.config.disallowed_tools.as_deref(),
            );
            catalog
        });
        // 保存工具目录副本，用于 TurnComplete 事件
        let tool_catalog_for_event = tools.clone();
        let base_url_for_event = self
            .deepseek_client
            .as_ref()
            .map(|client| client.base_url().to_string());

        // Main turn loop. Catch panics here so an internal error surfaces as a
        // failed TurnComplete instead of unwinding through `engine.run()` and
        // killing the whole engine-event-loop task — which left the UI stuck
        // on "working" forever with the engine silently dead (#2583, #1269).
        // 主 turn 循环。捕获 panic，防止引擎事件循环崩溃
        // 这是函数的核心——handle_deepseek_turn 才是真正执行 AI 对话循环的地方（发送 prompt、
        // 接收回复、执行工具调用、再发送结果...如此循环直到 AI 完成或达到 max_steps）。
        // 但更重要的是外层的 catch_unwind。在 Rust 中，panic 会展开（unwind）调用栈。如果不加保护，
        // 引擎内部的 panic 会直接杀死整个事件循环任务，导致 UI 卡死。 #2583 和 #1269 就是这样的 bug。
        // 现在用 AssertUnwindSafe + catch_unwind 包裹后，即使内部 panic，引擎也能优雅地返回一个 
        // TurnOutcomeStatus::Failed，并保存崩溃报告。
        use futures_util::FutureExt as _;
        let turn_result = std::panic::AssertUnwindSafe(self.handle_deepseek_turn(
            &mut turn,
            tool_registry.as_ref(),
            tools,
            input_policy.mode,
            force_update_plan_first,
            input_policy.dynamic_active_tools,
        ))
        .catch_unwind()
        .await;
        let (status, error) = match turn_result {
            Ok(outcome) => outcome,
            Err(panic) => {
                let detail = crate::utils::panic_message(&*panic);
                crate::utils::record_caught_panic("engine-event-loop", &detail);
                (
                    TurnOutcomeStatus::Failed,
                    Some(format!(
                        "The engine hit an internal error and stopped this turn: {detail}. \
                         Your session is intact — send your message again to retry. \
                         A crash report was saved to ~/.codewhale/crashes/."
                    )),
                )
            }
        };

        // token 用量追踪——turn.usage 包含了本轮消耗的 input_tokens 和 output_tokens。累加到 total_usage 后，
        // UI 就能显示"本次会话已花费 $X.XX"。goal 用量用于跟踪 /goal 任务是否超出 token 预算。
        // 把本轮 token 用量累加到会话总用量
        self.session.total_usage.add(&turn.usage);
        // 记录 goal 的用量
        self.record_goal_usage_for_turn(&turn.usage, turn.elapsed());

        // TurnComplete 是整个函数最重要的输出事件。UI 收到它后会：
        // 1. 停止加载动画 
        // 2. 显示 token 用量和费用 
        // 3. 如果失败，显示错误信息 
        // 4. 重新启用输入框
        // 先发送 goal 更新事件
        self.emit_goal_updated().await;
        // 发送 TurnComplete 事件
        let _ = self
            .tx_event
            .send(Event::TurnComplete {
                usage: turn.usage,
                status,
                error,
                tool_catalog: tool_catalog_for_event,
                base_url: base_url_for_event,
            })
            .await;

        // Post-turn snapshot. Fire-and-forget: TurnComplete is already
        // emitted, so the UI is unblocked and the user can type / select /
        // paste immediately (#234). The git work proceeds on the blocking
        // pool without forcing the engine loop to await it.
        // 与事前快照不同，事后快照使用 spawn_blocking_supervised（注意没有 .await）——fire-and-forget。
        // 因为此时 TurnComplete 已经发出，UI 已经解除了阻塞，没必要让用户等待快照完成。
        // 如果快照失败，有 supervised 任务管理器会记录日志。
        if self.config.snapshots_enabled {
            // `snapshot_prompt_post` was cloned from `content` above,
            // before `content` was moved into the session messages.
            let post_workspace = self.session.workspace.clone();
            let post_seq = self.turn_counter;
            let post_cap = self.config.snapshots_max_workspace_bytes;
            crate::utils::spawn_blocking_supervised("post-turn-snapshot", move || {
                post_turn_snapshot(
                    &post_workspace,
                    post_seq,
                    post_cap,
                    Some(&snapshot_prompt_post),
                );
            });
        }

        // ── 跨轮次 Goal 延续 ───────────────────────────────────
        //  如果本轮成功完成，且 goal 仍然是 Active 状态(且未超出预算), 则自动发起下一轮。
        // 这就是 Goal 自动循环机制。当用户通过 /goal "重构整个项目" 设置了一个目标后：
        // 1. 第一轮执行完成
        // 2. 引擎检查 goal 是否还是 Active（用户没暂停）+ 是否在预算内
        // 3. 如果条件满足，自动通过 tx_op 通道发送一个新的 Op::SendMessage
        // 4. 引擎的 op 处理循环收到后，会再次调用 handle_send_message
        // 5. 如此循环直到 AI 自我报告完成、用户暂停、或预算用完
        // 注意几个细节： 
        // - goal_objective: None：不重复传目标，避免无限嵌套
        // - provenance: UserInputProvenance::Runtime：标记为运行时发起，这样 UI 可以区分"用户手动发的"和"自动继续的"
        // - "Failed 或 Interrupted 的 turn 不会继续"——用户按 Esc 中断后，goal 循环就停了
        if status == TurnOutcomeStatus::Completed
            && let Some(continuation) = self.goal_continuation_if_active()
        {
            // 使用与本轮相同的route/mode/approval配置重新派发消息
            // The non-Copy values were moved into
            // `self.config` / `self.session` earlier in this function, so
            // we clone them back out here.
            let _ = self
                .tx_op
                .send(Op::SendMessage {
                    content: continuation,   // 继续提示（如"继续完成目标"）
                    mode,                    // 沿用相同模式
                    provider,
                    model: self.session.model.clone(),
                    goal_objective: None,    // ← 注意：传 None，防止无限嵌套
                    goal_token_budget: None,
                    goal_status: GoalStatus::Active,  // ← 保持 Active
                    reasoning_effort: self.session.reasoning_effort.clone(),
                    reasoning_effort_auto,
                    auto_model,
                    allow_shell,
                    trust_mode,
                    auto_approve,
                    approval_mode,
                    translation_enabled,
                    show_thinking,
                    allowed_tools: self.config.allowed_tools.clone(),
                    dynamic_tools: dynamic_tools.clone(),
                    hook_executor: self.config.hook_executor.clone(),
                    verbosity: self.config.verbosity.clone(),
                    provenance: UserInputProvenance::Runtime,    // ← 标记为"运行时发起"
                })
                .await;
        }
    }

    /// 调用 compact_messages_safe 压缩会话消息
    /// 保留"固定"消息（如重要的用户指令）
    /// 替换会话消息，合并压缩摘要
    /// 发送 CompactionStarted / CompactionCompleted 事件
    async fn handle_manual_compaction(&mut self) {
        let id = format!("compact_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let zero_usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            ..Usage::default()
        };
        let Some(client) = self.deepseek_client.clone() else {
            let message = "Manual compaction unavailable: API client not configured".to_string();
            self.emit_compaction_failed(id, false, message.clone())
                .await;
            let _ = self
                .tx_event
                .send(Event::error(ErrorEnvelope::fatal_auth(message.clone())))
                .await;
            let _ = self
                .tx_event
                .send(Event::TurnComplete {
                    usage: zero_usage,
                    status: TurnOutcomeStatus::Failed,
                    error: Some(message),
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
            return;
        };

        let start_message = "Manual context compaction started".to_string();
        self.emit_compaction_started(id.clone(), false, start_message)
            .await;

        let compaction_pins = self
            .session
            .working_set
            .pinned_message_indices(&self.session.messages, &self.session.workspace);
        let compaction_paths = self.session.working_set.top_paths(24);
        let messages_before = self.session.messages.len();
        let mut turn_status = TurnOutcomeStatus::Completed;
        let mut turn_error = None;

        match compact_messages_safe(
            &client,
            &self.session.messages,
            &self.config.compaction,
            Some(&self.session.workspace),
            Some(&compaction_pins),
            Some(&compaction_paths),
        )
        .await
        {
            Ok(result) => {
                if !result.messages.is_empty() || self.session.messages.is_empty() {
                    let messages_after = result.messages.len();
                    self.session.replace_messages(result.messages);
                    self.merge_compaction_summary(result.summary_prompt);
                    self.emit_session_updated().await;
                    let removed = messages_before.saturating_sub(messages_after);
                    let message = if result.retries_used > 0 {
                        format!(
                            "Compaction complete: {messages_before} → {messages_after} messages ({removed} removed, {} retries)",
                            result.retries_used
                        )
                    } else {
                        format!(
                            "Compaction complete: {messages_before} → {messages_after} messages ({removed} removed)"
                        )
                    };
                    self.emit_compaction_completed(
                        id,
                        false,
                        message,
                        Some(messages_before),
                        Some(messages_after),
                    )
                    .await;
                } else {
                    let message = "Compaction skipped: produced empty result".to_string();
                    self.emit_compaction_failed(id, false, message.clone())
                        .await;
                    turn_status = TurnOutcomeStatus::Failed;
                    turn_error = Some(message);
                }
            }
            Err(err) => {
                let message = format!("Manual context compaction failed: {err}");
                self.emit_compaction_failed(id, false, message.clone())
                    .await;
                let _ = self.tx_event.send(Event::status(message.clone())).await;
                turn_status = TurnOutcomeStatus::Failed;
                turn_error = Some(message);
            }
        }

        let _ = self
            .tx_event
            .send(Event::TurnComplete {
                usage: zero_usage,
                status: turn_status,
                error: turn_error,
                tool_catalog: None,
                base_url: None,
            })
            .await;
    }

    /// 比 compaction 更激进：通过 API 精炼/删除消息
    /// 发送 purge-started / purge-failed / purge-completed 事件
    async fn handle_purge(&mut self) {
        let zero_usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            ..Usage::default()
        };
        let Some(client) = self.deepseek_client.clone() else {
            let message = "Purge unavailable: API client not configured".to_string();
            emit_purge_failed(&self.tx_event, message.clone()).await;
            let _ = self
                .tx_event
                .send(Event::error(ErrorEnvelope::fatal_auth(message.clone())))
                .await;
            let _ = self
                .tx_event
                .send(Event::TurnComplete {
                    usage: zero_usage,
                    status: TurnOutcomeStatus::Failed,
                    error: Some(message),
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
            return;
        };

        emit_purge_started(
            &self.tx_event,
            "Agent context purge in progress\u{2026}".to_string(),
        )
        .await;
        let messages_before = self.session.messages.len();

        let (status, error) = match run_purge(
            &client,
            &self.session.messages,
            &self.session.model,
            self.session.reasoning_effort.clone(),
            effective_max_output_tokens_for_route(&self.session.model, self.active_route_limits),
        )
        .await
        {
            Ok(result) => {
                let messages_after = result.messages.len();
                self.session.replace_messages(result.messages);
                self.emit_session_updated().await;

                let summary = format!(
                    "Purge complete: {messages_before} → {messages_after} messages \
                         ({} removed, {} condensed)",
                    result.removed_count, result.replaced_count,
                );
                emit_purge_completed(
                    &self.tx_event,
                    messages_before,
                    messages_after,
                    result.removed_count,
                    result.replaced_count,
                    summary,
                )
                .await;
                (TurnOutcomeStatus::Completed, None)
            }
            Err(e) => {
                emit_purge_failed(&self.tx_event, e.clone()).await;
                (TurnOutcomeStatus::Failed, Some(e))
            }
        };

        let _ = self
            .tx_event
            .send(Event::TurnComplete {
                usage: zero_usage,
                status,
                error,
                tool_catalog: None,
                base_url: None,
            })
            .await;
    }

    /// Token 估计与消息修剪
    /// 基于 (session.messages_revision, system_prompt_fingerprint) 做缓存
    /// 5个调用点每轮都要查，缓存避免重复计算
    fn estimated_input_tokens(&mut self) -> usize {
        // Memoized on (session.messages_revision, system-prompt fingerprint).
        // The cache invalidates as soon as either input changes; until then
        // repeated calls (capacity checkpoints, /status, context inspector,
        // TUI footer) all hit the cached value.
        self.token_estimate_cache.lookup_or_compute(
            self.session.messages_revision,
            self.session.system_prompt.as_ref(),
            &self.session.messages,
        )
    }

    /// 从最旧的消息开始删除，直到 token ≤ 预算
    /// 返回删除了多少条消息
    fn trim_oldest_messages_to_budget(&mut self, target_input_budget: usize) -> usize {
        let mut removed = 0usize;
        while self.session.messages.len() > MIN_RECENT_MESSAGES_TO_KEEP
            && self.estimated_input_tokens() > target_input_budget
        {
            self.session.messages.trim_front(1);
            self.session.bump_messages_revision();
            removed = removed.saturating_add(1);
        }
        removed
    }

    /// 紧急上下文溢出恢复：
    /// 1. 尝试通过 API 做摘要压缩（带 working-set 固定点）
    /// 2. 如果 API 失败，退回到本地 trim 策略
    /// 3. 发送 compaction 相关事件
    /// 返回是否成功
    async fn recover_context_overflow(&mut self, client: &DeepSeekClient, reason: &str) -> bool {
        let Some(target_budget) = context_input_budget_for_route(
            self.api_provider,
            &self.session.model,
            self.active_route_limits,
            0,
        ) else {
            return false;
        };

        let id = format!("compact_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let start_message = format!("Emergency context compaction started ({reason})");
        self.emit_compaction_started(id.clone(), true, start_message)
            .await;

        let before_tokens = self.estimated_input_tokens();
        let before_count = self.session.messages.len();

        let mut retries_used = 0u32;
        let mut summary_prompt = None;
        let mut compacted_messages: Vec<Message> = self.session.messages.clone().into();

        let mut forced_config = self.config.compaction.clone();
        forced_config.enabled = true;
        forced_config.token_threshold = forced_config
            .token_threshold
            .min(target_budget.saturating_sub(1))
            .max(1);

        // Preserve the working-set pins on the emergency/preflight path too.
        // Previously this passed None/None, so a compaction routed here (which,
        // on large windows, is the path that actually fires) could summarize
        // away pinned errors, patches, and the files the user is editing.
        let compaction_pins = self
            .session
            .working_set
            .pinned_message_indices(&self.session.messages, &self.session.workspace);
        let compaction_paths = self.session.working_set.top_paths(24);

        match compact_messages_safe(
            client,
            &self.session.messages,
            &forced_config,
            Some(&self.session.workspace),
            Some(&compaction_pins),
            Some(&compaction_paths),
        )
        .await
        {
            Ok(result) => {
                retries_used = result.retries_used;
                compacted_messages = result.messages;
                summary_prompt = result.summary_prompt;
            }
            Err(err) => {
                let _ = self
                    .tx_event
                    .send(Event::status(format!(
                        "Emergency compaction API pass failed: {err}. Falling back to local trim."
                    )))
                    .await;
            }
        }

        if !compacted_messages.is_empty() || self.session.messages.is_empty() {
            self.session.replace_messages(compacted_messages);
        }
        self.merge_compaction_summary(summary_prompt);

        let trimmed = self.trim_oldest_messages_to_budget(target_budget);
        self.emit_session_updated().await;
        let after_tokens = self.estimated_input_tokens();
        let after_count = self.session.messages.len();
        let recovered = after_tokens <= target_budget
            && (after_tokens < before_tokens || after_count < before_count || trimmed > 0);

        if recovered {
            let removed = before_count.saturating_sub(after_count);
            let mut details = format!(
                "Emergency compaction complete: {before_count} → {after_count} messages ({removed} removed), ~{before_tokens} → ~{after_tokens} tokens"
            );
            if retries_used > 0 {
                details.push_str(&format!(" ({retries_used} retries)"));
            }
            if trimmed > 0 {
                details.push_str(&format!(", trimmed {trimmed} oldest"));
            }
            self.emit_compaction_completed(
                id,
                true,
                details.clone(),
                Some(before_count),
                Some(after_count),
            )
            .await;
            let _ = self.tx_event.send(Event::status(details)).await;
            return true;
        }

        let message = format!(
            "Emergency context compaction failed to reduce request below model limit \
             (estimate ~{after_tokens} tokens, budget ~{target_budget})."
        );
        self.emit_compaction_failed(id, true, message.clone()).await;
        let _ = self.tx_event.send(Event::status(message)).await;
        false
    }

    /// Role/type model map for sub-agent runtimes: roster member pins first,
    /// then explicit `[subagents]` overrides on top so explicit config wins
    /// (#fleet-roster cutover (v0.8.67)).
    fn subagent_role_models(&self) -> HashMap<String, String> {
        let mut models = self.config.fleet_roster.model_overrides();
        models.extend(
            self.config
                .subagent_model_overrides
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        models
    }

    /// 构建工具上下文，告诉每个工具：当前什么模式、能不能联网、往哪写文件。
    fn build_tool_context(&self, mode: AppMode, auto_approve: bool) -> ToolContext {
        let authority = TurnAuthority::from_effective_fields(
            mode,
            self.session.allow_shell,
            self.session.trust_mode,
            mode == AppMode::Yolo || auto_approve,
            self.session.approval_mode,
        );
        // Load the per-workspace trusted-paths list (#29) on every tool-context
        // build. Cheap (a small JSON file) and always reflects the latest
        // `/trust add` / `/trust remove` mutations without an explicit cache
        // refresh hook.
        let trusted = crate::workspace_trust::WorkspaceTrust::load_for(&self.session.workspace);
        let mut trusted_external_paths = trusted.paths().to_vec();
        let clipboard_images_dir =
            crate::tui::clipboard::clipboard_images_dir(&self.session.workspace);
        if !trusted_external_paths
            .iter()
            .any(|path| path == &clipboard_images_dir)
        {
            trusted_external_paths.push(clipboard_images_dir);
        }
        let mut ctx = ToolContext::with_auto_approve(
            self.session.workspace.clone(),
            authority.trust_mode,
            self.session.notes_path.clone(),
            self.session.mcp_config_path.clone(),
            authority.auto_approve,
        )
        .with_state_namespace(self.session.id.clone())
        .with_features(self.config.features.clone())
        .with_shell_manager(self.shell_manager.clone())
        .with_runtime_services(self.config.runtime_services.clone())
        .with_skills_config(
            self.config.skills_dir.clone(),
            self.config.skills_scan_codewhale_only,
        )
        .with_session_objects(crate::rlm::session::SessionObjectSnapshot::new(
            self.session.id.clone(),
            self.session.model.clone(),
            self.session.workspace.clone(),
            self.session.system_prompt.clone(),
            self.session.messages.clone().into(),
        ))
        .with_cancel_token(self.cancel_token.clone())
        .with_shell_policy(authority.shell_policy())
        .with_trusted_external_paths(trusted_external_paths)
        .with_follow_symlinks(self.config.workspace_follow_symlinks);

        // Hand the user-memory path to tools so the model-callable
        // `remember` tool can append entries (#489). `None` when the
        // feature is disabled — tools short-circuit on that.
        if self.config.memory_enabled {
            ctx.memory_path = Some(self.config.memory_path.clone());
        }

        if let Some(decider) = self.config.network_policy.as_ref() {
            ctx = ctx.with_network_policy(decider.clone());
        }

        // Wire the large-output router (#548). Only attaches when the
        // [workshop] config table is present; sub-agents don't inherit the
        // router (their ToolContext is built separately) to prevent recursive
        // routing of the synthesis call itself.
        if let Some(workshop_cfg) = self.config.workshop.as_ref()
            && let Some(vars_arc) = self.workshop_vars.as_ref()
        {
            let router =
                crate::tools::large_output_router::LargeOutputRouter::new(workshop_cfg.clone());
            ctx = ctx.with_large_output_router(router, vars_arc.clone());
        }

        // Wire the external sandbox backend (#516). exec_shell checks this
        // field and routes commands through the backend instead of spawning
        // a local process when it's set.
        if let Some(backend) = self.sandbox_backend.as_ref() {
            ctx = ctx.with_sandbox_backend(std::sync::Arc::clone(backend));
        }

        // Wire search provider config.
        ctx.search_provider = self.config.search_provider;
        ctx.search_api_key = self.config.search_api_key.clone();
        ctx.search_base_url = self.config.search_base_url.clone();

        let policy = authority.sandbox_policy(&self.session.workspace);
        let mut ctx = ctx.with_elevated_sandbox_policy(policy);
        if matches!(mode, AppMode::Plan) {
            ctx = ctx.with_shell_network_denied_hint(
                "Shell command blocked: Plan mode runs shell commands in a read-only sandbox — no writes, no network. Use Act mode (`/mode act`) for any command that creates or modifies files, or that needs network access.",
            );
        }
        ctx
    }

    /// 延迟初始化 MCP 连接池（第一次使用时才创建）
    /// 从 mcp_config_path 读取配置
    /// 缓存起来避免重复创建
    async fn ensure_mcp_pool(&mut self) -> Result<Arc<AsyncMutex<McpPool>>, ToolError> {
        if let Some(pool) = self.mcp_pool.as_ref() {
            return Ok(Arc::clone(pool));
        }
        let mut pool = McpPool::from_config_path_with_workspace(
            &self.session.mcp_config_path,
            &self.session.workspace,
        )
        .unwrap_or_else(|e| {
            tracing::debug!("No MCP config: {e}");
            McpPool::new(McpConfig::default())
        });
        if let Some(decider) = self.config.network_policy.as_ref() {
            pool = pool.with_network_policy(decider.clone());
        }
        let pool = Arc::new(AsyncMutex::new(pool));
        self.mcp_pool = Some(Arc::clone(&pool));
        Ok(pool)
    }

    async fn mcp_tools(&mut self) -> Vec<Tool> {
        let pool = match self.ensure_mcp_pool().await {
            Ok(pool) => pool,
            Err(err) => {
                let _ = self.tx_event.send(Event::status(format!("{err:#}"))).await;
                return Vec::new();
            }
        };

        let mut pool = pool.lock().await;
        let errors = pool.connect_all().await;
        for (server, err) in errors {
            let _ = self
                .tx_event
                .send(Event::status(format!(
                    "Failed to connect MCP server '{server}': {err:#}"
                )))
                .await;
        }

        pool.to_api_tools()
    }

    /// Handle a turn using the DeepSeek API.
    #[allow(clippy::too_many_lines)]
    /// 运行请求前的分层上下文检查点（#159）。检查当前活跃输入的估算值
    /// 是否已超过软分界阈值，如果超过，则通过 Flash 生成一个 `<archived_context>`
    /// 块，并将其作为助手消息追加到上下文中。该函数在每次 API 请求前从
    /// `handle_deepseek_turn` 中调用，以确保模型始终拥有最新的导航辅助信息。
    /// 分层上下文检查点（Seam 机制）：
    /// - L1：token < 阈值 → 什么都不做
    /// - L2：产生 archived_context 摘要（通过 Flash 轻量模型）
    /// - L3：重新压缩已有的 archived_context
    /// 保留最近 N 轮的"逐字窗口"，只压缩旧内容
    async fn layered_context_checkpoint(&mut self) {
        if self.seam_manager.is_none() {
            return;
        }
        if !self.seam_manager.as_ref().unwrap().config().enabled {
            return;
        }

        // Compute the estimated token count *before* taking a long-lived
        // `&SeamManager` borrow — `estimated_input_tokens` mutates the
        // engine's token-estimate cache, which would conflict.
        let estimated_tokens = self.estimated_input_tokens();
        let seam_mgr = self.seam_manager.as_ref().unwrap();
        let highest = seam_mgr.highest_level().await;
        let Some(level) = seam_mgr.seam_level_for(estimated_tokens, highest) else {
            return;
        };

        // Determine the message range to summarize: everything before the
        // verbatim window. The verbatim window (last ~16 turns) stays
        // untouched so the model always has ground-truth recent context.
        let msg_count = self.session.messages.len();
        let verbatim_start = seam_mgr.verbatim_window_start(msg_count);
        if verbatim_start == 0 {
            return; // Not enough messages to summarize.
        }

        let msg_range_end = verbatim_start;
        let pinned = self
            .session
            .working_set
            .pinned_message_indices(&self.session.messages, &self.session.workspace);

        let _ = self
            .tx_event
            .send(Event::status(format!(
                "⏻ producing L{level} context seam ({msg_range_end} messages)…"
            )))
            .await;

        // If we have existing seams, recompact; otherwise produce fresh.
        let existing_seams = seam_mgr.collect_seam_texts(&self.session.messages).await;
        let seam_text = if existing_seams.is_empty() {
            match seam_mgr
                .produce_soft_seam(
                    &self.session.messages,
                    level,
                    0,
                    msg_range_end,
                    Some(&self.session.workspace),
                    &pinned,
                )
                .await
            {
                Ok(text) => text,
                Err(err) => {
                    crate::logging::warn(format!("L{level} soft seam failed: {err}"));
                    return;
                }
            }
        } else {
            let recent: Vec<&Message> = (0..msg_range_end)
                .filter_map(|i| self.session.messages.get(i))
                .collect();
            match seam_mgr
                .recompact(&existing_seams, &recent, level, 0, msg_range_end)
                .await
            {
                Ok(text) => text,
                Err(err) => {
                    crate::logging::warn(format!("L{level} recompact failed: {err}"));
                    return;
                }
            }
        };

        if seam_text.is_empty() {
            return;
        }

        // Capture seam count before the mutable borrow below.
        let seam_count = seam_mgr.seam_count().await;

        // Append the seam as an assistant message. This is an append-only
        // operation — no messages are deleted. The prefix cache stays hot.
        self.add_session_message(Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: seam_text,
                cache_control: None,
            }],
        })
        .await;

        let _ = self
            .tx_event
            .send(Event::status(format!(
                "⏻ L{level} seam complete ({seam_count} total, {msg_range_end} messages covered)"
            )))
            .await;
    }

    /// 基于当前的非模式上下文刷新稳定的系统提示词。
    /// 非模式上下文（non-mode context）：指不依赖当前交互模式（比如对话模式、编辑模式、调试模式等）的
    /// 全局状态信息，例如工作区路径、当前打开的文件、用户偏好设置等。
    /// 重建系统提示词，包含：
    /// - 用户记忆
    /// - 目标信息
    /// - 模式说明
    /// - 技能上下文
    /// - SlopLedger 门控块
    /// - 压缩摘要
    /// 如果有 system_prompt_override 就跳过
    /// 返回值： 不返回值，只更新Engine::session.system_promp
    fn refresh_system_prompt(&mut self) {
        // 为系统提示词组合 <user_memory> 块
        let user_memory_block = crate::memory::compose_block(
            self.config.memory_enabled && !self.config.moraine_fallback, // TODO(v0.8.71): remove when Moraine recall stable; see #3490, #3495
            &self.config.memory_path,
        );
        // 当前目标是什么
        let prompt_goal_objective = goal_objective_for_prompt(
            self.config.goal_objective.as_deref(),
            &self.config.goal_state,
        );
        // 组装基础 system prompt,包括：
        // 宪法文本,模式指令（Agent/Plan/Yolo),Locale,项目上下文(AGENTS.md等)，用户宪章
        // Project Context Pack(工作区的目录结构等)，技能上下文（skills_dir下的md）
        // 指令源（用户自定义的额外指令），用户记忆（user_memory_block），目标（goal_objective）
        // MCP工具(MCP服务器注册的工具列表)
        let base = prompts::system_prompt_for_mode_with_context_skills_session_and_approval(
            &self.config.workspace,
            None,
            Some(&self.config.skills_dir),
            Some(&self.config.instructions),
            prompts::PromptSessionContext {
                user_memory_block: user_memory_block.as_deref(),
                goal_objective: prompt_goal_objective.as_deref(),
                project_context_pack_enabled: self.config.project_context_pack_enabled,
                locale_tag: &self.config.locale_tag,
                translation_enabled: self.config.translation_enabled,
                model_id: &self.config.model,
                context_window_override: Some(crate::route_budget::route_context_window_tokens(
                    self.api_provider,
                    &self.config.model,
                    self.active_route_limits,
                )),
                show_thinking: self.config.show_thinking,
                verbosity: self.config.verbosity.as_deref(),
                skills_scan_codewhale_only: self.config.skills_scan_codewhale_only,
            },
        );
        // 合并压缩摘要,将基础 prompt 和压缩摘要合并.
        let mut stable_prompt =
            merge_system_prompts(Some(&base), self.session.compaction_summary_prompt.clone());

        // SlopLedger completion-gate: inject unresolved slop entries into the
        // system prompt so the agent can autonomously review them before
        // claiming the task is done (#2127).
        // 检查 SlopLedger（"草率日志"用于追踪AI承诺要做但未完成的事项的简单本地文件）是否有未解决的条目。
        // 返回 Option<String>。
        // e.g 
        // ``` XML
        //   <slop_ledger>
        //   注意：以下事项尚未完成，在声明任务完成前请自行审查并处理：
        //    - [ ] 检查 src/lib.rs 中的类型错误
        //    - [ ] 更新文档
        //   </slop_ledger>
        // ```
        // 设计意图：SlopLedger 是 #2127 中引入的机制——AI在编码过程中常常"承诺会做某事"但忘记执行。
        // SlopLedger 文件记录这些未兑现的承诺。slop_ledger_gate_block 读取该文件，如果非空则追加到
        // system prompt 末尾，作为实时提醒。这样LLM在声称任务完成前会先看到"你还有这些事没做完"，促使
        // 其自主审查并处理。
        let gate_block = self.slop_ledger_gate_block();
        if let Some(ref block) = gate_block
            && let Some(SystemPrompt::Text(prompt_text)) = &mut stable_prompt
        {
            prompt_text.push_str("\n\n");
            prompt_text.push_str(block);
        }

        let stable_hash = system_prompt_hash(stable_prompt.as_ref());
        if self.session.system_prompt_override {
            // 如果 system_prompt_override 被设置为 true, 意味着用户/外部系统手动设置了 system prompt
            return;
        }
        if self.session.last_system_prompt_hash != Some(stable_hash) {
            self.session.system_prompt = stable_prompt;
            self.session.last_system_prompt_hash = Some(stable_hash);
        }
    }

    fn slop_ledger_gate_block(&mut self) -> Option<String> {
        let modified = crate::slop_ledger::SlopLedger::default_path()
            .ok()
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok());

        if let Some((cached_modified, cached_block)) = &self.slop_ledger_gate_cache
            && *cached_modified == modified
        {
            return cached_block.clone();
        }

        let loaded = crate::slop_ledger::SlopLedger::load()
            .ok()
            .and_then(|ledger| {
                if ledger.has_open_entries() {
                    ledger.completion_gate_summary()
                } else {
                    None
                }
            });
        self.slop_ledger_gate_cache = Some((modified, loaded.clone()));
        loaded
    }

    /// Merge a compaction summary into the system prompt.
    ///
    /// **Zone affiliation (#2264)**: this mutates the system prompt, which is
    /// part of the `PinnedPrefix` zone in the three-zone contract. Compaction
    /// is the one intentional mid-session prefix mutation — the engine
    /// intentionally accepts the cache-invalidation cost because the
    /// context-reduction benefit outweighs it.
    fn merge_compaction_summary(&mut self, summary_prompt: Option<SystemPrompt>) {
        if summary_prompt.is_none() {
            return;
        }
        self.session.compaction_summary_prompt = merge_system_prompts(
            self.session.compaction_summary_prompt.as_ref(),
            summary_prompt.clone(),
        );
        let merged = merge_system_prompts(self.session.system_prompt.as_ref(), summary_prompt);
        self.session.last_system_prompt_hash = Some(system_prompt_hash(merged.as_ref()));
        self.session.system_prompt = merged;
    }
}

// 插件工具
fn default_plugin_tools_dir() -> PathBuf {
    // ~/.codewhale/tools
    codewhale_config::codewhale_home()
        .unwrap_or_else(|_| {
            dirs::home_dir().map_or_else(|| PathBuf::from(".codewhale"), |h| h.join(".codewhale"))
        })
        .join("tools")
}

fn plugin_tools_dir(tools_config: Option<&crate::config::ToolsConfig>) -> PathBuf {
    if let Some(tools_config) = tools_config
        && let Some(custom_dir) = tools_config.plugin_dir.as_deref()
    {
        return PathBuf::from(shellexpand::tilde(custom_dir).as_ref());
    }
    default_plugin_tools_dir()
}

/// 加载插件工具目录下的所有工具
/// 应用 tool overrides（禁用/替换原生工具）
/// 返回新加载的工具名集合
fn configure_plugin_tools(
    tool_registry: &mut crate::tools::ToolRegistry,
    tools_config: Option<&crate::config::ToolsConfig>,
) -> std::collections::HashSet<String> {
    let names_before: std::collections::HashSet<String> = tool_registry
        .names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    let plugin_dir = plugin_tools_dir(tools_config);
    tool_registry.load_plugins(&plugin_dir);

    if let Some(tools_config) = tools_config
        && let Some(ref overrides) = tools_config.overrides
    {
        tool_registry.apply_overrides(overrides, &plugin_dir);
    }

    let names_after: std::collections::HashSet<String> = tool_registry
        .names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    &names_after - &names_before
}

fn system_prompt_hash(prompt: Option<&SystemPrompt>) -> u64 {
    let mut hasher = DefaultHasher::new();
    match prompt {
        Some(SystemPrompt::Text(text)) => {
            0u8.hash(&mut hasher);
            text.hash(&mut hasher);
        }
        Some(SystemPrompt::Blocks(blocks)) => {
            1u8.hash(&mut hasher);
            for block in blocks {
                block.block_type.hash(&mut hasher);
                block.text.hash(&mut hasher);
                if let Some(cache_control) = &block.cache_control {
                    cache_control.cache_type.hash(&mut hasher);
                }
            }
        }
        None => {
            2u8.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// 去掉空白，过滤空字符串
fn normalized_goal_objective(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// 把主进程的目标信息同步到共享状态
/// # Arguments
/// * `goal_state` 共享的目标状态（多线程安全）
/// * `objective` 目标描述文本
/// * `token_budget` token 预算上限
/// * `status` 目标状态：Active / Paused / Completed / Blocked 
fn sync_goal_state_from_host(
    goal_state: &SharedGoalState,
    objective: Option<&str>,
    token_budget: Option<u32>,
    status: GoalStatus,
) {
    match goal_state.lock() {
        Ok(mut state) => state.sync_from_host_status(objective, token_budget, status),
        Err(err) => tracing::warn!("goal state lock poisoned while syncing host goal: {err}"),
    }
}

// 获取活跃目标的信息用于注入提示词
// 如goal_state有值则返回GoalState::Objective，否则返回configured_goal;
fn goal_objective_for_prompt(
    configured_goal: Option<&str>,
    goal_state: &SharedGoalState,
) -> Option<String> {
    match goal_state.lock() {
        Ok(state) => {
            if let Some(objective) = state.objective() {
                // Preserve original behavior: return None (not fallback) when
                // objective exists but goal is inactive.
                return state.is_active().then(|| objective.to_string());
            }
        }
        Err(err) => tracing::warn!("goal state lock poisoned while building prompt: {err}"),
    }
    normalized_goal_objective(configured_goal)
}

// ── 模式与审批提示词作为请求时的运行时元数据 ────────────────────────
//
// 模式契约（mode contracts）和审批策略（approval policies）不会持久化到
// 会话历史中，也不会作为额外的系统消息发送。取而代之的是，每次 API 请求
// 都会在消息尾部投射一条临时的、属于 user 角色的运行时元数据消息。
// 这样一来，稳定的系统提示词在字节层面上保持稳定，存储的历史记录在字节
// 层面上也保持稳定，并且严格要求 chat-template 的服务端永远不会看到
// messages[0] 之外的系统消息。
// 简而言之：把会变化的东西放在末尾，把不变的东西放在开头，既保缓存，又保兼容。

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolAskRuleDecision {
    /// 弹窗询问用户
    Prompt(String),
    /// 直接拒绝
    Block(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AutoReviewPlanDecision {
    NoChange,
    ForcePrompt(String),
    Block(String),
}

pub(super) fn auto_review_run_origin_for_plan(
    detached_start: bool,
) -> crate::tui::auto_review::RunOrigin {
    if detached_start {
        crate::tui::auto_review::RunOrigin::Background
    } else {
        crate::tui::auto_review::RunOrigin::Interactive
    }
}

// The parameter list intentionally mirrors `AutoReviewContext::from_tool_call`,
// which this thin wrapper builds; the 8 call sites (1 prod + tests) read clearer
// passing the fields than constructing a context first.
#[allow(clippy::too_many_arguments)]
pub(super) fn auto_review_plan_decision(
    policy: &crate::tui::auto_review::AutoReviewPolicy,
    tool_name: &str,
    tool_input: &Value,
    run_origin: crate::tui::auto_review::RunOrigin,
    approval_mode: crate::tui::approval::ApprovalMode,
    user_intent: Option<&str>,
    workspace_trusted: bool,
    dirty_worktree: bool,
) -> (AutoReviewPlanDecision, Value) {
    let context = crate::tui::auto_review::AutoReviewContext::from_tool_call(
        tool_name,
        tool_input,
        run_origin,
        approval_mode,
        user_intent,
        workspace_trusted,
        dirty_worktree,
    );
    let decision = policy.evaluate(&context);
    let audit_event = policy.audit_event(&context, &decision);
    let plan_decision = match decision.action {
        crate::tui::auto_review::AutoReviewAction::Allow
        | crate::tui::auto_review::AutoReviewAction::AskUser => AutoReviewPlanDecision::NoChange,
        crate::tui::auto_review::AutoReviewAction::HoldForReview => {
            // HoldForReview only originates from the built-in safety floor
            // (configured rules produce Allow/Block), so name the gate
            // honestly instead of blaming an "auto-review policy" the user
            // may never have configured (#3883).
            let reason = format!(
                "Built-in safety gate requires approval: {}",
                decision.reason
            );
            if matches!(approval_mode, crate::tui::approval::ApprovalMode::Never) {
                AutoReviewPlanDecision::Block(reason)
            } else {
                AutoReviewPlanDecision::ForcePrompt(reason)
            }
        }
        crate::tui::auto_review::AutoReviewAction::Block => AutoReviewPlanDecision::Block(format!(
            "Auto-review policy blocked tool '{tool_name}': {}",
            decision.reason
        )),
    };
    (plan_decision, audit_event)
}

/// 这些是安全策略的核心实现。permissions.toml 里配置的规则最终在这里生效。
/// shell 命令的 exec_policy 检查
/// 比如：command 在白名单 → None（不拦截）
///       command 在黑名单 → Block
///       command 需要审批 → Prompt
pub(super) fn exec_shell_ask_rule_decision(
    config: &EngineConfig,
    tool_name: &str,
    tool_input: &Value,
    workspace: &Path,
    approval_mode: crate::tui::approval::ApprovalMode,
) -> Option<ToolAskRuleDecision> {
    if tool_name != "exec_shell" {
        return None;
    }
    let command = tool_input.get("command").and_then(Value::as_str)?;
    tool_ask_rule_decision_for_context(config, tool_name, command, None, workspace, approval_mode)
}

/// 文件工具（read/write/edit/search）的路径权限检查
/// 遍历 tool_input 中涉及的每个文件路径
pub(super) fn file_tool_ask_rule_decision(
    config: &EngineConfig,
    tool_name: &str,
    tool_input: &Value,
    workspace: &Path,
    approval_mode: crate::tui::approval::ApprovalMode,
) -> Option<ToolAskRuleDecision> {
    let paths = file_tool_permission_paths(tool_name, tool_input)?;
    if paths.is_empty() {
        return tool_ask_rule_decision_for_context(
            config,
            tool_name,
            "",
            None,
            workspace,
            approval_mode,
        );
    }

    let mut prompt: Option<String> = None;
    for path in paths {
        match tool_ask_rule_decision_for_context(
            config,
            tool_name,
            "",
            Some(&path),
            workspace,
            approval_mode,
        ) {
            Some(ToolAskRuleDecision::Block(reason)) => {
                return Some(ToolAskRuleDecision::Block(reason));
            }
            Some(ToolAskRuleDecision::Prompt(reason)) => {
                prompt.get_or_insert(reason);
            }
            None => {}
        }
    }
    prompt.map(ToolAskRuleDecision::Prompt)
}

/// 核心：对一条命令/路径执行策略引擎匹配
/// 先查 deny 规则 → 再查 allow 规则 → 再看 ask 规则
fn tool_ask_rule_decision_for_context(
    config: &EngineConfig,
    tool_name: &str,
    command: &str,
    path: Option<&str>,
    workspace: &Path,
    approval_mode: crate::tui::approval::ApprovalMode,
) -> Option<ToolAskRuleDecision> {
    let cwd = workspace.to_string_lossy();
    let ask_for_approval = match approval_mode {
        crate::tui::approval::ApprovalMode::Never => AskForApproval::Never,
        crate::tui::approval::ApprovalMode::Auto
        | crate::tui::approval::ApprovalMode::Bypass
        | crate::tui::approval::ApprovalMode::Suggest => AskForApproval::OnFailure,
    };
    let decision = config
        .exec_policy_engine
        .check(ExecPolicyContext {
            command,
            cwd: cwd.as_ref(),
            tool: Some(tool_name),
            path,
            ask_for_approval,
            sandbox_mode: None,
        })
        .ok()?;
    if !decision.allow {
        Some(ToolAskRuleDecision::Block(decision.reason().to_string()))
    } else if decision.requires_approval {
        Some(ToolAskRuleDecision::Prompt(decision.reason().to_string()))
    } else {
        None
    }
}

fn file_tool_permission_paths(tool_name: &str, input: &Value) -> Option<Vec<String>> {
    match tool_name {
        "read_file" | "write_file" | "edit_file" | "file_search" | "grep_files" => {
            Some(string_field(input, "path").into_iter().collect())
        }
        "list_dir" => Some(vec![
            string_field(input, "path").unwrap_or_else(|| ".".to_string()),
        ]),
        "apply_patch" => Some(apply_patch_permission_paths(input)),
        _ => None,
    }
}

fn string_field(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn apply_patch_permission_paths(input: &Value) -> Vec<String> {
    crate::tools::apply_patch::preflight_apply_patch(input)
        .map(|preflight| preflight.touched_files)
        .unwrap_or_default()
}

/// 这是外部调用引擎的唯一入口。 创建引擎后立刻 spawn 到后台，返回 EngineHandle 给调用者（通常是 TUI 主线程）。
/// 创建一个新的异步任务，在其中运行引擎实例，让引擎在后台持续工作（比如处理消息循环、监听事件等），
/// 而当前线程/任务可以继续做其他事情。这是 Rust/异步运行时（如 tokio）中常见的并发模式。
pub fn spawn_engine(config: EngineConfig, api_config: &Config) -> EngineHandle {
    let (engine, handle) = Engine::new(config, api_config);
    // spawn_supervised：在受监管的 tokio task 中运行引擎
    spawn_supervised(
        "engine-event-loop",
        std::panic::Location::caller(),
        async move {
            engine.run().await;
        },
    );

    handle  // 只返回句柄，引擎在后台运行
}

#[cfg(test)]
pub(crate) struct MockEngineHandle {
    pub handle: EngineHandle,
    pub rx_op: mpsc::Receiver<Op>,
    rx_approval: mpsc::Receiver<ApprovalDecision>,
    pub rx_steer: mpsc::Receiver<String>,
    pub tx_event: mpsc::Sender<Event>,
    pub cancel_token: CancellationToken,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MockApprovalEvent {
    Approved {
        id: String,
    },
    Denied {
        id: String,
    },
    RetryWithPolicy {
        id: String,
        policy: crate::sandbox::SandboxPolicy,
    },
}

#[cfg(test)]
impl MockEngineHandle {
    pub(crate) async fn recv_approval_event(&mut self) -> Option<MockApprovalEvent> {
        match self.rx_approval.recv().await? {
            ApprovalDecision::Approved { id } => Some(MockApprovalEvent::Approved { id }),
            ApprovalDecision::Denied { id } => Some(MockApprovalEvent::Denied { id }),
            ApprovalDecision::RetryWithPolicy { id, policy } => {
                Some(MockApprovalEvent::RetryWithPolicy { id, policy })
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn mock_engine_handle() -> MockEngineHandle {
    let (tx_op, rx_op) = mpsc::channel(32);
    let (tx_event, rx_event) = mpsc::channel(256);
    let (tx_approval, rx_approval) = mpsc::channel(64);
    let (tx_user_input, _rx_user_input) = mpsc::channel(32);
    let (tx_steer, rx_steer) = mpsc::channel(64);
    let cancel_token = CancellationToken::new();
    let shared_cancel_token = Arc::new(StdMutex::new(cancel_token.clone()));
    let cancel_reason: Arc<StdMutex<Option<CancelReason>>> = Arc::new(StdMutex::new(None));
    let shared_paused = Arc::new(StdMutex::new(false));
    let handle = EngineHandle {
        tx_op,
        rx_event: Arc::new(RwLock::new(rx_event)),
        cancel_token: shared_cancel_token,
        cancel_reason,
        tx_approval,
        tx_user_input,
        tx_steer,
        shared_paused,
    };

    MockEngineHandle {
        handle,
        rx_op,
        rx_approval,
        rx_steer,
        tx_event,
        cancel_token,
    }
}

mod approval;
mod context;
mod handle;
pub(crate) use context::compact_tool_result_for_context;
/// Public so external hosts/wrappers can reuse the engine's input-budget math
/// (see `context_input_budget_for_route`'s doc) instead of re-deriving it.
pub use context::context_input_budget_for_route;
#[cfg(test)]
use context::route_context_budget_for_provider;
use context::{
    MAX_CONTEXT_RECOVERY_ATTEMPTS, MIN_RECENT_MESSAGES_TO_KEEP,
    effective_max_output_tokens_for_route, estimate_input_tokens_conservative,
    extract_compaction_summary_prompt, is_context_length_error_message,
    route_context_budget_for_route, summarize_text,
};
#[cfg(test)]
use context::{context_input_budget_for_provider, effective_max_output_tokens};
mod dispatch;
mod lsp_hooks;
mod streaming;
mod token_estimate_cache;
mod tool_catalog;
mod tool_execution;
mod tool_setup;
mod turn_loop;
pub(crate) use token_estimate_cache::TokenEstimateCache;

pub(super) const MAX_PARALLEL_SHELL_EXEC: usize = 4;

pub(crate) fn default_active_native_tool_names() -> &'static [&'static str] {
    tool_catalog::DEFAULT_ACTIVE_NATIVE_TOOLS
}

/// Drop catalog entries the execution gates would reject (#3027): the model
/// should never be advertised a tool it cannot call. Deny wins over allow.
fn filter_tool_catalog_for_gates(
    catalog: &mut Vec<Tool>,
    allowed_tools: Option<&[String]>,
    disallowed_tools: Option<&[String]>,
) {
    catalog.retain(|tool| {
        !turn_loop::command_denies_tool(disallowed_tools, &tool.name)
            && turn_loop::command_allows_tool(allowed_tools, &tool.name)
    });
}

use self::approval::{ApprovalDecision, ApprovalResult, UserInputDecision};
#[cfg(test)]
use self::dispatch::should_parallelize_tool_batch;
use self::dispatch::{
    ParallelToolResult, ParallelToolResultEntry, ToolApprovalStamp, ToolExecGuard, ToolExecOutcome,
    ToolExecutionBatch, ToolExecutionPlan, caller_allowed_for_tool, caller_type_for_tool_use,
    final_tool_input, format_tool_error, malformed_tool_arguments_error,
    malformed_tool_arguments_input, mcp_tool_approval_description, mcp_tool_is_parallel_safe,
    mcp_tool_is_read_only, parse_parallel_tool_calls, parse_tool_input,
    plan_tool_execution_batches, should_force_update_plan_first, should_stop_after_plan_tool,
    stamp_tool_result_approval,
};
#[cfg(test)]
use self::lsp_hooks::edited_paths_for_tool;
#[cfg(test)]
use self::streaming::TOOL_CALL_START_MARKERS;
#[cfg(test)]
use self::streaming::filter_tool_call_delta;
use self::streaming::{
    ContentBlockKind, FAKE_WRAPPER_NOTICE, MAX_STREAM_ERRORS_BEFORE_FAIL, MAX_STREAM_RETRIES,
    MAX_TRANSPARENT_STREAM_RETRIES, STREAM_MAX_CONTENT_BYTES, STREAM_MAX_DURATION_SECS,
    ToolCallDeltaFilterState, ToolUseState, contains_fake_tool_wrapper,
    filter_tool_call_delta_with_state, flush_tool_call_delta_state, should_resume_after_sleep,
    should_transparently_retry_stream, sleep_gap_detected, stream_read_error_user_message,
};
use self::tool_catalog::{
    CODE_EXECUTION_TOOL_NAME, JS_EXECUTION_TOOL_NAME, MULTI_TOOL_PARALLEL_NAME,
    REQUEST_USER_INPUT_NAME, active_tools_for_step, build_model_tool_catalog_with_surface,
    ensure_advanced_tooling, execute_code_execution_tool, execute_tool_search,
    initial_active_tools, is_tool_search_tool, maybe_hydrate_requested_deferred_tool,
    missing_tool_error_message, tool_catalog_consistency_issues,
};
#[cfg(test)]
use self::tool_catalog::{
    TOOL_SEARCH_NAME, build_model_tool_catalog, maybe_activate_requested_deferred_tool,
    preflight_requested_deferred_tool, should_default_defer_tool,
};
use self::tool_execution::emit_tool_audit;
use crate::tools::js_execution::execute_js_execution_tool;

#[cfg(test)]
mod tests;
