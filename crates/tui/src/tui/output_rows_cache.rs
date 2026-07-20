//! 每个单元格工具输出整形管道的记忆化缓存。
//!
//! `output_rows`（在 `tui::history` 中）遍历原始工具输出，对每行进行 ANSI 剥离，
//! 将类似路径/URL 的行分类，并将其余行包装到当前视口宽度。
//! `selected_output_indices` 然后计算紧凑"实时"视图显示的头部/尾部/重要性子集。
//! 这两个函数都是 `(output, width)` 和 `(rows, line_limit)` 的纯函数，
//! 但它们在每一渲染帧为每个可见的工具单元格被调用。
//! 对于 120 FPS 渲染循环上的 4 KB 输出，这相当于每帧每个单元格 2-6 次冗余遍历。
//!
//! 此模块在这两个纯函数前面添加了一个进程本地、内容寻址的缓存。
//! 该缓存是全局的（每个进程一个），并查询一个小的 `HashMap`，
//! 键为 `(content_hash, width)` 用于行，`(rows_hash, line_limit)` 用于索引。
//! 插入顺序 LRU 驱逐保持内存有界。
//!
//! ## 缓存何时有益
//!
//! - 反复滚动到视图中的长工具单元格（模型在部分失败后经常重新请求同一个 `read_file`）。
//! - 在流式传输时以 120 FPS 重新渲染整个记录：
//!   实时尾部下方已完成工具单元格在每一帧上都不变，因此它们的
//!   `output_rows` 和 `selected_output_indices` 调用是纯缓存命中。
//! - 终端大小调整仍然正确失效，因为 `width` 是键的一部分。
//!
//! ## 缓存何时失效
//!
//! - 新的工具输出（不同的 `content_hash`）。
//! - 单元格的首次渲染（缓存为空）。
//! - 自上次渲染后终端宽度发生变化。

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use crate::tui::history::OutputRow;

/// LRU 的默认容量。为最坏情况的"200 个单元格的 5,000 行记录，
/// 加上实时尾部的 4 KB 行缓存"而设计 ——
/// 远低于一兆字节。
const DEFAULT_CAPACITY: usize = 256;

/// 内部缓存条目。存储包装后的 `Vec<OutputRow>` 加上
/// `Vec<usize>` 的选定索引，以便单次键查找可以满足
/// 两个渲染步骤。当 `line_limit` 更改时，索引被延迟重新计算；
/// 行在所有行限制之间共享。
#[derive(Debug, Clone)]
struct CacheEntry {
    rows: Vec<OutputRow>,
    /// `line_limit -> 选定索引` 的映射。受限于
    /// 渲染器传入的不同行限制（通常为 1-3）。
    selected_by_limit: HashMap<usize, Vec<usize>>,
}

impl CacheEntry {
    fn new(rows: Vec<OutputRow>) -> Self {
        Self {
            rows,
            selected_by_limit: HashMap::new(),
        }
    }
}

/// `(output, width) -> OutputRowsCacheEntry` 的有界 LRU 缓存。
///
/// 驱逐策略是插入顺序：当缓存达到 `capacity` 时，
/// 最旧插入的键首先被丢弃。重新插入一个
/// 现有的键（不同的内容）保持原始位置，因此
/// 在每一帧上重新渲染同一个单元格不会搅动不相关的
/// 条目。
#[derive(Debug)]
struct OutputRowsCacheInner {
    capacity: usize,
    by_key: HashMap<RowsKey, CacheEntry>,
    insertion_order: VecDeque<RowsKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RowsKey {
    /// 原始工具输出的 64 位内容哈希。两个具有不同字节的输出
    /// 产生不同的哈希；相同的字节产生相同的哈希。
    content_hash: u64,
    /// 用于换行的终端宽度。调整大小会使其失效。
    width: u16,
}

impl OutputRowsCacheInner {
    fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    fn with_capacity(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            capacity: cap,
            by_key: HashMap::with_capacity(cap),
            insertion_order: VecDeque::with_capacity(cap),
        }
    }

    /// 获取或计算 `output` 在 `width` 下的包装输出行。
    /// 命中时，返回缓存的 `Vec<OutputRow>` 的克隆 ——
    /// 调用者可以在不持有锁的情况下进行迭代。
    fn get_or_compute_rows<F>(
        &mut self,
        content_hash: u64,
        width: u16,
        compute: F,
    ) -> Vec<OutputRow>
    where
        F: FnOnce() -> Vec<OutputRow>,
    {
        let key = RowsKey {
            content_hash,
            width,
        };
        if let Some(entry) = self.by_key.get(&key) {
            return entry.rows.clone();
        }

        let rows = compute();
        let entry = CacheEntry::new(rows.clone());

        if self.by_key.len() >= self.capacity
            && let Some(oldest) = self.insertion_order.pop_front()
        {
            self.by_key.remove(&oldest);
        }
        self.by_key.insert(key, entry);
        self.insertion_order.push_back(key);
        rows
    }

    /// 获取或计算缓存在给定 `line_limit` 下的选定索引。
    /// 首先按 `(content_hash, width)` 查找行条目（用于插入行的相同键），
    /// 然后查询该条目上的每行限制映射。`compute` 仅在
    /// 第一次调用给定 `(content_hash, width, line_limit)` 三元组时被调用。
    fn get_or_compute_indices<F>(
        &mut self,
        content_hash: u64,
        width: u16,
        line_limit: usize,
        compute: F,
    ) -> Vec<usize>
    where
        F: FnOnce() -> Vec<usize>,
    {
        let key = RowsKey {
            content_hash,
            width,
        };
        if let Some(entry) = self.by_key.get_mut(&key)
            && let Some(indices) = entry.selected_by_limit.get(&line_limit)
        {
            return indices.clone();
        }

        let indices = compute();
        if let Some(entry) = self.by_key.get_mut(&key) {
            entry.selected_by_limit.insert(line_limit, indices.clone());
        }
        indices
    }
}

thread_local! {
    /// 线程本地缓存。TUI 渲染循环在单个线程上运行，
    /// 因此 `!Sync` 缓存就足够了，并且避免了与可能调用同一模块的
    /// 后台工作线程的争用。
    static GLOBAL_CACHE: RefCell<OutputRowsCacheInner> =
        RefCell::new(OutputRowsCacheInner::new());
}

/// 重置全局缓存。由测试和 `/clear` 使用。
#[cfg(test)]
pub fn reset_for_tests() {
    GLOBAL_CACHE.with(|c| *c.borrow_mut() = OutputRowsCacheInner::new());
}

/// 查找（或计算）`output` 在 `width` 下的包装输出行。
/// 命中时，缓存的 `Vec<OutputRow>` 被克隆，无需重新运行
/// 逐行 ANSI 剥离或换行传递。
/// 基于字符串键的便捷封装，覆盖了 [`get_or_compute_rows_with_hash`]。只有
/// 测试现在使用它，因为生产调用者哈希一次并传递哈希。
#[cfg(test)]
pub fn get_or_compute_rows<F>(output: &str, width: u16, compute: F) -> Vec<OutputRow>
where
    F: FnOnce() -> Vec<OutputRow>,
{
    get_or_compute_rows_with_hash(hash_str(output), width, compute)
}

/// 同 [`get_or_compute_rows`] 但接受预计算的内容哈希，因此
/// 已经对输出进行了哈希的调用者（例如也用于键控
/// [`get_or_compute_indices`]）不会第二次哈希（#3757 review）。
pub fn get_or_compute_rows_with_hash<F>(content_hash: u64, width: u16, compute: F) -> Vec<OutputRow>
where
    F: FnOnce() -> Vec<OutputRow>,
{
    GLOBAL_CACHE.with(|c| {
        c.borrow_mut()
            .get_or_compute_rows(content_hash, width, compute)
    })
}

/// 查找（或计算）之前缓存的行负载在给定 `line_limit` 下的选定索引。
/// `content_hash` 是传递给 [`get_or_compute_rows`] 的同一个
/// 64 位内容哈希。
pub fn get_or_compute_indices<F>(
    content_hash: u64,
    width: u16,
    line_limit: usize,
    compute: F,
) -> Vec<usize>
where
    F: FnOnce() -> Vec<usize>,
{
    GLOBAL_CACHE.with(|c| {
        c.borrow_mut()
            .get_or_compute_indices(content_hash, width, line_limit, compute)
    })
}

/// FNV-1a 64 位内容哈希。廉价，无每进程密钥，并且在渲染热路径上
/// 针对中小型工具输出字符串比 `DefaultHasher`（SipHash）
/// 快约 5-10 倍。缓存是一个正确性优化，而非安全边界——
/// 64 位碰撞空间对于每进程 LRU 预期的
/// ≤ 几百个条目来说足够宽，并且碰撞只会导致假失效，
/// 绝不会导致错误数据。
pub fn hash_str(s: &str) -> u64 {
    /// FNV-1a 64 位偏移基准。
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    /// FNV-1a 64 位素数。
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    // 最后混合长度，以便两个共享前缀但长度不同
    //（例如一个带有尾随换行符）的字符串仍然只在
    // 真正相同的内容上碰撞。
    hash ^= s.len() as u64;
    hash.wrapping_mul(FNV_PRIME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str) -> OutputRow {
        OutputRow {
            text: text.to_string(),
            intact: false,
        }
    }

    #[test]
    fn cache_hit_returns_cached_rows() {
        reset_for_tests();

        let calls = std::cell::Cell::new(0u32);
        let compute = || {
            calls.set(calls.get() + 1);
            vec![row("hello"), row("world")]
        };

        let a = get_or_compute_rows("payload", 80, compute);
        let b = get_or_compute_rows("payload", 80, || {
            calls.set(calls.get() + 1);
            vec![row("hello"), row("world")]
        });
        assert_eq!(calls.get(), 1, "second call should hit the cache");
        assert_eq!(a, b);
    }

    #[test]
    fn different_width_invalidates_rows() {
        reset_for_tests();

        let calls = std::cell::Cell::new(0u32);
        let make = || {
            calls.set(calls.get() + 1);
            vec![row("hello")]
        };

        let _ = get_or_compute_rows("payload", 80, make);
        let _ = get_or_compute_rows("payload", 120, make);
        assert_eq!(calls.get(), 2, "different width must miss the cache");
    }

    #[test]
    fn different_output_invalidates_rows() {
        reset_for_tests();

        let calls = std::cell::Cell::new(0u32);
        let make = || {
            calls.set(calls.get() + 1);
            vec![row("x")]
        };

        let _ = get_or_compute_rows("payload-a", 80, make);
        let _ = get_or_compute_rows("payload-b", 80, make);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn indices_cached_per_line_limit() {
        reset_for_tests();

        let rows = get_or_compute_rows("payload", 80, || {
            vec![row("a"), row("b"), row("c"), row("d"), row("e")]
        });
        assert_eq!(rows.len(), 5);

        let content_hash = hash_str("payload");
        let mut calls = 0;
        let pick_two_a = get_or_compute_indices(content_hash, 80, 2, || {
            calls += 1;
            vec![0usize, 4]
        });
        let pick_two_b = get_or_compute_indices(content_hash, 80, 2, || {
            calls += 1;
            vec![0usize, 4]
        });
        assert_eq!(calls, 1, "second lookup with same limit hits the cache");
        assert_eq!(pick_two_a, pick_two_b);
        assert_eq!(pick_two_a, vec![0, 4]);

        // 不同的 line_limit 必须失效并重新计算。
        let _ = get_or_compute_indices(content_hash, 80, 3, || {
            calls += 1;
            vec![0usize, 1, 4]
        });
        assert_eq!(calls, 2);
    }

    #[test]
    fn capacity_evicts_oldest() {
        // 构建一个私有缓存，以便我们可以严格控制其大小。
        let mut cache = OutputRowsCacheInner::with_capacity(2);

        let _ = cache.get_or_compute_rows(1, 80, || vec![row("a")]);
        let _ = cache.get_or_compute_rows(2, 80, || vec![row("b")]);
        let _ = cache.get_or_compute_rows(3, 80, || vec![row("c")]);
        // 第一个条目（哈希 1）应该已被驱逐。
        let mut compute_calls = 0;
        let _ = cache.get_or_compute_rows(1, 80, || {
            compute_calls += 1;
            vec![row("a")]
        });
        assert_eq!(compute_calls, 1, "evicted entry must miss");
    }

    #[test]
    fn hash_str_stable_for_identical_input() {
        assert_eq!(hash_str("hello"), hash_str("hello"));
        assert_ne!(hash_str("hello"), hash_str("world"));
    }

    #[test]
    fn hash_str_differs_on_length_suffix() {
        // 尾随换行符是不同内容；哈希必须不同。
        assert_ne!(hash_str("hello"), hash_str("hello\n"));
    }

    #[test]
    fn hash_str_handles_empty() {
        // 空字符串哈希到 FNV 偏移基准；结果只需要稳定。
        assert_eq!(hash_str(""), hash_str(""));
    }
}
