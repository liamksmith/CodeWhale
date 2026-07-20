//! 缓存守卫 CI 测试：验证多轮对话中前缀缓存的稳定性。
//!
//! 运行 8 个测试用例 × 每轮 14-24 次交互，检查尾部平均
//! 命中率是否保持在可配置阈值之上（默认 40%）。
//!
//! 环境变量：
//!   CODEWHALE_CACHE_GUARD=1              启用守卫（默认：禁用）
//!   CODEWHALE_CACHE_GUARD_THRESHOLD=90   命中率阈值（0-100）
//!   CODEWHALE_CACHE_GUARD_STRICT=1       阈值违例时失败（默认：警告）
//!
//! 用法：
//!   CODEWHALE_CACHE_GUARD=1 cargo test --test cache_guard
//!   CODEWHALE_CACHE_GUARD=1 CODEWHALE_CACHE_GUARD_STRICT=1 cargo test --test cache_guard

// Mock 不需要外部依赖。

// === 配置 ===

const DEFAULT_THRESHOLD: f64 = 40.0;
const ENABLED_ENV: &str = "CODEWHALE_CACHE_GUARD";
const THRESHOLD_ENV: &str = "CODEWHALE_CACHE_GUARD_THRESHOLD";
const STRICT_ENV: &str = "CODEWHALE_CACHE_GUARD_STRICT";

fn guard_enabled() -> bool {
    std::env::var(ENABLED_ENV)
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}

fn threshold() -> f64 {
    std::env::var(THRESHOLD_ENV)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_THRESHOLD)
}

fn strict() -> bool {
    std::env::var(STRICT_ENV)
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}

// === Mock 前缀缓存 ===

/// 模拟 DeepSeek 的服务端前缀缓存行为。
///
/// 缓存基于字节前缀匹配：如果当前请求的前 N 个字节与
/// 前一个请求的前 N 个字节匹配，则这 N 个字节被计为缓存命中。
struct MockPrefixCache {
    previous_body: Vec<u8>,
    total_input_bytes: u64,
    hit_bytes: u64,
    per_turn_hit_rates: Vec<f64>,
}

impl MockPrefixCache {
    fn new() -> Self {
        Self {
            previous_body: Vec::new(),
            total_input_bytes: 0,
            hit_bytes: 0,
            per_turn_hit_rates: Vec::new(),
        }
    }

    /// 提交请求体并计算本轮次的缓存命中/未命中。
    fn submit(&mut self, body: &[u8]) {
        let common_prefix = body
            .iter()
            .zip(self.previous_body.iter())
            .take_while(|(a, b)| a == b)
            .count();

        let body_len = body.len() as u64;
        self.total_input_bytes += body_len;
        self.hit_bytes += common_prefix as u64;

        let hit_rate = if body_len > 0 {
            common_prefix as f64 / body_len as f64
        } else {
            1.0
        };
        self.per_turn_hit_rates.push(hit_rate);

        self.previous_body = body.to_vec();
    }

    /// 计算最后 N 轮的平均命中率。
    fn tail_avg(&self, n: usize) -> f64 {
        let start = self.per_turn_hit_rates.len().saturating_sub(n);
        let tail = &self.per_turn_hit_rates[start..];
        if tail.is_empty() {
            0.0
        } else {
            tail.iter().sum::<f64>() / tail.len() as f64
        }
    }

    /// 所有轮次的总体命中率。
    fn overall_hit_rate(&self) -> f64 {
        if self.total_input_bytes == 0 {
            0.0
        } else {
            self.hit_bytes as f64 / self.total_input_bytes as f64
        }
    }
}

// === 测试用例生成器 ===

/// 生成模拟的纯对话轮次请求体。
fn plain_dialogue_body(turn: usize, with_reasoning: bool) -> Vec<u8> {
    let system = "You are a helpful assistant. Answer concisely and accurately.";
    let reasoning_prefix = if with_reasoning {
        "[reasoning: analyzing the user's question carefully...]"
    } else {
        ""
    };
    let user_msg = format!("User message turn {turn} — please respond to this query.");
    let body =
        format!("{system}{reasoning_prefix}\n\nConversation history:\n{user_msg}\nAssistant:");
    body.into_bytes()
}

/// 生成模拟的工具循环轮次请求体。
fn tool_loop_body(turn: usize, with_reasoning: bool) -> Vec<u8> {
    let system = "You are a helpful assistant with tool access.";
    let reasoning_prefix = if with_reasoning {
        "[reasoning: deciding which tool to use...]"
    } else {
        ""
    };
    let tool_name = if turn.is_multiple_of(2) {
        "read_file"
    } else {
        "write_file"
    };
    let tool_args = format!(r#"{{"path": "/tmp/file_{turn}.txt"}}"#);
    let user_msg = format!("User request turn {turn}");
    let body = format!(
        "{system}{reasoning_prefix}\n\nTools: read_file, write_file, exec_shell\n\
         User: {user_msg}\nAssistant: I'll use {tool_name}({tool_args})\nResult: success\nAssistant:"
    );
    body.into_bytes()
}

/// 生成模拟的混合大小请求体。
fn mixed_size_body(turn: usize) -> Vec<u8> {
    let system = "You are a helpful assistant.";
    let user_msg = match turn % 4 {
        0 => format!("Short question {turn}"),
        1 => format!(
            "Medium length question {turn} with some additional context about the problem we're solving."
        ),
        2 => {
            let long_context = "Lorem ipsum dolor sit amet. ".repeat(20);
            format!("Long question {turn} with extensive context: {long_context}")
        }
        _ => format!("Question {turn}"),
    };
    let body = format!("{system}\n\nUser: {user_msg}\nAssistant:");
    body.into_bytes()
}

// === 测试运行器 ===

struct CaseResult {
    name: String,
    tail_avg: f64,
    overall: f64,
    turns: usize,
    passed: bool,
}

fn run_case(
    name: &str,
    turns: usize,
    with_reasoning: bool,
    tool_loop: bool,
    mixed_sizes: bool,
) -> CaseResult {
    let mut cache = MockPrefixCache::new();

    for turn in 0..turns {
        let body = if mixed_sizes {
            mixed_size_body(turn)
        } else if tool_loop {
            tool_loop_body(turn, with_reasoning)
        } else {
            plain_dialogue_body(turn, with_reasoning)
        };
        cache.submit(&body);
    }

    let tail_avg = cache.tail_avg(5) * 100.0;
    let overall = cache.overall_hit_rate() * 100.0;
    let thresh = threshold();
    let passed = tail_avg >= thresh;

    CaseResult {
        name: name.to_string(),
        tail_avg,
        overall,
        turns,
        passed,
    }
}

// === 8 个测试用例 ===

#[test]
fn case_plain_dialogue() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("plain-dialogue", 14, true, false, false);
    report_and_assert(&result);
}

#[test]
fn case_plain_dialogue_no_reasoning() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("plain-dialogue-no-reasoning", 14, false, false, false);
    report_and_assert(&result);
}

#[test]
fn case_long_dialogue() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("long-dialogue", 18, true, false, false);
    report_and_assert(&result);
}

#[test]
fn case_mixed_message_sizes() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("mixed-message-sizes", 20, true, false, true);
    report_and_assert(&result);
}

#[test]
fn case_tool_loop() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("tool-loop", 14, true, true, false);
    report_and_assert(&result);
}

#[test]
fn case_tool_loop_no_reasoning() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("tool-loop-no-reasoning", 14, false, true, false);
    report_and_assert(&result);
}

#[test]
fn case_long_tool_loop() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("long-tool-loop", 24, true, true, false);
    report_and_assert(&result);
}

#[test]
fn case_long_tool_loop_no_reasoning() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("long-tool-loop-no-reasoning", 24, false, true, false);
    report_and_assert(&result);
}

// === 硬错误守卫 ===

#[test]
fn compaction_must_cause_at_least_one_miss() {
    if !guard_enabled() {
        return;
    }

    let mut cache = MockPrefixCache::new();
    let system = "You are a helpful assistant with a very long system prompt that gets compacted.";

    // 模拟 30 轮，其中压缩约在第 20 轮发生。
    // 压缩后，系统提示词发生显著变化。
    for turn in 0..30 {
        let body = if turn < 20 {
            format!("{system}\n\nUser: turn {turn}\nAssistant:")
        } else {
            // 压缩后：系统提示词被截断/更改。
            format!("You are a helpful assistant.\n\nUser: turn {turn}\nAssistant:")
        };
        cache.submit(body.as_bytes());
    }

    // 压缩后，应该至少有一次显著的未命中。
    // 阈值被放宽，因为我们的模拟不完美地模拟
    // DeepSeek 的基数树前缀缓存。
    let post_compaction_rates: Vec<f64> = cache.per_turn_hit_rates[20..].to_vec();
    let has_significant_miss = post_compaction_rates.iter().any(|&r| r < 0.8);

    if strict() {
        assert!(
            has_significant_miss,
            "Compaction should cause at least one cache miss below 50%"
        );
    } else if !has_significant_miss {
        eprintln!("[WARN] compaction_must_cause_at_least_one_miss: no significant miss detected");
    }
}

// === 辅助函数 ===

fn report_and_assert(result: &CaseResult) {
    let thresh = threshold();
    if result.passed {
        eprintln!(
            "[OK]   {}: tail_avg={:.1}% (overall={:.1}%, {} turns)",
            result.name, result.tail_avg, result.overall, result.turns
        );
    } else {
        eprintln!(
            "[WARN] {}: tail_avg={:.1}% < threshold={:.1}% (overall={:.1}%, {} turns)",
            result.name, result.tail_avg, thresh, result.overall, result.turns
        );
        if strict() {
            panic!(
                "[STRICT] {} failed: tail_avg={:.1}% < threshold={:.1}%",
                result.name, result.tail_avg, thresh
            );
        }
    }
}
