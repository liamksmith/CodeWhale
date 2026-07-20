//! 前缀缓存稳定性的三区域提示契约类型（#2264）。
//!
//! 将每个请求分为三个严格的区域：
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │ PinnedPrefix（构造后冻结）               │ ← 系统提示 + 工具目录
//! │   freeze() 时计算的 combined_sha256     │   缓存命中候选
//! ├─────────────────────────────────────────┤
//! │ AppendLog（仅追加）                     │ ← 对话历史
//! │   仅 push()，无插入/移除/编辑             │   保留先前轮次的前缀
//! ├─────────────────────────────────────────┤
//! │ TurnScratch（临时）                     │ ← 每轮元数据
//! │   每轮边界清除                          │   每请求唯一的新内容
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## 状态（阶段 1 基础）
//!
//! `PinnedPrefix` / `FrozenPrefix` / `PrefixDrift` 已可供使用。
//! `AppendLog` / `TurnScratch` / `ThreeZoneRequest` 是用于将来
//! 阶段的类型脚手架——尚未接入请求路径。

use crate::models::{Message, SystemPrompt, Tool};
// ── 辅助函数 ────────────────────────────────────────────────────────────

#[allow(dead_code)]
fn sha256_hex(bytes: &[u8]) -> String {
    crate::hashing::sha256_hex(bytes)
}

#[allow(dead_code)]
fn system_text(system: Option<&SystemPrompt>) -> String {
    match system {
        Some(SystemPrompt::Text(text)) => text.clone(),
        Some(SystemPrompt::Blocks(blocks)) => {
            let mut text = String::new();
            for block in blocks {
                text.push_str(&block.text);
                text.push('\n');
            }
            text
        }
        None => String::new(),
    }
}

/// 将工具序列化为确定性的、排序后的 JSON 字符串，用于哈希。
#[allow(dead_code)]
fn tool_catalog_digest(tools: &[Tool]) -> String {
    let mut serialized: Vec<String> = tools
        .iter()
        .filter_map(|t| serde_json::to_string(t).ok())
        .collect();
    serialized.sort();
    serialized.join("\n")
}

#[allow(dead_code)]
fn combined_hash(system_text: &str, tools: &[Tool]) -> String {
    let system_sha = sha256_hex(system_text.as_bytes());
    let tools_digest = tool_catalog_digest(tools);
    let tools_sha = sha256_hex(tools_digest.as_bytes());
    let combined = format!("{system_sha}:{tools_sha}");
    sha256_hex(combined.as_bytes())
}

// ── FrozenPrefix ───────────────────────────────────────────────────────

/// 不可变的冻结前缀——系统提示文本 + 工具目录，在冻结时哈希。
/// 只要系统提示文本和完整工具定义（名称、描述、模式）不变，
/// 哈希就保持稳定。
///
/// 使用 [`PinnedPrefix::freeze`] 来生成一个。
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct FrozenPrefix {
    pub(crate) system_text: String,
    pub(crate) tool_catalog: String,
    pub(crate) combined_sha256: String,
}

#[allow(dead_code)]
impl FrozenPrefix {
    /// 验证 `current_system_text` 和 `current_tools` 是否与冻结的
    /// 前缀匹配。稳定时返回 `Ok(())`，不匹配时返回 `Err(PrefixDrift)`。
    ///
    /// 快路径：在回退到 SHA-256 之前比较原始文本。
    pub fn verify(
        &self,
        current_system_text: &str,
        current_tools: &[Tool],
    ) -> Result<(), PrefixDrift> {
        let system_changed = current_system_text != self.system_text;
        let current_tool_catalog = tool_catalog_digest(current_tools);
        let tools_changed = current_tool_catalog != self.tool_catalog;

        if !system_changed && !tools_changed {
            return Ok(());
        }

        let current_hash = combined_hash(current_system_text, current_tools);
        Err(PrefixDrift {
            system_changed,
            tools_changed,
            frozen_hash: self.combined_sha256.clone(),
            current_hash,
        })
    }

    /// 返回用于显示的短（12 字符）人类可读 ID。
    #[must_use]
    pub fn short_id(&self) -> &str {
        if self.combined_sha256.len() >= 12 {
            &self.combined_sha256[..12]
        } else {
            &self.combined_sha256
        }
    }

    /// 返回完整的组合 SHA-256。
    #[must_use]
    pub fn hash(&self) -> &str {
        &self.combined_sha256
    }
}

// ── PinnedPrefix ───────────────────────────────────────────────────────

/// 可变前缀构建器。从系统提示和工具目录构造，然后调用
/// [`freeze`](Self::freeze) 来生成 [`FrozenPrefix`]。
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PinnedPrefix {
    system_text: String,
    tools: Vec<Tool>,
}

#[allow(dead_code)]
impl PinnedPrefix {
    #[must_use]
    pub fn new(system: Option<&SystemPrompt>, tools: Vec<Tool>) -> Self {
        Self {
            system_text: system_text(system),
            tools,
        }
    }

    /// 将此前缀冻结为不可变的 [`FrozenPrefix`]。
    #[must_use]
    pub fn freeze(&self) -> FrozenPrefix {
        let tool_catalog = tool_catalog_digest(&self.tools);
        let combined_sha256 = combined_hash(&self.system_text, &self.tools);

        FrozenPrefix {
            system_text: self.system_text.clone(),
            tool_catalog,
            combined_sha256,
        }
    }
}

// ── PrefixDrift ────────────────────────────────────────────────────────

/// 描述当前前缀与冻结基线的差异。
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct PrefixDrift {
    pub system_changed: bool,
    pub tools_changed: bool,
    pub frozen_hash: String,
    pub current_hash: String,
}

impl std::fmt::Display for PrefixDrift {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cause = match (self.system_changed, self.tools_changed) {
            (true, true) => "system prompt and tool set",
            (true, false) => "system prompt",
            (false, true) => "tool set",
            (false, false) => "unknown component",
        };
        write!(
            f,
            "prefix drift: {cause} changed (frozen={}, current={})",
            &self.frozen_hash[..12.min(self.frozen_hash.len())],
            &self.current_hash[..12.min(self.current_hash.len())]
        )
    }
}

// ── AppendLog ──────────────────────────────────────────────────────────

/// 仅追加的对话历史。通过 [`Deref`](std::ops::Deref) 解引用到
/// `&[Message]` 以进行透明读取访问；修改通过显式的方法
///（`push`、`truncate_to`、`trim_front`、`clear`）进行，
/// 这些方法的名称使缓存影响显而易见。
///
/// 阶段 4：`Session.messages` 的后端存储（#2264）。
#[derive(Debug, Clone)]
pub struct AppendLog {
    messages: Vec<Message>,
}

impl AppendLog {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    pub fn from_messages(messages: Vec<Message>) -> Self {
        Self { messages }
    }

    /// 向日志追加一条消息。单条消息的推送对于前缀缓存稳定性
    /// 是最便宜的变更——它扩展了字节序列而不干扰较早的轮次。
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// 一次操作追加多条消息（比重复 `push` 更少的缓存行失效）。
    pub fn push_batch(&mut self, batch: Vec<Message>) {
        self.messages.extend(batch);
    }

    /// 截断为仅保留最近 `new_len` 条消息。
    /// 丢弃较早的消息（以及它们对前缀缓存的贡献）
    /// 从前面开始丢弃。
    pub fn truncate_to(&mut self, new_len: usize) {
        self.messages.truncate(new_len);
    }

    /// 从前面移除 `count` 条消息（最旧的优先）。
    /// 破坏缓存：丢弃了较早轮次共享的前缀。
    pub fn trim_front(&mut self, count: usize) {
        if count >= self.messages.len() {
            self.messages.clear();
        } else {
            self.messages.drain(0..count);
        }
    }

    /// 移除所有消息。完全重置缓存状态。
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// 返回最后一条消息的可变引用（如果存在）。
    /// 优先使用此方法而非内部 vec 上的 `last_mut()`——
    /// 该名称表明仅修改最近轮次的内容。
    #[must_use]
    pub fn last_mut(&mut self) -> Option<&mut Message> {
        self.messages.last_mut()
    }

    /// 消费并返回内部的 `Vec<Message>`。
    #[must_use]
    pub fn into_inner(self) -> Vec<Message> {
        self.messages
    }
}

impl Default for AppendLog {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Vec<Message>> for AppendLog {
    fn from(messages: Vec<Message>) -> Self {
        Self { messages }
    }
}

impl From<AppendLog> for Vec<Message> {
    fn from(log: AppendLog) -> Self {
        log.messages
    }
}

impl std::ops::Deref for AppendLog {
    type Target = Vec<Message>;

    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}

// ── TurnScratch ────────────────────────────────────────────────────────

/// 每轮临时数据。在每个轮次边界清除。
///
/// **阶段 1 脚手架**——尚未接入引擎请求路径。
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct TurnScratch {
    pub working_set: Vec<String>,
    pub user_message: Option<Message>,
}

#[allow(dead_code)]
impl TurnScratch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.working_set.clear();
        self.user_message = None;
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.working_set.is_empty() && self.user_message.is_none()
    }
}

// ── ThreeZoneRequest ───────────────────────────────────────────────────

/// 准备好进行 DeepSeek API 序列化的组合三区域请求。
///
/// **阶段 1 脚手架**——尚未接入引擎请求路径。
/// 当前引擎继续直接使用 [`MessageRequest`]。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ThreeZoneRequest<'a> {
    pub prefix: &'a FrozenPrefix,
    pub log: &'a AppendLog,
    pub scratch: TurnScratch,
    pub model: String,
    pub max_tokens: u32,
    pub system: Option<SystemPrompt>,
    pub tools: Option<Vec<Tool>>,
    pub tool_choice: Option<serde_json::Value>,
    pub reasoning_effort: Option<String>,
    pub thinking: Option<serde_json::Value>,
    pub stream: Option<bool>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub metadata: Option<serde_json::Value>,
}

#[allow(dead_code)]
impl<'a> ThreeZoneRequest<'a> {
    /// 从系统提示、追加日志消息和临时用户消息构建完整消息列表。
    /// 返回的向量将序列化为 DeepSeek 聊天补全请求中的 `messages` 字段。
    #[must_use]
    pub fn build_messages(&self) -> Vec<Message> {
        let mut messages = Vec::with_capacity(self.message_count());

        match self.system.as_ref() {
            Some(SystemPrompt::Text(text)) => {
                messages.push(Message {
                    role: "system".to_string(),
                    content: vec![crate::models::ContentBlock::Text {
                        text: text.clone(),
                        cache_control: None,
                    }],
                });
            }
            Some(SystemPrompt::Blocks(blocks)) => {
                let content: Vec<crate::models::ContentBlock> = blocks
                    .iter()
                    .map(|block| crate::models::ContentBlock::Text {
                        text: block.text.clone(),
                        cache_control: block.cache_control.clone(),
                    })
                    .collect();
                messages.push(Message {
                    role: "system".to_string(),
                    content,
                });
            }
            None => {}
        }

        for msg in self.log.iter() {
            messages.push(msg.clone());
        }

        if let Some(ref user_msg) = self.scratch.user_message {
            messages.push(user_msg.clone());
        }

        messages
    }

    #[must_use]
    pub fn message_count(&self) -> usize {
        let system_count = if self.system.is_some() { 1 } else { 0 };
        let scratch_count = if self.scratch.user_message.is_some() {
            1
        } else {
            0
        };
        system_count + self.log.len() + scratch_count
    }
}

// ── 测试 ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ContentBlock;

    fn make_tool(name: &str) -> Tool {
        Tool {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::Value::Null,
            tool_type: None,
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: None,
            cache_control: None,
        }
    }

    fn make_message(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    // ── FrozenPrefix / PinnedPrefix ────────────────────────────────

    #[test]
    fn freeze_produces_stable_hash() {
        let tools = vec![make_tool("read"), make_tool("write")];
        let sys = SystemPrompt::Text("hello world".to_string());

        let a = PinnedPrefix::new(Some(&sys), tools.clone()).freeze();
        let b = PinnedPrefix::new(Some(&sys), tools).freeze();

        assert_eq!(a.combined_sha256, b.combined_sha256);
        assert_eq!(a.hash(), b.hash());
        assert_eq!(a.short_id(), b.short_id());
    }

    #[test]
    fn freeze_tool_order_is_stable() {
        let sys = SystemPrompt::Text("system".to_string());
        let tools_a = vec![make_tool("b"), make_tool("a")];
        let tools_b = vec![make_tool("a"), make_tool("b")];

        let a = PinnedPrefix::new(Some(&sys), tools_a).freeze();
        let b = PinnedPrefix::new(Some(&sys), tools_b).freeze();

        assert_eq!(a.combined_sha256, b.combined_sha256);
    }

    #[test]
    fn freeze_empty_tools() {
        let sys = SystemPrompt::Text("system".to_string());
        let frozen = PinnedPrefix::new(Some(&sys), vec![]).freeze();
        assert!(frozen.tool_catalog.is_empty());
        assert!(!frozen.combined_sha256.is_empty());
        assert_eq!(frozen.short_id().len(), 12);
    }

    #[test]
    fn freeze_no_system() {
        let tools = vec![make_tool("t1")];
        let frozen = PinnedPrefix::new(None, tools).freeze();
        assert!(frozen.system_text.is_empty());
        assert!(frozen.tool_catalog.contains("t1"));
    }

    #[test]
    fn verify_passes_when_stable() {
        let sys = SystemPrompt::Text("system".to_string());
        let tools = vec![make_tool("a")];
        let frozen = PinnedPrefix::new(Some(&sys), tools.clone()).freeze();

        assert!(frozen.verify("system", &tools).is_ok());
    }

    #[test]
    fn verify_detects_system_change() {
        let sys = SystemPrompt::Text("old".to_string());
        let tools = vec![make_tool("a")];
        let frozen = PinnedPrefix::new(Some(&sys), tools.clone()).freeze();

        let drift = frozen.verify("new", &tools).unwrap_err();
        assert!(drift.system_changed);
        assert!(!drift.tools_changed);
    }

    #[test]
    fn verify_detects_tool_change() {
        let sys = SystemPrompt::Text("system".to_string());
        let tools_a = vec![make_tool("a")];
        let frozen = PinnedPrefix::new(Some(&sys), tools_a).freeze();

        let tools_b = vec![make_tool("b")];
        let drift = frozen.verify("system", &tools_b).unwrap_err();
        assert!(!drift.system_changed);
        assert!(drift.tools_changed);
    }

    #[test]
    fn verify_detects_both_changes() {
        let sys = SystemPrompt::Text("old".to_string());
        let tools = vec![make_tool("a")];
        let frozen = PinnedPrefix::new(Some(&sys), tools).freeze();

        let drift = frozen.verify("new", &[make_tool("b")]).unwrap_err();
        assert!(drift.system_changed);
        assert!(drift.tools_changed);
    }

    #[test]
    fn verify_detects_schema_change() {
        let sys = SystemPrompt::Text("system".to_string());
        let tool_a = make_tool("a");
        let mut tool_a_v2 = make_tool("a");
        tool_a_v2.description = "updated desc".to_string();

        let frozen = PinnedPrefix::new(Some(&sys), vec![tool_a]).freeze();
        let drift = frozen.verify("system", &[tool_a_v2]).unwrap_err();
        // 相同名称，不同模式——应检测到变化。
        assert!(drift.tools_changed);
    }

    #[test]
    fn prefix_drift_display_is_readable() {
        let drift = PrefixDrift {
            system_changed: true,
            tools_changed: false,
            frozen_hash: "a".repeat(64),
            current_hash: "b".repeat(64),
        };
        let display = drift.to_string();
        assert!(display.contains("system prompt"));
        assert!(display.contains("aaaaaaaaaaaa"));
        assert!(display.contains("bbbbbbbbbbbb"));
    }

    // ── AppendLog ─────────────────────────────────────────────────

    #[test]
    fn append_log_push_and_iter() {
        let mut log = AppendLog::new();
        assert!(log.is_empty());

        log.push(make_message("user", "hello"));
        log.push(make_message("assistant", "hi"));

        assert_eq!(log.len(), 2);
        assert!(!log.is_empty());

        let messages: Vec<_> = log.iter().collect();
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn append_log_from_messages() {
        let msgs = vec![make_message("user", "a"), make_message("assistant", "b")];
        let log = AppendLog::from_messages(msgs);
        assert_eq!(log.len(), 2);
        assert_eq!(log.as_slice().len(), 2);
    }

    // ── TurnScratch ───────────────────────────────────────────────

    #[test]
    fn scratch_clear_empties_all_fields() {
        let mut scratch = TurnScratch::new();
        scratch.working_set.push("file.rs".to_string());
        scratch.user_message = Some(make_message("user", "task"));

        assert!(!scratch.is_empty());
        scratch.clear();
        assert!(scratch.is_empty());
        assert!(scratch.working_set.is_empty());
        assert!(scratch.user_message.is_none());
    }

    // ── ThreeZoneRequest ──────────────────────────────────────────

    #[test]
    fn build_messages_concatenates_zones() {
        let sys = SystemPrompt::Text("you are helpful".to_string());
        let tools = vec![make_tool("read")];
        let prefix = PinnedPrefix::new(Some(&sys), tools).freeze();

        let mut log = AppendLog::new();
        log.push(make_message("user", "prev question"));
        log.push(make_message("assistant", "prev answer"));

        let scratch = TurnScratch {
            working_set: vec!["main.rs".to_string()],
            user_message: Some(make_message("user", "current task")),
        };

        let request = ThreeZoneRequest {
            prefix: &prefix,
            log: &log,
            scratch,
            model: "deepseek-v4-pro".to_string(),
            max_tokens: 4096,
            system: Some(sys),
            tools: None,
            tool_choice: None,
            reasoning_effort: None,
            thinking: None,
            stream: None,
            temperature: None,
            top_p: None,
            metadata: None,
        };

        let messages = request.build_messages();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[2].role, "assistant");
        assert_eq!(messages[3].role, "user");
        assert_eq!(request.message_count(), 4);
    }

    #[test]
    fn build_messages_no_system_no_scratch() {
        let prefix = PinnedPrefix::new(None, vec![]).freeze();

        let mut log = AppendLog::new();
        log.push(make_message("user", "hi"));

        let request = ThreeZoneRequest {
            prefix: &prefix,
            log: &log,
            scratch: TurnScratch::new(),
            model: "x".to_string(),
            max_tokens: 1,
            system: None,
            tools: None,
            tool_choice: None,
            reasoning_effort: None,
            thinking: None,
            stream: None,
            temperature: None,
            top_p: None,
            metadata: None,
        };

        let messages = request.build_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(request.message_count(), 1);
    }

    #[test]
    fn blocks_system_prompt_preserves_cache_control() {
        use crate::models::{CacheControl, SystemBlock};
        let cc = Some(CacheControl {
            cache_type: "ephemeral".to_string(),
        });
        let blocks = SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: "hello".to_string(),
            cache_control: cc.clone(),
        }]);

        let prefix = PinnedPrefix::new(Some(&blocks), vec![]).freeze();
        let log = AppendLog::new();
        let scratch = TurnScratch::new();
        let request = ThreeZoneRequest {
            prefix: &prefix,
            log: &log,
            scratch,
            model: "x".to_string(),
            max_tokens: 1,
            system: Some(blocks),
            tools: None,
            tool_choice: None,
            reasoning_effort: None,
            thinking: None,
            stream: None,
            temperature: None,
            top_p: None,
            metadata: None,
        };

        let messages = request.build_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "system");
        // cache_control 应在块上保留。
        if let ContentBlock::Text {
            cache_control: actual_cc,
            ..
        } = &messages[0].content[0]
        {
            assert_eq!(
                actual_cc.as_ref().map(|c| c.cache_type.as_str()),
                Some("ephemeral")
            );
        } else {
            panic!("expected Text content block");
        }
    }
}
