//! 会话记录渲染的滚动状态跟踪。
//!
//! 会话记录视图使用扁平的行索引滚动模型：一个指向渲染的行元数据缓冲区的
//! 单个 `offset` 指向顶部可见行，保留 `usize::MAX` 作为表示"跟上实时尾部"的
//! 哨兵值。
//!
//! 为什么是扁平偏移而非单元格锚点？早期设计将视口锚定到 `(cell_index, line_in_cell)`
//! 对，假设单元格列表是只追加的。但事实并非如此——内容重写（RLM `repl`
//! 块扩展为 `Thinking + Text`、工具结果替换和压缩）可能会在用户下方
//! 重新编号或移除单元格。当锚点单元格消失时，视口会跳到底部（问题 #56）
//! 或"卡住"，因为下一个按键会从 `max_start` 解析。
//!
//! Codex 的分页器使用相同的行偏移形状；参见
//! `codex-rs/tui/src/pager_overlay.rs::PagerView`。

use std::time::{Duration, Instant};

use crate::tui::ui_text::CopyLineSeparator;

const TRACKPAD_EVENT_WINDOW: Duration = Duration::from_millis(35);
const WHEEL_LINES_PER_TICK: i32 = 3;
const TRACKPAD_BASE_LINES_PER_TICK: i32 = 1;
const TRACKPAD_MID_LINES_PER_TICK: i32 = 2;
const TRACKPAD_MAX_LINES_PER_TICK: i32 = 3;

// === 会话记录行元数据 ===

/// 描述渲染的会话记录行如何映射到历史单元格的元数据。
///
/// 滚动状态本身不查询这个——它只存储一个扁平行偏移——
/// 但其他渲染时辅助函数（选择绘制、发送闪烁、跳转到工具、
/// 滚动条百分比）仍然需要缓存暴露的行→单元格映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptLineMeta {
    CellLine {
        cell_index: usize,
        line_in_cell: usize,
        copy_prefix_width: usize,
        copy_separator_after: CopyLineSeparator,
    },
    Spacer,
}

impl TranscriptLineMeta {
    /// 如果此条目是单元格行，返回单元格/行索引。
    #[must_use]
    pub fn cell_line(&self) -> Option<(usize, usize)> {
        match *self {
            TranscriptLineMeta::CellLine {
                cell_index,
                line_in_cell,
                ..
            } => Some((cell_index, line_in_cell)),
            TranscriptLineMeta::Spacer => None,
        }
    }

    #[must_use]
    pub fn copy_separator_after(&self) -> CopyLineSeparator {
        match *self {
            TranscriptLineMeta::CellLine {
                copy_separator_after,
                ..
            } => copy_separator_after,
            TranscriptLineMeta::Spacer => CopyLineSeparator::Newline,
        }
    }

    #[must_use]
    pub fn copy_prefix_width(&self) -> usize {
        match *self {
            TranscriptLineMeta::CellLine {
                copy_prefix_width, ..
            } => copy_prefix_width,
            TranscriptLineMeta::Spacer => 0,
        }
    }
}

// === 会话记录滚动状态 ===

/// 表示"跟上实时尾部"的哨兵偏移——渲染器在绘制时将其转换为 `max_start`，
/// 因此新追加的行将视图下拉。
const TAIL_SENTINEL: usize = usize::MAX;

/// 会话记录视图的扁平行偏移滚动状态。
///
/// 存储顶部可见行在缓存 `line_meta` 缓冲区中的索引，
/// 或 [`TAIL_SENTINEL`]（`usize::MAX`）表示"固定在底部"。
/// 渲染器每帧将哨兵解析为当前行数和视口高度，
/// 因此内容重写只是钳制用户的偏移，而不是触发锚点恢复启发式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptScroll {
    offset: usize,
}

impl Default for TranscriptScroll {
    /// 默认状态是"跟上实时尾部"——与调用者已经依赖的历史
    /// `TranscriptScroll::ToBottom` 行为一致。
    fn default() -> Self {
        Self::to_bottom()
    }
}

impl TranscriptScroll {
    /// 跟上实时尾部的状态（默认）。
    #[must_use]
    pub const fn to_bottom() -> Self {
        Self {
            offset: TAIL_SENTINEL,
        }
    }

    /// 固定到特定行索引的状态。
    #[must_use]
    pub const fn at_line(offset: usize) -> Self {
        Self { offset }
    }

    /// 当视图正在跟上实时尾部时返回 true。
    #[must_use]
    pub const fn is_at_tail(self) -> bool {
        self.offset == TAIL_SENTINEL
    }

    /// 将滚动状态解析为具体的顶部行索引。
    ///
    /// `max_start` 是 `total_lines.saturating_sub(visible_lines)`。
    /// 返回的 `Self` 是规范化后的状态——如果解析的顶部到达了尾部
    ///（或者会话记录适合一个屏幕），我们折叠为 [`TranscriptScroll::to_bottom`]，
    /// 以便调用者可以将返回的状态视为权威状态。
    ///
    /// `line_meta` 为了与之前基于单元格锚点的实现的 API 兼容性而被接受。
    /// 在此处未使用，因为扁平偏移模型不需要单元格索引查找；我们只需钳制。
    #[must_use]
    pub fn resolve_top(self, line_meta: &[TranscriptLineMeta], max_start: usize) -> (Self, usize) {
        let _ = line_meta;
        if self.offset == TAIL_SENTINEL {
            return (Self::to_bottom(), max_start);
        }
        let top = self.offset.min(max_start);
        if top >= max_start {
            (Self::to_bottom(), max_start)
        } else {
            (Self::at_line(top), top)
        }
    }

    /// 应用滚动增量并返回更新后的状态。
    ///
    /// `delta_lines` 是有符号的：负数向上滚动（向开头），
    /// 正数向下滚动（向尾部）。当解析的偏移到达 `max_start` 时，
    /// 我们快照到 [`TranscriptScroll::to_bottom`]，以便后续追加的内容
    /// 将视图一起下拉。
    ///
    /// `line_meta` 为了 API 兼容性而被接受；只查询其长度。
    /// `visible_lines` 控制用于钳制的页面大小。
    #[must_use]
    pub fn scrolled_by(
        self,
        delta_lines: i32,
        line_meta: &[TranscriptLineMeta],
        visible_lines: usize,
    ) -> Self {
        if delta_lines == 0 {
            return self;
        }

        let total_lines = line_meta.len();
        if total_lines <= visible_lines {
            // 整个会话记录适合；只有"尾部"有意义。
            return Self::to_bottom();
        }

        let max_start = total_lines.saturating_sub(visible_lines);
        let current_top = if self.offset == TAIL_SENTINEL {
            max_start
        } else {
            self.offset.min(max_start)
        };

        let new_top = if delta_lines < 0 {
            current_top.saturating_sub(delta_lines.unsigned_abs() as usize)
        } else {
            let delta = usize::try_from(delta_lines).unwrap_or(usize::MAX);
            current_top.saturating_add(delta).min(max_start)
        };

        if new_top >= max_start {
            Self::to_bottom()
        } else {
            Self::at_line(new_top)
        }
    }

    /// 将滚动状态固定到渲染会话记录中的特定行索引
    ///（饱和到元数据缓冲区长度）。
    ///
    /// 如果 `line_meta` 为空则返回 `None`（在这种情况下调用者应默认为
    /// [`TranscriptScroll::to_bottom`]）。
    #[must_use]
    pub fn anchor_for(line_meta: &[TranscriptLineMeta], start: usize) -> Option<Self> {
        if line_meta.is_empty() {
            return None;
        }
        let clamped = start.min(line_meta.len().saturating_sub(1));
        Some(Self::at_line(clamped))
    }
}

/// 鼠标滚轮输入的方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
}

impl ScrollDirection {
    fn sign(self) -> i32 {
        match self {
            ScrollDirection::Up => -1,
            ScrollDirection::Down => 1,
        }
    }
}

/// 鼠标滚轮累积的有状态跟踪器。
#[derive(Debug, Default)]
pub struct MouseScrollState {
    last_event_at: Option<Instant>,
    last_direction: Option<ScrollDirection>,
    rapid_same_direction_ticks: u8,
}

/// 来自用户输入的计算滚动增量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollUpdate {
    pub delta_lines: i32,
}

impl MouseScrollState {
    /// 创建新的滚动状态跟踪器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 处理滚动事件并返回结果增量。
    pub fn on_scroll(&mut self, direction: ScrollDirection) -> ScrollUpdate {
        let now = Instant::now();
        self.on_scroll_at(direction, now)
    }

    fn on_scroll_at(&mut self, direction: ScrollDirection, now: Instant) -> ScrollUpdate {
        let is_trackpad = self
            .last_event_at
            .is_some_and(|last| now.saturating_duration_since(last) < TRACKPAD_EVENT_WINDOW);
        let same_direction = self.last_direction == Some(direction);

        self.last_event_at = Some(now);
        self.last_direction = Some(direction);

        let lines_per_tick = if is_trackpad {
            if same_direction {
                self.rapid_same_direction_ticks = self.rapid_same_direction_ticks.saturating_add(1);
            } else {
                self.rapid_same_direction_ticks = 1;
            }
            match self.rapid_same_direction_ticks {
                0..=2 => TRACKPAD_BASE_LINES_PER_TICK,
                3..=5 => TRACKPAD_MID_LINES_PER_TICK,
                _ => TRACKPAD_MAX_LINES_PER_TICK,
            }
        } else {
            self.rapid_same_direction_ticks = 0;
            WHEEL_LINES_PER_TICK
        };

        ScrollUpdate {
            delta_lines: direction.sign() * lines_per_tick,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell_line(cell_index: usize, line_in_cell: usize) -> TranscriptLineMeta {
        TranscriptLineMeta::CellLine {
            cell_index,
            line_in_cell,
            copy_prefix_width: 0,
            copy_separator_after: CopyLineSeparator::Newline,
        }
    }

    /// 为具有 `cell_count` 个单元格（每个 `lines_per_cell` 行高，由分隔符分隔）
    /// 的会话记录构建合成行元数据数组。
    fn synth_line_meta(cell_count: usize, lines_per_cell: usize) -> Vec<TranscriptLineMeta> {
        let mut meta = Vec::new();
        for cell in 0..cell_count {
            for line in 0..lines_per_cell {
                meta.push(cell_line(cell, line));
            }
            if cell + 1 < cell_count {
                meta.push(TranscriptLineMeta::Spacer);
            }
        }
        meta
    }

    /// 默认状态跟上实时尾部。针对任何 `max_start` 解析都会
    /// 返回 `max_start` 和规范尾部状态。
    #[test]
    fn default_state_is_tail() {
        let state = TranscriptScroll::default();
        assert!(state.is_at_tail());
        let meta = synth_line_meta(5, 3);
        let max_start = 6;
        let (resolved, top) = state.resolve_top(&meta, max_start);
        assert!(resolved.is_at_tail());
        assert_eq!(top, max_start);
    }

    /// `max_start` 以下的固定偏移解析为自身，不变。
    ///（原先："锚点单元格仍然存在"——相同意图：滚动位置在仍然有效时保留。）
    #[test]
    fn resolve_top_keeps_position_when_offset_in_range() {
        let meta = synth_line_meta(5, 3); // 19 个条目
        let max_start = meta.len().saturating_sub(8);
        let state = TranscriptScroll::at_line(9);
        let (resolved, top) = state.resolve_top(&meta, max_start);
        assert_eq!(resolved, TranscriptScroll::at_line(9));
        assert_eq!(top, 9);
    }

    /// 问题 #56 的回归测试：当内容重写缩小会话记录时，用户的偏移
    /// 超过了新的 `max_start`，我们钳制到新的最大值——我们绝不能跳转到顶部，
    /// 也不能通过将用户发送到重写前内容的原始底部来静默丢失位置。
    /// 捕捉到尾部是正确的行为，因为用户的预期位置下不再有任何内容。
    #[test]
    fn resolve_top_clamps_when_offset_past_max_start() {
        let meta = synth_line_meta(3, 2); // 8 个条目（单元格 0..3，2 行 + 2 个分隔符）
        let max_start = meta.len().saturating_sub(4);
        // 用户曾经滚动到重写后不再存在的一行。
        let state = TranscriptScroll::at_line(15);
        let (resolved, top) = state.resolve_top(&meta, max_start);
        // 超过 max_start 折叠为尾部（这是正确答案：
        // max_start 之后没有内容可显示）。
        assert!(resolved.is_at_tail());
        assert_eq!(top, max_start);
    }

    /// 我们在此重构中防范的新错误的回归测试：上滚到会话记录中间，
    /// 内容在我们下方重写，然后再次绘制，当偏移仍在范围内时必须
    /// 保留偏移（如果需要则钳制），而不得跳转到顶部或底部。
    #[test]
    fn resolve_top_preserves_midway_offset_after_content_rewrite() {
        // 重写前的会话记录：10 个单元格 × 3 行 + 9 个分隔符 = 39 行。
        let pre = synth_line_meta(10, 3);
        let visible = 8;
        let pre_max_start = pre.len().saturating_sub(visible);

        // 用户上滚到中间某行（第 12 行）。
        let state = TranscriptScroll::at_line(12);
        let (state, top_before) = state.resolve_top(&pre, pre_max_start);
        assert_eq!(top_before, 12);
        assert_eq!(state, TranscriptScroll::at_line(12));

        // 内容重写：第 4 个单元格扩展了两行（例如内联
        // RLM `repl` 块变成了 Thinking + Text）。总数增长。
        let mut post = pre.clone();
        post.insert(13, cell_line(4, 3));
        post.insert(14, cell_line(4, 4));
        let post_max_start = post.len().saturating_sub(visible);
        let (state2, top_after) = state.resolve_top(&post, post_max_start);
        // 关键：仍在第 12 行，未拉到底部或顶部。
        assert_eq!(state2, TranscriptScroll::at_line(12));
        assert_eq!(top_after, 12);

        // 内容重写将会话记录缩小到偏移以下。
        let post_shrunk = synth_line_meta(3, 3); // 总共 11 行
        let shrunk_max_start = post_shrunk.len().saturating_sub(visible);
        let (state3, top_shrunk) = state.resolve_top(&post_shrunk, shrunk_max_start);
        // 偏移 12 > 11；我们钳制到尾部（max_start 之后无内容）。
        assert!(state3.is_at_tail());
        assert_eq!(top_shrunk, shrunk_max_start);
    }

    /// 从过时偏移 `scrolled_by`：按下 Up 应仍将用户向上移动，
    /// 而不是锁定在底部。扁平偏移模型使这变得简单——
    /// 只需在应用增量前将偏移钳制到 `max_start`。
    #[test]
    fn scrolled_by_does_not_teleport_on_stale_offset() {
        let meta = synth_line_meta(3, 2); // 8 个条目
        let visible = 4;
        let max_start = meta.len().saturating_sub(visible);
        // 用户之前滚动到了会话记录的新结尾之后。
        let stale = TranscriptScroll::at_line(20);
        let new_state = stale.scrolled_by(-1, &meta, visible);
        // 要么最终滚动到底部附近（max_start - 1），要么
        // 如果 max_start 为 0，已经在尾部。
        if meta.len() > visible {
            // 应该在 max_start - 1 = 3。
            assert_eq!(new_state, TranscriptScroll::at_line(max_start - 1));
        }
    }

    /// 当会话记录完全适合视口时，scrolled_by 总是折叠到尾部。
    #[test]
    fn scrolled_by_collapses_to_bottom_when_view_fits() {
        let meta = synth_line_meta(2, 2);
        let visible = meta.len() + 5;
        let state = TranscriptScroll::at_line(0);
        let new_state = state.scrolled_by(-1, &meta, visible);
        assert!(new_state.is_at_tail());
    }

    /// 从尾部向下滚动保持正数增量在尾部（我们不能滚动过底部）。
    #[test]
    fn scrolled_by_from_tail_down_stays_at_tail() {
        let meta = synth_line_meta(5, 3);
        let visible = 6;
        let state = TranscriptScroll::to_bottom();
        let new_state = state.scrolled_by(5, &meta, visible);
        assert!(new_state.is_at_tail());
    }

    /// 从尾部向上滚动负数增量从 `max_start` 后退 |delta|。
    #[test]
    fn scrolled_by_from_tail_up_walks_back_from_max_start() {
        let meta = synth_line_meta(5, 3); // 19 个条目
        let visible = 6;
        let max_start = meta.len().saturating_sub(visible);
        let state = TranscriptScroll::to_bottom();
        let new_state = state.scrolled_by(-3, &meta, visible);
        assert_eq!(new_state, TranscriptScroll::at_line(max_start - 3));
    }

    /// `anchor_for` 将请求的起始值钳制到元数据范围并产生固定状态。
    #[test]
    fn anchor_for_clamps_start_into_range() {
        let meta = synth_line_meta(4, 1);
        let anchor = TranscriptScroll::anchor_for(&meta, 0).expect("非空");
        assert_eq!(anchor, TranscriptScroll::at_line(0));

        let anchor = TranscriptScroll::anchor_for(&meta, 1_000_000).expect("非空");
        assert_eq!(
            anchor,
            TranscriptScroll::at_line(meta.len().saturating_sub(1))
        );
    }

    /// 空的 `line_meta` 返回 `None`，以便调用者可以回退到
    /// [`TranscriptScroll::to_bottom`]。
    #[test]
    fn anchor_for_empty_returns_none() {
        let meta: Vec<TranscriptLineMeta> = Vec::new();
        assert!(TranscriptScroll::anchor_for(&meta, 0).is_none());
    }

    /// 尾部状态解析为 `max_start`，无论 `line_meta` 内容如何。
    #[test]
    fn to_bottom_resolves_to_max_start() {
        let meta = synth_line_meta(5, 2);
        let max_start = 7;
        let (state, top) = TranscriptScroll::to_bottom().resolve_top(&meta, max_start);
        assert!(state.is_at_tail());
        assert_eq!(top, max_start);
    }

    #[test]
    fn mouse_scroll_single_wheel_tick_moves_three_lines() {
        let mut state = MouseScrollState::new();
        let start = Instant::now();

        assert_eq!(
            state.on_scroll_at(ScrollDirection::Down, start).delta_lines,
            3
        );
        assert_eq!(
            state.on_scroll_at(ScrollDirection::Up, start).delta_lines,
            -1,
            "相同时间戳被视为快速精确输入"
        );
    }

    #[test]
    fn mouse_scroll_rapid_same_direction_accelerates_but_caps() {
        let mut state = MouseScrollState::new();
        let start = Instant::now();

        let deltas = [
            state.on_scroll_at(ScrollDirection::Down, start).delta_lines,
            state
                .on_scroll_at(ScrollDirection::Down, start + Duration::from_millis(10))
                .delta_lines,
            state
                .on_scroll_at(ScrollDirection::Down, start + Duration::from_millis(20))
                .delta_lines,
            state
                .on_scroll_at(ScrollDirection::Down, start + Duration::from_millis(30))
                .delta_lines,
            state
                .on_scroll_at(ScrollDirection::Down, start + Duration::from_millis(40))
                .delta_lines,
            state
                .on_scroll_at(ScrollDirection::Down, start + Duration::from_millis(50))
                .delta_lines,
            state
                .on_scroll_at(ScrollDirection::Down, start + Duration::from_millis(60))
                .delta_lines,
            state
                .on_scroll_at(ScrollDirection::Down, start + Duration::from_millis(70))
                .delta_lines,
        ];

        assert_eq!(deltas, [3, 1, 1, 2, 2, 2, 3, 3]);
    }

    #[test]
    fn mouse_scroll_direction_change_resets_acceleration() {
        let mut state = MouseScrollState::new();
        let start = Instant::now();

        for step in 0..8 {
            let _ = state.on_scroll_at(
                ScrollDirection::Down,
                start + Duration::from_millis(step * 10),
            );
        }

        assert_eq!(
            state
                .on_scroll_at(ScrollDirection::Up, start + Duration::from_millis(90))
                .delta_lines,
            -1
        );
    }

    #[test]
    fn mouse_scroll_slow_gap_resets_to_wheel_tick() {
        let mut state = MouseScrollState::new();
        let start = Instant::now();

        assert_eq!(
            state.on_scroll_at(ScrollDirection::Down, start).delta_lines,
            3
        );
        assert_eq!(
            state
                .on_scroll_at(ScrollDirection::Down, start + Duration::from_millis(100))
                .delta_lines,
            3
        );
    }
}
