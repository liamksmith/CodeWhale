//! 追加式分层上下文管理，基于 Flash 接缝管理器（issue #159）。
//!
//! ## 为什么
//!
//! 当前的轮询/压缩/容量机制存在一个致命缺陷：它们会替换或重写消息，
//! 这破坏了 DeepSeek V4 的前缀缓存（SS4.2.1）。
//! 前缀缓存以 128 token 的粒度提供约 90% 的缓存 token 折扣。
//! 用摘要替换旧消息会破坏替换点的缓存——之后每个 token 都必须重新计算。
//!
//! 追加式分层方法保留所有原始消息，并追加由 V4 Flash 生成的
//! `<archived_context>` 摘要块。这些块是*导航辅助*——模型先读取它们，
//! 在需要精确信息时再深入查看原始消息。前缀缓存对
//! 整个稳定前缀保持有效。在 v0.7.5 中，此管理器为可选加入，
//! 同时审计缓存/时序策略。
//!
//! ## 软接缝级别
//!
//! | 级别 | 活跃输入触发阈值 | 覆盖消息范围    | 摘要密度      |
//! |-------|------------------|-----------------|----------------|
//! | L1    | 192K             | 0–128K          | ~2,500 token   |
//! | L2    | 384K             | 0–320K          | ~1,800 token   |
//! | L3    | 576K             | 0–512K          | ~1,200 token   |
//!
//! 阈值源自 V4 论文的图 9（MMR）：128K->256K 是实际上的拐点，降幅为 -0.09。
//! L1 在 192K 时触发，位于拐点之前。

use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use tokio::sync::Mutex;

use crate::client::DeepSeekClient;
use crate::compaction::KEEP_RECENT_MESSAGES;
use crate::compaction::plan_compaction;
use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt};

/// 默认接缝模型——Flash 便宜且快速，非常适合摘要生成。
pub const DEFAULT_SEAM_MODEL: &str = "deepseek-v4-flash";

/// 基于当前请求输入估算的默认阈值。
pub const DEFAULT_L1_THRESHOLD: usize = 192_000;
pub const DEFAULT_L2_THRESHOLD: usize = 384_000;
pub const DEFAULT_L3_THRESHOLD: usize = 576_000;

/// 逐字窗口：最后 N 轮对话永不参与摘要。
pub const VERBATIM_WINDOW_TURNS: usize = 16;

/// 每个接缝级别的大致 token 上限。
const L1_MAX_TOKENS: u32 = 3_200;
const L2_MAX_TOKENS: u32 = 2_400;
const L3_MAX_TOKENS: u32 = 1_600;

/// Flash 接缝管理器的配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeamConfig {
    /// 分层上下文管理器是否启用。
    pub enabled: bool,
    /// 逐字窗口：最后 N 轮对话永不参与摘要。
    pub verbatim_window_turns: usize,
    /// 基于当前请求输入估算的软接缝阈值。
    pub l1_threshold: usize,
    pub l2_threshold: usize,
    pub l3_threshold: usize,
    /// 用于接缝/简报工作的模型。
    pub seam_model: String,
}

impl Default for SeamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            verbatim_window_turns: VERBATIM_WINDOW_TURNS,
            l1_threshold: DEFAULT_L1_THRESHOLD,
            l2_threshold: DEFAULT_L2_THRESHOLD,
            l3_threshold: DEFAULT_L3_THRESHOLD,
            seam_model: DEFAULT_SEAM_MODEL.to_string(),
        }
    }
}

/// 单个软接缝块的元数据。
#[derive(Debug, Clone)]
pub struct SeamMetadata {
    /// 级别（1、2 或 3）。
    pub level: u8,
    /// 覆盖的消息范围（包含起始索引，不包含结束索引）。
    /// 保留供将来诊断使用。
    #[allow(dead_code)]
    pub start_idx: usize,
    #[allow(dead_code)]
    pub end_idx: usize,
    /// 摘要的大致 token 数。
    #[allow(dead_code)]
    pub token_estimate: usize,
    /// 接缝生成的时间。
    #[allow(dead_code)]
    pub timestamp: DateTime<Utc>,
    /// 生成摘要的模型。
    #[allow(dead_code)]
    pub model: String,
}

/// Flash 接缝管理器——生成 `<archived_context>` 块。
pub struct SeamManager {
    /// 用于摘要工作的 Flash 客户端。
    flash_client: DeepSeekClient,
    /// 配置文件。
    config: SeamConfig,
    /// 当前活跃的接缝列表（按从旧到新顺序）。
    active_seams: Arc<Mutex<Vec<SeamMetadata>>>,
}

impl SeamManager {
    /// 使用 Flash 客户端创建新的接缝管理器。
    pub fn new(flash_client: DeepSeekClient, config: SeamConfig) -> Self {
        Self {
            flash_client,
            config,
            active_seams: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 获取当前配置。
    pub fn config(&self) -> &SeamConfig {
        &self.config
    }

    /// 当前活跃的接缝数量。
    pub async fn seam_count(&self) -> usize {
        self.active_seams.lock().await.len()
    }

    /// 判断给定的当前请求输入估算是否应触发接缝以及触发哪个级别。
    /// 当不需要接缝时返回 `None`。
    #[must_use]
    pub fn seam_level_for(
        &self,
        active_input_tokens: usize,
        highest_existing_level: Option<u8>,
    ) -> Option<u8> {
        seam_level_for_active_input(&self.config, active_input_tokens, highest_existing_level)
    }

    /// 计算逐字窗口：最后 N 条消息的索引，这些消息永不参与摘要。
    /// 返回逐字窗口的起始索引。
    pub fn verbatim_window_start(&self, message_count: usize) -> usize {
        let turn_count = message_count / 2; // 粗略估算：每轮包含 user+assistant 各一条
        let verbatim_turns = self.config.verbatim_window_turns.min(turn_count);
        let verbatim_messages = (verbatim_turns * 2).min(message_count);
        message_count.saturating_sub(verbatim_messages)
    }

    /// 为给定的消息范围和级别生成软接缝。
    ///
    /// 返回 `<archived_context>` XML 块字符串，准备追加为助手消息。
    pub async fn produce_soft_seam(
        &self,
        messages: &[Message],
        level: u8,
        start_idx: usize,
        end_idx: usize,
        workspace: Option<&Path>,
        pinned_indices: &[usize],
    ) -> Result<String> {
        if messages.is_empty() || start_idx >= end_idx {
            return Ok(String::new());
        }

        let range = &messages[start_idx..end_idx.min(messages.len())];
        if range.is_empty() {
            return Ok(String::new());
        }

        // 使用压缩固定启发式算法来识别应从摘要中排除的消息。
        // 固定的消息保持原样；接缝摘要涵盖其余所有内容。
        let local_pins = local_pins_for_range(pinned_indices, start_idx, end_idx, messages.len());
        let plan = plan_compaction(
            range,
            workspace,
            KEEP_RECENT_MESSAGES.min(range.len().saturating_sub(1)),
            Some(&local_pins),
            None,
        );

        // 收集需要摘要的消息（未固定的），排除已固定的消息。
        let to_summarize: Vec<&Message> = range
            .iter()
            .enumerate()
            .filter(|(idx, _msg)| !plan.pinned_indices.contains(idx))
            .map(|(_idx, msg)| msg)
            .collect();

        if to_summarize.is_empty() {
            // 没有需要摘要的内容——所有消息都已固定。
            return Ok(String::new());
        }

        let summary = self
            .summarize_messages(&to_summarize, level, start_idx, end_idx)
            .await?;

        let density_label = match level {
            1 => "~2,500 tokens",
            2 => "~1,800 tokens",
            3 => "~1,200 tokens",
            _ => "unknown",
        };

        let timestamp = Utc::now();
        let token_estimate = summary.len() / 4;

        // 记录此接缝。
        {
            let mut seams = self.active_seams.lock().await;
            seams.push(SeamMetadata {
                level,
                start_idx,
                end_idx,
                token_estimate,
                timestamp,
                model: self.config.seam_model.clone(),
            });
        }

        Ok(format!(
            "<archived_context level=\"{level}\" range=\"msg {start_idx}-{end_idx}\" \
             tokens=\"~{token_estimate}\" density=\"{density_label}\" \
             model=\"{seam_model}\" timestamp=\"{ts}\">\n\
             {summary}\n\
             </archived_context>",
            seam_model = self.config.seam_model,
            ts = timestamp.to_rfc3339()
        ))
    }

    /// 将现有接缝重新压缩为更高级别的块。消费先前的
    /// `<archived_context>` 内容并与新消息融合。
    pub async fn recompact(
        &self,
        existing_seams: &[String],
        new_messages: &[&Message],
        level: u8,
        start_idx: usize,
        end_idx: usize,
    ) -> Result<String> {
        let mut input = String::from(
            "## Prior Context Summaries\n\n\
             The following <archived_context> blocks were produced earlier. \
             Merge their key information into a single denser summary.\n\n",
        );

        for (i, seam) in existing_seams.iter().enumerate() {
            let _ = write!(input, "### Seam {}\n{seam}\n\n", i + 1);
        }

        if !new_messages.is_empty() {
            input.push_str("## Recent Messages\n\n");
            for msg in new_messages {
                let role = &msg.role;
                for block in &msg.content {
                    if let ContentBlock::Text { text, .. } = block {
                        let _ = write!(input, "**{role}:** {text}\n\n");
                    }
                }
            }
        }

        let (max_tokens, word_limit) = match level {
            2 => (L2_MAX_TOKENS, 700),
            3 => (L3_MAX_TOKENS, 400),
            _ => (L3_MAX_TOKENS, 400),
        };

        let request = MessageRequest {
            model: self.config.seam_model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: format!(
                        "Synthesize the following context into a single dense summary. \
                         Preserve: decisions made, file paths, error messages, \
                         constraints, hypotheses, open questions, and task state. \
                         Drop: greeting, filler, repeated information. \
                         Keep it under {word_limit} words.\n\n{input}"
                    ),
                    cache_control: None,
                }],
            }],
            max_tokens,
            system: Some(SystemPrompt::Text(
                "You are a context compaction specialist. Produce dense, factual summaries that \
                 preserve every decision, path, error, constraint, and open question. Drop \
                 conversational filler and repetition."
                    .to_string(),
            )),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(false),
            temperature: Some(0.1),
            top_p: None,
        };

        let response = self.flash_client.create_message(request).await?;
        // 接缝重新压缩调用需要计费；通过旁路通道（#526）报告，
        // 以使页脚总额与 DeepSeek 网站一致。
        crate::cost_status::report(&response.model, &response.usage);
        let summary = response
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let token_estimate = summary.len() / 4;
        let timestamp = Utc::now();

        // 记录此重新压缩后的接缝。
        {
            let mut seams = self.active_seams.lock().await;
            seams.push(SeamMetadata {
                level,
                start_idx,
                end_idx,
                token_estimate,
                timestamp,
                model: self.config.seam_model.clone(),
            });
        }

        Ok(format!(
            "<archived_context level=\"{level}\" range=\"msg {start_idx}-{end_idx}\" \
             tokens=\"~{token_estimate}\" model=\"{model}\" timestamp=\"{ts}\">\n\
             {summary}\n\
             </archived_context>",
            model = self.config.seam_model,
            ts = timestamp.to_rfc3339()
        ))
    }

    /// 内部方法：使用 Flash 对消息切片进行摘要。
    async fn summarize_messages(
        &self,
        messages: &[&Message],
        level: u8,
        start_idx: usize,
        end_idx: usize,
    ) -> Result<String> {
        let mut conversation = String::new();

        for msg in messages {
            let role = if msg.role == "user" {
                "User"
            } else {
                "Assistant"
            };
            for block in &msg.content {
                match block {
                    ContentBlock::Text { text, .. } => {
                        let snippet = truncate_chars(text, 800);
                        let _ = write!(conversation, "{role}: {snippet}\n\n");
                    }
                    ContentBlock::ToolUse { name, .. } => {
                        let _ = write!(conversation, "{role}: [Used tool: {name}]\n\n");
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        let snippet = truncate_chars(content, 200);
                        let _ = write!(conversation, "Tool result: {snippet}\n\n");
                    }
                    ContentBlock::Thinking { .. } => {
                        // 在接缝摘要中跳过思考块。
                    }
                    ContentBlock::ServerToolUse { .. }
                    | ContentBlock::ToolSearchToolResult { .. }
                    | ContentBlock::CodeExecutionToolResult { .. }
                    | ContentBlock::ImageUrl { .. } => {}
                }
            }
        }

        let (max_tokens, word_limit) = match level {
            1 => (L1_MAX_TOKENS, 800),
            2 => (L2_MAX_TOKENS, 600),
            3 => (L3_MAX_TOKENS, 400),
            _ => (L3_MAX_TOKENS, 400),
        };

        let request = MessageRequest {
            model: self.config.seam_model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: format!(
                        "Summarize the following conversation segment (messages {start_idx}-{end_idx}). \
                         Preserve: key decisions and their rationale, exact file paths, \
                         command invocations, error messages, tool-result facts, constraints \
                         discovered, hypotheses being tested, and open questions. \
                         Drop: greetings, filler, repeated information, and thinking blocks. \
                         Keep it under {word_limit} words.\n\n---\n\n{conversation}"
                    ),
                    cache_control: None,
                }],
            }],
            max_tokens,
            system: Some(SystemPrompt::Text(
                "You are a context summarization specialist. Produce dense, factual summaries \
                 that preserve every decision, path, error, constraint, and open question. \
                 Never omit a file path, error message, or decision rationale."
                    .to_string(),
            )),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(false),
            temperature: Some(0.1),
            top_p: None,
        };

        let response = self.flash_client.create_message(request).await?;
        // 接缝摘要调用需要计费；通过旁路通道（#526）报告，
        // 以使页脚总额与 DeepSeek 网站一致。
        crate::cost_status::report(&response.model, &response.usage);
        let summary = response
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(summary)
    }

    /// 收集所有活跃接缝的文本内容（供重新压缩或简报使用）。
    pub async fn collect_seam_texts(&self, messages: &[Message]) -> Vec<String> {
        let _seams = self.active_seams.lock().await;
        let mut texts = Vec::new();

        // 从消息中提取 `<archived_context>` 块。
        for msg in messages {
            if msg.role == "assistant" {
                for block in &msg.content {
                    if let ContentBlock::Text { text, .. } = block
                        && text.contains("<archived_context")
                    {
                        texts.push(text.clone());
                    }
                }
            }
        }

        texts
    }

    /// 获取当前记录的最高接缝级别。
    pub async fn highest_level(&self) -> Option<u8> {
        let seams = self.active_seams.lock().await;
        seams.last().map(|s| s.level)
    }
}

#[must_use]
pub fn seam_level_for_active_input(
    config: &SeamConfig,
    active_input_tokens: usize,
    highest_existing_level: Option<u8>,
) -> Option<u8> {
    if !config.enabled {
        return None;
    }
    let highest = highest_existing_level.unwrap_or(0);

    // 每个级别最多触发一次，且必须按顺序触发。
    if highest < 1 && active_input_tokens >= config.l1_threshold {
        return Some(1);
    }
    if highest < 2 && active_input_tokens >= config.l2_threshold {
        return Some(2);
    }
    if highest < 3 && active_input_tokens >= config.l3_threshold {
        return Some(3);
    }
    None
}

/// 截断字符串至 max_chars 长度，尊重 Unicode 边界。
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

fn local_pins_for_range(
    pinned_indices: &[usize],
    start_idx: usize,
    end_idx: usize,
    message_count: usize,
) -> Vec<usize> {
    let end_idx = end_idx.min(message_count);
    pinned_indices
        .iter()
        .copied()
        .filter(|idx| *idx >= start_idx && *idx < end_idx)
        .map(|idx| idx - start_idx)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seam_levels_fire_in_order() {
        // 在测试环境中无法在没有 API 密钥的情况下创建 DeepSeekClient。
        // 仅测试纯逻辑函数。
        let config = SeamConfig::default();

        assert_eq!(seam_level_for_active_input(&config, 100_000, None), None);
        assert_eq!(seam_level_for_active_input(&config, 192_000, None), Some(1));
        assert_eq!(
            seam_level_for_active_input(&config, 384_000, Some(1)),
            Some(2)
        );
        assert_eq!(
            seam_level_for_active_input(&config, 576_000, Some(2)),
            Some(3)
        );
    }

    #[test]
    fn seam_trigger_uses_active_request_size_not_lifetime_usage() {
        let config = SeamConfig::default();
        let lifetime_prompt_usage = 900_000usize;
        let active_request_input = 120_000usize;

        assert!(lifetime_prompt_usage >= config.l3_threshold);
        assert_eq!(
            seam_level_for_active_input(&config, active_request_input, None),
            None
        );
    }

    #[test]
    fn verbatim_window_calculation() {
        let config = SeamConfig {
            verbatim_window_turns: 4,
            ..Default::default()
        };
        // 4 轮逐字对话 = 8 条消息
        // 20 条消息: 20 - (4*2) = 12
        assert_eq!(20usize.saturating_sub(8), 12);
        // 8 条消息: 8 - 8 = 0
        assert_eq!(8usize.saturating_sub(8), 0);
        // 4 条消息: 4 - 4 = 0
        assert_eq!(4usize.saturating_sub(4), 0);

        let _ = config;
    }

    #[test]
    fn truncate_chars_handles_unicode() {
        assert_eq!(truncate_chars("abc😀é", 3), "abc".to_string());
        assert_eq!(truncate_chars("abc😀é", 4), "abc😀".to_string());
        assert_eq!(truncate_chars("abc😀é", 10), "abc😀é".to_string());
        assert_eq!(truncate_chars("", 5), "".to_string());
    }

    #[test]
    fn global_pins_are_mapped_to_soft_seam_slice_indices() {
        let pins = vec![1, 4, 5, 8, 12];

        let local = local_pins_for_range(&pins, 4, 9, 10);

        assert_eq!(local, vec![0, 1, 4]);
    }

    #[test]
    fn disabled_config() {
        let config = SeamConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!config.enabled);
    }
}
