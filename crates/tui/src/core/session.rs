//! 核心引擎的会话状态管理。
//!
//! 跟踪对话历史、Token 用量统计和会话元数据。
//! 这是一个会话状态管理模块，负责跟踪对话历史、Token 用量统计和会话元数据。
//! 它属于CodeWhale TUI引擎的"核心层"——引擎每次与模型对话时，都依赖这个Session来维护对话上下文。

use crate::models::{Message, SystemPrompt, Usage};  // 一条对话消息（用户消息、助手消息、工具结果等）;系统提示词（发给模型的开场指令）;单次 API 调用返回的 Token 用量;
use crate::prefix_cache::PrefixStabilityManager;  // 前缀缓存稳定性监视器（检测提示词前缀是否变化）
use crate::project_context::{ProjectContext, load_project_context_with_parents};  // 项目上下文（从 AGENTS.md 等文件加载）;加载项目上下文并向上递归到父目录
use crate::prompt_zones::{AppendLog, FrozenPrefix};  // 追加日志——高效的只追加消息列表;冻结前缀——会话首轮固化下来的不可变前缀基线
use crate::tui::approval::ApprovalMode;  // 审批模式枚举（比如 "Suggest" 建议模式、"AutoApprove" 自动批准）
use crate::working_set::WorkingSet;  // 工作集——跟踪当前活跃的文件和路径
use std::path::PathBuf;

/// 引擎的会话状态。
#[derive(Debug, Clone)]
pub struct Session {
    /// 当前使用的模型名称（如 "deepseek-v4-pro"）。
    pub model: String,

    /// Reasoning-effort tier for DeepSeek thinking mode:
    /// `"off" | "low" | "medium" | "high" | "max"`. `None` lets the provider
    /// apply its own defaults.
    /// 控制 DeepSeek 思考模式的深度。None（空值）让供应商使用默认值。
    pub reasoning_effort: Option<String>,
    /// 标记用户是否选择了"自动推理强度"。
    pub reasoning_effort_auto: bool,

    /// 用户是否选择了自动模型路由。
    pub auto_model: bool,

    /// 工作区目录的路径
    pub workspace: PathBuf,

    /// 可选的系统提示词。None 表示还没有设置。
    pub system_prompt: Option<SystemPrompt>,
    /// 为 true 时，表示当前 system_prompt 是持久化/运行时提供的，不应被模式/上下文刷新替换。
    pub system_prompt_override: bool,
    /// 上次组装的稳定系统提示词的哈希值（64 位无符号整数）。
    /// 用于检测"系统提示词有没有变"——变了才需要替换，没变就不用动（性能优化）。
    pub last_system_prompt_hash: Option<u64>,
    /// 上下文压缩后生成的摘要块（/compact 命令的产物）。
    pub compaction_summary_prompt: Option<SystemPrompt>,

    /// 对话历史 (API format), backed by AppendLog（一个高效的、只追加（append-only）的消息列表。） (#2264).
    pub messages: AppendLog,

    /// 整个会话的累计 Token 用量，类型是我们自定义的 SessionUsage
    pub total_usage: SessionUsage,

    /// 是否允许执行 Shell 命令。
    pub allow_shell: bool,

    /// 是否信任工作区外的路径（安全边界）。
    pub trust_mode: bool,

    /// 是否自动批准工具安全检查。
    pub auto_approve: bool,

    /// 实时的审批策略，类型是枚举（不是 bool，因为审批有多种模式，如"建议"、"自动批准"、"手动审批"），用来指导系统提示词的生成。
    pub approval_mode: ApprovalMode,

    /// 笔记文件路径。
    pub notes_path: PathBuf,

    /// MCP（Model Context Protocol）配置文件路径。
    pub mcp_config_path: PathBuf,

    /// 会话 ID，用 UUID v4 生成，用于追踪和持久化。
    pub id: String,

    /// 可选的项目上下文，从 AGENTS.md、CLAUDE.md 等文件中加载。
    /// 如果加载到了内容，就是 Some(...)，否则 None。
    pub project_context: Option<ProjectContext>,

    /// 仓库感知的工作集，用于上下文管理。跟踪当前激活的文件和路径。
    pub working_set: WorkingSet,

    /// 前缀缓存稳定性监视器（受 Reasonix 的 Pillar 1 启发）。
    /// 跟踪不可变前缀的指纹，检测跨轮次的偏移。
    /// 在引擎构造时设置；第一次组装系统提示词之前为 None。
    /// 前缀缓存稳定性监视器。跟踪不可变前缀的"指纹"，在每一轮间检测是否发生了偏移。
    /// 在引擎构造时设置；第一次组装系统提示词之前为 None。
    pub prefix_stability: Option<PrefixStabilityManager>,

    /// 三区不可变前缀基线 (#2264)。在会话的第一次请求时冻结；
    /// 后续每次请求前，都要拿当前系统+工具状态与之比对验证。第一次轮次之前为 None。
    /// 三区不可变前缀基线。在会话的第一次请求时冻结；后续每次请求前，都要拿当前系统+工具状态与之比对验证。
    /// 为什么冻结？因为 DeepSeek 的 KV 缓存依赖于字节级稳定的前缀——前缀一旦冻结，后续轮次可以复用缓存，
    /// 大幅降低成本（缓存命中比未命中便宜约 100 倍）。
    pub frozen_prefix: Option<FrozenPrefix>,

    /// 每次直接修改 `messages` 时递增的单调计数器。
    /// 被 [`crate::core::engine::token_estimate_cache::TokenEstimateCache`]
    /// 用于记忆化每轮 Token 估算，而无需重新遍历消息列表。
    /// 默认为 0；在 [`Session::add_message`]、
    /// [`Session::replace_messages`] 以及 `core/engine.rs` 中的其他修改点递增。
    /// 单调递增计数器。每次直接修改 messages 时 +1。
    /// 被 Token 估算缓存消费——缓存通过比对版本号来判断是否需要重新计算，而不是每次都遍历消息列表。
    pub messages_revision: u64,
}

/// 会话的累计用量统计。
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_field_names)]
pub struct SessionUsage {
    pub input_tokens: u64,    // 累计输入 Token
    pub output_tokens: u64,   // 累计输出 Token
    /// 缓存未命中（miss）的 Token 数。None 表示 API 从未报告过这个值——不能显示为 0，
    /// 因为 0 可能被误解为"没有未命中"
    pub cache_creation_input_tokens: Option<u64>,
    /// 缓存命中（hit）的 Token 数。同理 None 表示从未观察到
    pub cache_read_input_tokens: Option<u64>,
}

impl SessionUsage {
    /// 添加一轮次的用量
    pub fn add(&mut self, usage: &Usage) {
        self.input_tokens += u64::from(usage.input_tokens);
        self.output_tokens += u64::from(usage.output_tokens);
        // 如果API报告了缓存未命中Token，就把它们加到累计值上；如果是第一次累加（之前是 None），从 0 开始。
        if let Some(tokens) = usage.prompt_cache_miss_tokens {
            self.cache_creation_input_tokens =
                Some(self.cache_creation_input_tokens.unwrap_or(0) + u64::from(tokens));
        }
        if let Some(tokens) = usage.prompt_cache_hit_tokens {
            self.cache_read_input_tokens =
                Some(self.cache_read_input_tokens.unwrap_or(0) + u64::from(tokens));
        }
    }
}

impl Session {
    /// 创建一个新会话
    /// `model` 模型名
    /// `workspace` 工作区路径
    /// `allow_shell` 是否允许 Shell
    /// `trust_mode` 是否信任模式
    /// `notes_path` 笔记路径
    /// `mcp_config_path` MCP 配置路径
    pub fn new(
        model: String,
        workspace: PathBuf,
        allow_shell: bool,
        trust_mode: bool,
        notes_path: PathBuf,
        mcp_config_path: PathBuf,
    ) -> Self {
        // 从 AGENTS.md、CLAUDE.md 等文件加载项目上下文。
        // 从工作区目录开始，向上递归查找 AGENTS.md、CLAUDE.md 等文件，加载项目上下文。
        let project_context = load_project_context_with_parents(&workspace);
        let has_context = project_context.has_instructions();  // 检查是否加载到了有效指令。

        Self {
            model,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            workspace,
            system_prompt: None,
            system_prompt_override: false,
            compaction_summary_prompt: None,
            messages: AppendLog::new(),
            total_usage: SessionUsage::default(),
            allow_shell,
            trust_mode,
            auto_approve: false,
            approval_mode: ApprovalMode::Suggest,  // 默认建议模式
            notes_path,
            mcp_config_path,
            id: uuid::Uuid::new_v4().to_string(),  // 生成一个随机的 UUID v4 字符串作为会话 ID。
            project_context: if has_context {
                Some(project_context)
            } else {
                None
            },
            last_system_prompt_hash: None,
            working_set: WorkingSet::default(),
            prefix_stability: None,
            frozen_prefix: None,
            messages_revision: 0,
        }
    }

    /// 向对话中添加一条消息
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.messages_revision = self.messages_revision.saturating_add(1);
    }

    /// 替换整个消息历史。由会话恢复和压缩使用。
    /// 即使新历史有不同的长度，也仅递增一次 `messages_revision`，以便下游缓存原子地失效。
    /// 替换整个消息历史。
    #[allow(dead_code)]
    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages.into();
        self.messages_revision = self.messages_revision.saturating_add(1);
    }

    /// 不修改消息列表内容，只递增 `messages_revision`。
    /// 预留给那些就地修改消息列表的场景（例如，就地重写内容块）。
    /// 大多数调用点不需要这个——推荐使用 [`add_message`](Self::add_message) 和
    /// [`replace_messages`](Self::replace_messages)。
    /// 不修改消息列表内容，只增加版本号。预留给那些就地修改消息的场景（比如改写某个内容块）。
    pub fn bump_messages_revision(&mut self) {
        self.messages_revision = self.messages_revision.saturating_add(1);
    }

    /// 从当前消息中重建工作集（尽力而为）。
    /// 从当前消息中重建工作集（尽力而为）
    pub fn rebuild_working_set(&mut self) {
        self.working_set
            .rebuild_from_messages(&self.messages, &self.workspace);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_usage_cache_starts_none() {
        let usage = SessionUsage::default();
        assert!(usage.cache_creation_input_tokens.is_none());
        assert!(usage.cache_read_input_tokens.is_none());
    }

    #[test]
    fn session_usage_cache_remains_none_when_api_omits_cache() {
        let mut usage = SessionUsage::default();
        let api_usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            prompt_cache_hit_tokens: None,
            prompt_cache_miss_tokens: None,
            reasoning_tokens: None,
            reasoning_replay_tokens: None,
            server_tool_use: None,
        };
        usage.add(&api_usage);
        assert!(usage.cache_creation_input_tokens.is_none());
        assert!(usage.cache_read_input_tokens.is_none());
    }

    #[test]
    fn session_usage_cache_accumulates_when_reported() {
        let mut usage = SessionUsage::default();
        let api_usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            prompt_cache_hit_tokens: Some(30),
            prompt_cache_miss_tokens: Some(70),
            reasoning_tokens: None,
            reasoning_replay_tokens: None,
            server_tool_use: None,
        };
        usage.add(&api_usage);
        assert_eq!(usage.cache_read_input_tokens, Some(30));
        assert_eq!(usage.cache_creation_input_tokens, Some(70));
        usage.add(&api_usage);
        assert_eq!(usage.cache_read_input_tokens, Some(60));
        assert_eq!(usage.cache_creation_input_tokens, Some(140));
    }

    #[test]
    fn session_usage_cache_preserves_explicit_zero() {
        let mut usage = SessionUsage::default();
        let api_usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            prompt_cache_hit_tokens: Some(0), // explicit zero from provider
            prompt_cache_miss_tokens: Some(1234),
            reasoning_tokens: None,
            reasoning_replay_tokens: None,
            server_tool_use: None,
        };
        usage.add(&api_usage);
        // 0 is a valid observed value, must NOT be converted to None
        assert_eq!(usage.cache_read_input_tokens, Some(0));
        assert_eq!(usage.cache_creation_input_tokens, Some(1234));
    }
}
