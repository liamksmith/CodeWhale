//! 事后翻译拦截层。
//!
//! 当输出翻译启用时（`/translate`），此模块提供拦截逻辑，检测英文模型输出
//! 并在显示前将其替换为中文翻译。主要机制是 `prompts.rs` 中的系统提示指令；
//! 此模块是当模型输出尽管有指令仍泄露英文时的后备方案。
//!
//! ## 架构
//!
//! - `needs_translation()` — 启发式检测文本是否主要为英文并应被翻译。
//! - `translate_text()` — 通过共享的 `DeepSeekClient` 调用当前会话模型，
//!   将文本翻译为当前区域设置。专用的翻译代理只接收源文本并只返回翻译结果
//!   — 无工具调用，无对话历史。
//! - `TranslationStatus` — 在 UI 中追踪每条消息的翻译状态。

use anyhow::Result;

use crate::client::DeepSeekClient;

/// 启发式阈值：如果超过此比例的字母字符是拉丁字母（A-Z / a-z），
/// 则文本被视为英文。
const ENGLISH_LATIN_RATIO_THRESHOLD: f64 = 0.6;

/// 应用启发式所需的最小字母字符数 — 避免在短混合语言字符串上出现误报。
const MIN_ALPHA_CHARS_FOR_DETECTION: usize = 10;

/// 每个 CJK 字符相当于多少个拉丁字母"信息单位"。
/// 单个 CJK 字符约携带一个简短英文单词（2-4 个字母）的信息量，
/// 因此我们将 CJK 加权为 3 倍以便公平比较。
const CJK_CHAR_WEIGHT: usize = 3;

/// 检测文本内容是否主要是英文并应被翻译。
///
/// 启发式比较 CJK 字符（加权）与拉丁字母。
/// CJK 字符每个字形携带的信息量更大，因此即使在英文单词中包含少量中文字符的字符串也不会被标记。
#[must_use]
pub fn needs_translation(text: &str) -> bool {
    let mut latin_count = 0usize;
    let mut cjk_count = 0usize;

    for ch in text.chars() {
        if ch.is_ascii_alphabetic() {
            latin_count += 1;
        } else if is_cjk(ch) {
            cjk_count += 1;
        }
    }

    let total_alpha = latin_count + (cjk_count * CJK_CHAR_WEIGHT);

    if total_alpha < MIN_ALPHA_CHARS_FOR_DETECTION {
        return false;
    }

    // 如果加权的 CJK 占主导地位，说明已经是中文 — 无需翻译。
    if (cjk_count * CJK_CHAR_WEIGHT) > latin_count {
        return false;
    }

    let ratio = latin_count as f64 / total_alpha as f64;
    ratio >= ENGLISH_LATIN_RATIO_THRESHOLD
}

/// 检查字符是否在 CJK 统一表意文字区块内，或者是常见的中/日/韩字符。
fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Unified Ideographs Extension A
        | '\u{2E80}'..='\u{2EFF}' // CJK Radicals Supplement
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth and Fullwidth Forms
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
    )
}

/// 使用专用翻译代理将文本翻译到请求的目标语言。
///
/// 这是一个轻量级的、聚焦的 API 调用 — 无流式传输、无工具调用、无对话历史。
/// 代理的唯一职责是翻译。
///
/// # 错误
///
/// 如果 API 调用失败或响应格式错误，则返回错误。
pub async fn translate_text(
    text: &str,
    client: &DeepSeekClient,
    model: &str,
    target_language: &str,
) -> Result<String> {
    client.translate(text, model, target_language).await
}

/// 单条消息翻译操作的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TranslationStatus {
    /// 无需翻译（已经是中文或文本不足）。
    NotNeeded,
    /// 翻译正在进行中 — 原始英文仍显示，带有指示器。
    Pending,
    /// 翻译成功完成。
    Done,
    /// 翻译失败 — 显示原始英文并附带后备说明。
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_avoids_false_positive() {
        assert!(!needs_translation("hi"));
        assert!(!needs_translation("ok"));
    }

    #[test]
    fn english_text_detected() {
        assert!(needs_translation(
            "This is a message from the assistant explaining how the code works."
        ));
    }

    #[test]
    fn chinese_text_not_detected() {
        assert!(!needs_translation(
            "这是助手的一条中文回复，解释了代码的工作原理。"
        ));
    }

    #[test]
    fn mixed_mostly_english_detected() {
        assert!(needs_translation(
            "The function handle_request takes a Request param and returns a Response."
        ));
    }

    #[test]
    fn mixed_mostly_chinese_not_detected() {
        assert!(!needs_translation(
            "这个 handle_request 函数接收一个 Request 参数并返回 Response。"
        ));
    }

    #[test]
    fn code_with_short_labels_not_falsely_detected() {
        assert!(!needs_translation("let x = 1; let y = 2;"));
    }

    #[test]
    fn long_english_code_is_detected() {
        assert!(needs_translation(
            "function calculateTotalRevenueForQuarterlyReport() { return; }"
        ));
    }

    #[test]
    fn js_comments_in_english_detected() {
        assert!(needs_translation(
            "// This is a JavaScript function that handles user authentication\nfunction login() {}"
        ));
    }
}
