//! 实时对话覆盖层的换行缓存（#94）。
//!
//! 每个单元格的渲染输出以 `(CellId, width, revision)` 键缓存。
//! 修订版本来自 `App.history_revisions`（或合成的活跃单元格修订版）；
//! 一旦单元格发生变更（因为上游标签发生变化），缓存就会使条目失效。
//! 宽度变化会使该单元格的所有条目失效，因为换行布局依赖于宽度。
//!
//! 活跃单元格（流式助手正文、进行中的工具条目）在每次变更时都会
//! 更新其修订版本，因此缓存始终反映其输出的最新帧，而无需为不相关的
//! 单元格付出重新换行的代价。由调整大小驱动的重新换行仅限于
//! 宽度键刚刚发生变化的单元格；其他内容不会被无效化。
//!
//! 缓存有上限，以在长会话中保持内存可预测。
//! 驱逐策略是简单的插入顺序方案——严格的 LRU 对于访问模式
//!（每个渲染帧全量扫描）来说过于复杂。

use std::collections::HashMap;
use std::collections::VecDeque;

use ratatui::text::Line;

/// 缓存条目数量的软上限，超过后执行插入顺序驱逐。
/// 针对最坏情况的"200 个单元格的 5000 行对话，调整大小两次"模式；
/// 即使有 10 KB 的单元格，也远低于 1 MB。
const DEFAULT_CAPACITY: usize = 512;

/// 实时渲染中对话单元格的标识符。`History(idx)` 用于定位
/// 给定索引处的已定稿历史单元格；`Active(entry_idx)` 用于在
/// 轮次进行中定位合成的活跃单元格条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CellId {
    History(usize),
    Active(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    cell: CellId,
    width: u16,
    revision: u64,
}

/// 有上限的换行缓存。键为 `(cell_id, width, revision)`——
/// 单元格修订版本的任何变化（变更）、终端宽度的变化（调整大小）
/// 或单元格身份的变化（插入/删除导致索引偏移）都会导致缓存未命中。
#[derive(Debug)]
pub struct TranscriptCache {
    capacity: usize,
    entries: HashMap<Key, Vec<Line<'static>>>,
    /// 插入顺序，以便在缓存满时驱逐最旧的条目。两步法
    ///（HashMap + VecDeque）使插入为 O(1)，查找保持 O(1)。
    insertion_order: VecDeque<Key>,
}

impl Default for TranscriptCache {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
}

impl TranscriptCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::with_capacity(capacity.max(1)),
            insertion_order: VecDeque::with_capacity(capacity.max(1)),
        }
    }

    /// 查找之前在此确切键处渲染的换行结果。
    /// 如果该单元格从未在此宽度/修订版处换行，则返回 `None`。
    #[must_use]
    pub fn get(&self, cell: CellId, width: u16, revision: u64) -> Option<&[Line<'static>]> {
        let key = Key {
            cell,
            width,
            revision,
        };
        self.entries.get(&key).map(Vec::as_slice)
    }

    /// 缓存一个全新的换行结果。如果缓存达到容量上限，
    /// 则首先驱逐最早插入的条目。
    pub fn insert(&mut self, cell: CellId, width: u16, revision: u64, lines: Vec<Line<'static>>) {
        let key = Key {
            cell,
            width,
            revision,
        };
        // 原地替换已有键——保持其在插入顺序队列中的位置，
        // 以免触发虚假驱逐。
        if self.entries.insert(key, lines).is_some() {
            return;
        }
        if self.entries.len() > self.capacity
            && let Some(oldest) = self.insertion_order.pop_front()
        {
            self.entries.remove(&oldest);
        }
        self.insertion_order.push_back(key);
    }

    /// 删除所有缓存的条目。当底层对话形状发生剧烈变化时使用
    ///（例如会话重置）。
    #[allow(dead_code)] // 为 /clear 和会话重置调用点保留。
    pub fn clear(&mut self) {
        self.entries.clear();
        self.insertion_order.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    fn line(s: &str) -> Line<'static> {
        Line::from(Span::raw(s.to_string()))
    }

    #[test]
    fn miss_returns_none() {
        let cache = TranscriptCache::new();
        assert!(cache.get(CellId::History(0), 80, 1).is_none());
    }

    #[test]
    fn round_trip_returns_inserted_lines() {
        let mut cache = TranscriptCache::new();
        let lines = vec![line("hello"), line("world")];
        cache.insert(CellId::History(0), 80, 1, lines.clone());
        let got = cache
            .get(CellId::History(0), 80, 1)
            .expect("条目应已缓存");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].spans[0].content, "hello");
    }

    #[test]
    fn revision_bump_invalidates_cell() {
        let mut cache = TranscriptCache::new();
        cache.insert(CellId::History(0), 80, 1, vec![line("v1")]);
        // rev=1 时命中
        assert!(cache.get(CellId::History(0), 80, 1).is_some());
        // rev=2 时未命中——调用方应重新换行并再次插入。
        assert!(cache.get(CellId::History(0), 80, 2).is_none());
    }

    #[test]
    fn width_change_invalidates_cell() {
        let mut cache = TranscriptCache::new();
        cache.insert(CellId::History(0), 80, 1, vec![line("v1")]);
        assert!(cache.get(CellId::History(0), 80, 1).is_some());
        assert!(cache.get(CellId::History(0), 100, 1).is_none());
    }

    #[test]
    fn active_cells_are_distinct_from_history() {
        let mut cache = TranscriptCache::new();
        cache.insert(CellId::History(0), 80, 1, vec![line("history")]);
        cache.insert(CellId::Active(0), 80, 1, vec![line("active")]);
        assert_eq!(
            cache.get(CellId::History(0), 80, 1).unwrap()[0].spans[0].content,
            "history"
        );
        assert_eq!(
            cache.get(CellId::Active(0), 80, 1).unwrap()[0].spans[0].content,
            "active"
        );
    }

    #[test]
    fn reinsert_same_key_does_not_evict() {
        // 容量为 2——重新插入已有键不得导致其他条目被驱逐；
        // 否则每帧重新渲染同一单元格会不断将无关条目挤出缓存。
        let mut cache = TranscriptCache::with_capacity(2);
        cache.insert(CellId::History(0), 80, 1, vec![line("a")]);
        cache.insert(CellId::History(1), 80, 1, vec![line("b")]);
        cache.insert(CellId::History(0), 80, 1, vec![line("a-prime")]);
        assert!(cache.get(CellId::History(1), 80, 1).is_some());
    }

    #[test]
    fn capacity_evicts_oldest_on_overflow() {
        let mut cache = TranscriptCache::with_capacity(2);
        cache.insert(CellId::History(0), 80, 1, vec![line("a")]);
        cache.insert(CellId::History(1), 80, 1, vec![line("b")]);
        cache.insert(CellId::History(2), 80, 1, vec![line("c")]);
        // 最旧条目（History(0)）应被移除；两个较新的键保留。
        assert!(cache.get(CellId::History(0), 80, 1).is_none());
        assert!(cache.get(CellId::History(1), 80, 1).is_some());
        assert!(cache.get(CellId::History(2), 80, 1).is_some());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn clear_drops_everything() {
        let mut cache = TranscriptCache::new();
        cache.insert(CellId::History(0), 80, 1, vec![line("v1")]);
        cache.clear();
        assert!(cache.get(CellId::History(0), 80, 1).is_none());
        assert_eq!(cache.len(), 0);
    }
}
