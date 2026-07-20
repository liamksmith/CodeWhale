//! 用于流式文本的换行门控。
//!
//! `LineBuffer` 是位于分块器上游的安全层，它将最后一个 `\n` 之后的任何文本
//! 保留到下一个换行符到达为止。这防止了不完整的多字符 Markdown——
//! 最重要的是不完整的代码围栏（` ``` `），其含义取决于同一行后续内容——
//! 进入渲染器的可见状态。
//!
//! 心智模型：
//! - `push(delta)` 将原始流文本追加到内部待处理缓冲区。
//! - `take_committable()` 仅返回直到并包括最后一个 `\n` 的前缀，
//!   并清除此前缀。最后一个 `\n` 之后的内容留在缓冲区中等待下一次 push。
//! - `flush()` 返回剩余内容，在模型信号通知轮次完成时的流结束时使用。
//!   （分块器上游的合约是：只有完整行的文本才会被提交；`flush()` 是在
//!   知道不会有更多文本到达时的显式逃生出口。）
//!
//! 完整原理参见任务简报中的 `cx5_chx5_newline_gate.md`。

/// 保存流式文本，直到到达换行边界。
///
/// 在流式处理管道中位于 [`StreamChunker`](super::commit_tick::StreamChunker) 的上游：
///
/// ```text
/// raw delta -> LineBuffer.push -> take_committable -> StreamChunker.push_delta -> commit tick
/// ```
///
/// 分块器也对它的待处理缓冲区执行"排空到最后一个换行符为止"的规则，
/// 但 `LineBuffer` 作为*独立*层存在，这样：
/// 1. 合约是显式的且可本地测试。
/// 2. 未来的下游消费者（例如乐观渲染排队行的实时预览）不会意外看到不完整的围栏。
/// 3. 轮次结束的 flush 语义归属于门控本身，而非策略。
#[derive(Debug, Default, Clone)]
pub struct LineBuffer {
    /// 自上次提交以来尚未释放的待处理文本，因为尚未看到终止的 `\n`。
    pending: String,
}

impl LineBuffer {
    /// 创建一个空缓冲区。
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加原始增量文本。
    pub fn push(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.pending.push_str(delta);
    }

    /// 返回待处理缓冲区中直到并包括最后一个 `\n` 的前缀。
    /// 该换行符之后的任何内容（如果有）将保持缓冲。
    ///
    /// 当缓冲区为空或尚未包含换行符时返回空字符串——
    /// 调用方可以将空字符串情况视为"此次 push 没有可提交的内容"。
    pub fn take_committable(&mut self) -> String {
        let Some(last_nl) = self.pending.rfind('\n') else {
            return String::new();
        };
        // 排空直到并包括最后一个换行符的所有内容。剩余的尾部（换行符之后）
        // 保留在 `pending` 中，并在做出下一个提交决策前与下一次 `push` 连接。
        self.pending.drain(..=last_nl).collect()
    }

    /// 返回缓冲区中的所有剩余内容，即使它不以换行符结尾。
    /// 在流结束时使用，这样我们就不会丢失最后的不完整行。
    pub fn flush(&mut self) -> String {
        std::mem::take(&mut self.pending)
    }

    /// 缓冲区是否包含任何未提交的文本。
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// 待处理尾部的字节长度（用于测试/可观测性）。
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 重置缓冲区（例如在流重新启动时）。
    pub fn reset(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_without_newline_holds_everything() {
        // 基石不变性：在换行符终止该行之前，任何内容都不会逃出门控。
        // 这就是保护不完整代码围栏的机制（例如 ``` 在块 N 中到达，
        // 语言标签在块 N+1 中到达）。
        let mut buf = LineBuffer::new();
        buf.push("hello");
        assert_eq!(buf.take_committable(), "");
        assert_eq!(buf.pending_len(), 5);
        assert!(!buf.is_empty());
    }

    #[test]
    fn push_with_trailing_partial_returns_only_prefix() {
        let mut buf = LineBuffer::new();
        buf.push("hello\nwo");
        assert_eq!(buf.take_committable(), "hello\n");
        // 尾部保留供下一次调用。
        assert_eq!(buf.pending_len(), 2);
        assert!(!buf.is_empty());
    }

    #[test]
    fn next_push_is_concatenated_with_held_tail() {
        let mut buf = LineBuffer::new();
        buf.push("hello\nwo");
        assert_eq!(buf.take_committable(), "hello\n");
        // 保留的 "wo" 与 "rld\n" 拼接，整行变得可提交。
        buf.push("rld\n");
        assert_eq!(buf.take_committable(), "world\n");
        assert!(buf.is_empty());
    }

    #[test]
    fn flush_returns_unterminated_tail() {
        let mut buf = LineBuffer::new();
        buf.push("trailing without newline");
        // 没有换行符 → 无可提交内容。
        assert_eq!(buf.take_committable(), "");
        // 流结束时的 flush 返回原始内容。
        assert_eq!(buf.flush(), "trailing without newline");
        assert!(buf.is_empty());
    }

    #[test]
    fn flush_is_empty_when_buffer_drained() {
        let mut buf = LineBuffer::new();
        buf.push("a\n");
        assert_eq!(buf.take_committable(), "a\n");
        assert_eq!(buf.flush(), "");
    }

    #[test]
    fn multi_line_burst_returns_prefix_through_last_newline() {
        // 一次 push 中包含多个换行符：一直到最后一个换行符的整个前缀
        // 一次性全部可提交；只有未终止的尾部被保留。
        let mut buf = LineBuffer::new();
        buf.push("a\nb\nc\nd");
        assert_eq!(buf.take_committable(), "a\nb\nc\n");
        assert_eq!(buf.pending_len(), 1);
        // 用换行符完成 "d"，在下次 take 时释放。
        buf.push("\n");
        assert_eq!(buf.take_committable(), "d\n");
    }

    #[test]
    fn partial_code_fence_never_escapes_the_gate() {
        // CX#5 的验收场景：一个代码围栏块的开始标记跨越多个增量到达时，
        // 绝对不能在没有终止换行符的情况下暴露 "foo```rust"。
        // 我们断言在每个中间提交中，*已提交*的文本要么包含换行符要么为空——
        // 即语言标记前的不完整围栏永远不会泄露。
        let mut buf = LineBuffer::new();

        // 块 1：以围栏开始标记结尾的段落片段。
        buf.push("foo```");
        let c1 = buf.take_committable();
        assert!(
            c1.is_empty() || c1.ends_with('\n'),
            "partial fence leaked: {c1:?}"
        );
        assert!(
            !c1.contains("foo```"),
            "fence opener escaped without newline: {c1:?}"
        );

        // 块 2：语言标签 + 正文开始。围栏行现在以换行符终止，
        // 因此可以提交；换行符后的正文被保留。
        buf.push("rust\nlet x");
        let c2 = buf.take_committable();
        assert!(
            c2.ends_with('\n'),
            "expected newline-terminated commit: {c2:?}"
        );
        assert_eq!(c2, "foo```rust\n");

        // 块 3：正文剩余部分和围栏关闭标记。
        buf.push("= 1;\n```\n");
        let c3 = buf.take_committable();
        assert_eq!(c3, "let x= 1;\n```\n");
        assert!(buf.is_empty());
    }

    #[test]
    fn empty_push_is_a_noop() {
        let mut buf = LineBuffer::new();
        buf.push("");
        assert!(buf.is_empty());
        assert_eq!(buf.take_committable(), "");
    }

    #[test]
    fn reset_clears_pending_tail() {
        let mut buf = LineBuffer::new();
        buf.push("partial");
        assert_eq!(buf.pending_len(), 7);
        buf.reset();
        assert!(buf.is_empty());
        assert_eq!(buf.flush(), "");
    }
}
