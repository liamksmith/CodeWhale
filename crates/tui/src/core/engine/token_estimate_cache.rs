//! [`crate::compaction::estimate_input_tokens_conservative`] 的进程级记忆化。
//!
//! Token 估算器会遍历完整的 [`crate::models::Message`] 历史记录和
//! 活跃的系统提示词，这是引擎热路径中每个轮次最昂贵的 CPU 开销。
//! 相同的输入数据在每个轮次中至少被五个位置查询：容量前/后工具检查点、
//! 错误升级、接缝管理器以及修剪消息预算检查，另外还有来自 TUI 页脚、
//! `/status`、`/debug` 和上下文检查器的四个查询位置。
//!
//! 没有记忆化时，包含 200 条消息历史和 5 KB 工具结果的场景
//! 每次调用约需 2 ms；一个轮次中就是 20 ms 的纯浪费。估算器
//! 本身是 `(messages, system_prompt)` 的纯函数，因此
//! 内容版本化的缓存是安全的：调用者在每次变更时递增
//! `messages_revision`，我们还将系统提示词的快速指纹
//! 作为键的一部分。
//!
//! 该缓存仅为进程级——跨会话持久化被有意排除在范围之外
//!（跨会话的提示词基磁盘缓存请参见 PR #2520）。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::compaction::estimate_input_tokens_conservative;
use crate::models::{Message, SystemPrompt};

/// 滚动审计环的默认容量。设计为 64 条目的窗口
/// 可在无界增长的情况下覆盖完整的容量控制器观察周期。
const AUDIT_RING_CAPACITY: usize = 64;

/// `estimate_input_tokens_conservative` 的进程级记忆化。
///
/// 缓存键为 `(messages_revision, system_fingerprint)`
/// 对，引擎在每次内容变更时都会递增两者。命中时
/// 返回先前存储的 Token 估算值，无需重新遍历
/// 消息列表。未命中时，运行估算器并将结果存储
/// 在审计环条目旁边。
#[derive(Debug, Default, Clone)]
pub struct TokenEstimateCache {
    /// 引擎在每次消息变更时递增的单调计数器。
    messages_revision: u64,
    /// 当前系统提示词文本的稳定 64 位哈希。在缓存未命中时
    /// 每次 `lookup_or_compute` 调用计算一次。
    system_fingerprint: u64,
    /// 缓存的 Token 计数，仅当两个键都匹配当前输入时有效。
    cached_tokens: Option<usize>,
    /// 最近 (revision, tokens) 对的审计环。最新条目
    /// 是尾部；超出容量时丢弃最旧的。用于
    /// 可观测性，向 `/status` 展示缓存效果。
    audit_ring: Vec<(u64, usize)>,
    /// 自上次缓存清除以来的缓存命中次数。饱和于
    /// `u64::MAX`（实践中几乎不可能达到）。
    hits: u64,
    /// 自上次缓存清除以来的缓存未命中次数。
    misses: u64,
}

impl TokenEstimateCache {
    /// 构造一个全新的空缓存。`messages_revision` 默认为 0；
    /// 引擎必须在发生变更时调用 [`bump_messages_revision`](Self::bump_messages_revision)
    /// 以便下次查找正确失效。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回缓存的 Token 估算值，未命中时重新计算。
    ///
    /// `messages_revision` 是引擎的单调计数器；在每次
    /// 添加/删除/清除时递增。`system_prompt` 可以为 `None`。`messages`
    /// 在调用期间被借用，以便未命中时可以重新 Token 化。
    pub fn lookup_or_compute(
        &mut self,
        messages_revision: u64,
        system_prompt: Option<&SystemPrompt>,
        messages: &[Message],
    ) -> usize {
        let system_fingerprint = fingerprint_system_prompt(system_prompt);

        if self.messages_revision == messages_revision
            && self.system_fingerprint == system_fingerprint
            && let Some(tokens) = self.cached_tokens
        {
            self.hits = self.hits.saturating_add(1);
            return tokens;
        }

        let tokens = estimate_input_tokens_conservative(messages, system_prompt);
        self.messages_revision = messages_revision;
        self.system_fingerprint = system_fingerprint;
        self.cached_tokens = Some(tokens);
        self.misses = self.misses.saturating_add(1);
        self.push_audit(messages_revision, tokens);
        tokens
    }

    /// 记录消息修订版本递增。引擎在
    /// `session.messages` 被变更时调用此方法。使用小于
    /// 当前值的值调用是空操作（缓存是单调的）。
    #[allow(dead_code)] // 为将来 /clear 和重置路径的接线而暴露；测试会用到
    pub fn bump_messages_revision(&mut self, revision: u64) {
        if revision > self.messages_revision {
            self.messages_revision = revision;
            self.cached_tokens = None;
        }
    }

    /// 忘记所有缓存状态。由 `/clear` 和会话重置路径使用。
    #[allow(dead_code)] // 为将来 /clear 和重置路径的接线而暴露；测试会用到
    pub fn invalidate(&mut self) {
        self.cached_tokens = None;
        self.system_fingerprint = 0;
        self.audit_ring.clear();
        self.hits = 0;
        self.misses = 0;
    }

    /// 返回自上次 `invalidate` 调用以来的 `(hits, misses)` 计数器。
    #[allow(dead_code)] // 通过后续的 /status 展现；测试会用到
    #[must_use]
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// 返回最新的 `(revision, tokens)` 审计条目，最新的
    /// 在最前面。受 [`AUDIT_RING_CAPACITY`] 限制。
    #[allow(dead_code)] // 通过后续的 /status 展现；测试会用到
    #[must_use]
    pub fn recent_audit(&self) -> &[(u64, usize)] {
        &self.audit_ring
    }

    fn push_audit(&mut self, revision: u64, tokens: usize) {
        if self.audit_ring.len() >= AUDIT_RING_CAPACITY {
            self.audit_ring.remove(0);
        }
        self.audit_ring.push((revision, tokens));
    }
}

/// 系统提示词文本的稳定 64 位哈希。遍历与估算器
/// 消费的相同形状：一个 `Text` 变体或 `Blocks` 列表。
/// 为 `None` 返回 0，使空情况可区分但比较廉价。
fn fingerprint_system_prompt(system: Option<&SystemPrompt>) -> u64 {
    let Some(system) = system else {
        return 0;
    };
    let mut hasher = DefaultHasher::new();
    match system {
        SystemPrompt::Text(text) => {
            "text".hash(&mut hasher);
            text.hash(&mut hasher);
        }
        SystemPrompt::Blocks(blocks) => {
            "blocks".hash(&mut hasher);
            blocks.len().hash(&mut hasher);
            for block in blocks {
                block.block_type.hash(&mut hasher);
                block.text.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ContentBlock, SystemBlock};

    fn user_text(s: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: s.to_string(),
                cache_control: None,
            }],
        }
    }

    fn sys_text(s: &str) -> SystemPrompt {
        SystemPrompt::Text(s.to_string())
    }

    #[test]
    fn first_call_is_a_miss() {
        let mut cache = TokenEstimateCache::new();
        let messages = vec![user_text("hello world")];
        let tokens = cache.lookup_or_compute(1, None, &messages);
        let (hits, misses) = cache.stats();
        assert!(tokens > 0);
        assert_eq!(hits, 0);
        assert_eq!(misses, 1);
    }

    #[test]
    fn repeated_call_with_same_revision_is_a_hit() {
        let mut cache = TokenEstimateCache::new();
        let messages = vec![user_text("hello world")];
        let _ = cache.lookup_or_compute(1, None, &messages);
        let _ = cache.lookup_or_compute(1, None, &messages);
        let (hits, misses) = cache.stats();
        assert_eq!(hits, 1);
        assert_eq!(misses, 1);
    }

    #[test]
    fn revision_bump_invalidates() {
        let mut cache = TokenEstimateCache::new();
        let messages = vec![user_text("hi")];
        let a = cache.lookup_or_compute(1, None, &messages);
        let b = cache.lookup_or_compute(2, None, &messages);
        let (hits, misses) = cache.stats();
        // 两次调用都是未命中（不同版本号），都没有命中缓存。
        assert_eq!(a, b);
        assert_eq!(hits, 0);
        assert_eq!(misses, 2);
    }

    #[test]
    fn system_prompt_change_invalidates() {
        let mut cache = TokenEstimateCache::new();
        let messages = vec![user_text("hi")];
        let _ = cache.lookup_or_compute(1, Some(&sys_text("alpha")), &messages);
        let _ = cache.lookup_or_compute(1, Some(&sys_text("beta")), &messages);
        let (hits, misses) = cache.stats();
        assert_eq!(hits, 0);
        assert_eq!(misses, 2);
    }

    #[test]
    fn bump_messages_revision_clears_cache() {
        let mut cache = TokenEstimateCache::new();
        let messages = vec![user_text("x")];
        let _ = cache.lookup_or_compute(1, None, &messages);
        cache.bump_messages_revision(2);
        let _ = cache.lookup_or_compute(2, None, &messages);
        let (hits, misses) = cache.stats();
        assert_eq!(hits, 0);
        assert_eq!(misses, 2);
    }

    #[test]
    fn bump_to_smaller_revision_is_noop() {
        let mut cache = TokenEstimateCache::new();
        let messages = vec![user_text("x")];
        let _ = cache.lookup_or_compute(5, None, &messages);
        cache.bump_messages_revision(2);
        // 版本号降低了，缓存对于版本 5 应该仍然有效
        let _ = cache.lookup_or_compute(5, None, &messages);
        let (hits, _) = cache.stats();
        assert_eq!(hits, 1, "向下的版本递增不能使缓存失效");
    }

    #[test]
    fn invalidate_resets_state() {
        let mut cache = TokenEstimateCache::new();
        let messages = vec![user_text("x")];
        let _ = cache.lookup_or_compute(1, None, &messages);
        let _ = cache.lookup_or_compute(1, None, &messages);
        cache.invalidate();
        let (hits, misses) = cache.stats();
        assert_eq!(hits, 0);
        assert_eq!(misses, 0);
    }

    #[test]
    fn blocks_system_prompt_yields_distinct_fingerprint() {
        let blocks_a = SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: "alpha".to_string(),
            cache_control: None,
        }]);
        let blocks_b = SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: "beta".to_string(),
            cache_control: None,
        }]);
        let mut cache = TokenEstimateCache::new();
        let messages = vec![user_text("hi")];
        let _ = cache.lookup_or_compute(1, Some(&blocks_a), &messages);
        let _ = cache.lookup_or_compute(1, Some(&blocks_b), &messages);
        let (hits, misses) = cache.stats();
        assert_eq!(hits, 0);
        assert_eq!(misses, 2);
    }

    #[test]
    fn audit_ring_records_recent_pairs() {
        let mut cache = TokenEstimateCache::new();
        let messages = vec![user_text("hi")];
        for rev in 1..=5 {
            let _ = cache.lookup_or_compute(rev, None, &messages);
        }
        let ring = cache.recent_audit();
        assert_eq!(ring.len(), 5);
        assert_eq!(ring.last().copied(), Some((5, ring.last().unwrap().1)));
    }

    #[test]
    fn audit_ring_bounded_by_capacity() {
        let mut cache = TokenEstimateCache::new();
        let messages = vec![user_text("hi")];
        for rev in 1..=(AUDIT_RING_CAPACITY + 10) as u64 {
            let _ = cache.lookup_or_compute(rev, None, &messages);
        }
        let ring = cache.recent_audit();
        assert_eq!(ring.len(), AUDIT_RING_CAPACITY);
        // 最新条目应该是最新请求的版本号
        assert_eq!(ring.last().unwrap().0, (AUDIT_RING_CAPACITY + 10) as u64);
    }
}
