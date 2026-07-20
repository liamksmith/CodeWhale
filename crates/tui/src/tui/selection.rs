//! 记录视图的文本选择状态。

use std::time::Instant;

// === 类型 ===

/// 记录中的选择端点（行/列）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptSelectionPoint {
    pub line_index: usize,
    pub column: usize,
}

/// 记录视图中的当前选择状态。
#[derive(Debug, Clone, Copy, Default)]
pub struct TranscriptSelection {
    pub anchor: Option<TranscriptSelectionPoint>,
    pub head: Option<TranscriptSelectionPoint>,
    pub dragging: bool,
}

/// 拖拽超出边缘的自动滚动状态。当用户按住左键且光标位于记录矩形上方或下方时，
/// 主循环推进 `pending_scroll_delta` 并以固定节奏扩展选择头部，
/// 从而在一次拖拽中选择长段落（#1163）。
#[derive(Debug, Clone, Copy)]
pub struct SelectionAutoscroll {
    /// `-1` 向上滚动，`+1` 向下滚动。不会为 `0`。
    pub direction: i32,
    /// 上次在边界内的鼠标列，以绝对终端坐标表示。
    pub column: u16,
    /// 允许下一次 tick 触发的时间。
    pub next_tick: Instant,
}

impl TranscriptSelection {
    /// 清除任何活动的选择。
    pub fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.dragging = false;
    }

    /// 完整选择是否处于活动状态。
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.anchor.is_some() && self.head.is_some()
    }

    /// 返回从起点到终点排序的选择端点。
    #[must_use]
    pub fn ordered_endpoints(
        &self,
    ) -> Option<(TranscriptSelectionPoint, TranscriptSelectionPoint)> {
        let anchor = self.anchor?;
        let head = self.head?;
        if (head.line_index, head.column) < (anchor.line_index, anchor.column) {
            Some((head, anchor))
        } else {
            Some((anchor, head))
        }
    }
}
