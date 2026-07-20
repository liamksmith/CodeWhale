//! 活跃的进行中工具/执行单元格——单个可变组，缓冲当前回合的并行工具工作。
//!
//! ## 原因
//!
//! 当模型在单个助手回合中发出并行工具调用时（例如
//! 两个 `read_file` 和一个 `grep_files` 并发运行），
//! 简单地将每个工具开始追加为自己的历史单元格会使记录
//! 在完成结果乱序到达时"跳动"。Codex 的模式是将所有
//! 进行中的工具工作保持在一个可变的活动单元格中；一旦回合
//! 解析完成，活动单元格就定稿到记录中。
//!
//! ## 契约
//!
//! - 每回合最多一个 [`ActiveCell`]。它持有零个或多个
//!   仍在变异的 [`HistoryCell`]（状态 `Running`、输出待定等）。
//! - 持有者 [`crate::tui::app::App`] 在 `App.history` 之后渲染活动单元格的内容，
//!   因此它们出现在实时尾部。
//! - 像 `tool_cells` / `tool_details_by_cell` 等辅助函数使用的单元格索引
//!   指向虚拟序列 `App.history ++ active_cell.entries`。每个
//!   条目的索引是 `App.history.len() + entry_offset`。
//! - 当工具完成但其 `tool_id` 不匹配任何活动条目时（孤儿），
//!   调用方将已完成的独立单元格推送到 `App.history`，
//!   而不是修改活动组。这使 `active_cell` 保持对实际启动内容的稳定反映，
//!   并避免合并不相关的工具工作。
//! - 在 `TurnComplete`（或取消）时，活动单元格被"刷新"：
//!   进行中的条目被标记为提供的终端状态，然后
//!   每个条目都被追加到 `App.history`。伴随映射
//!   （`tool_cells`、`tool_details_by_cell`）被重写以指向新的
//!   `App.history` 索引。
//!
//! ## 修订计数器
//!
//! 活动组内的单元格在变异时不会更改指针标识，
//! 因此记录缓存不能依赖枚举相等性进行失效检测。我们
//! 公开 `revision()` 和 `bump_revision()`；渲染器在计算
//! 每个单元格的缓存修订时将这与 `App.history_version` 结合。

use crate::tui::history::{ExploringCell, ExploringEntry, HistoryCell, ToolCell, ToolStatus};

/// 进行中的活动单元格：一组可变的 [`HistoryCell`] 条目序列。
///
/// 概念上是一个 Codex 意义上的单个"实时尾部"单元格：它作为
/// 记录末尾的一个逻辑块出现，但内部由一个或多个条目组成
///（每个条目渲染为其自己的 [`HistoryCell`]）。
/// 我们保持它们为单独条目的原因——而不是融合为单个概念块——
/// 是它们可能具有不同的形状（`ExecCell`、`ExploringCell` 聚合、
/// MCP 工具结果等），且现有的渲染器已经知道如何正确绘制每种形状。
/// 合并为单个渲染路径会重复我们已经拥有的逻辑。
#[derive(Debug, Clone, Default)]
pub struct ActiveCell {
    entries: Vec<HistoryCell>,
    /// 当前与此活动单元格关联的工具 ID。映射值是指向
    /// [`Self::entries`] 的索引。多个工具 ID 可以映射到同一个
    /// 条目（现有的 `ExploringCell` 将多个读取聚合到单个条目中）。
    tool_to_entry: std::collections::HashMap<String, usize>,
    /// 当前 `ExploringCell` 条目的索引（如果存在），以便额外的
    /// 探索工具启动追加到它而不是创建新单元格。
    exploring_entry: Option<usize>,
    /// 每次变异时递增。用于告诉记录缓存活动单元格需要重新渲染，
    /// 即使它在虚拟单元格列表中的位置没有变化。
    revision: u64,
}

impl ActiveCell {
    /// 创建一个空的活动单元格。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 条目数量（每个条目渲染为其自己的 [`HistoryCell`]）。
    #[must_use]
    #[allow(dead_code)] // Public surface used by tests and future renderers.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// 活动单元格是否包含任何条目。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 对底层条目的只读访问（用于渲染）。
    #[must_use]
    pub fn entries(&self) -> &[HistoryCell] {
        &self.entries
    }

    /// 对特定条目的可变访问。递增修订计数器，以便
    /// 渲染器知道缓存的行已过时。
    pub fn entry_mut(&mut self, index: usize) -> Option<&mut HistoryCell> {
        if index < self.entries.len() {
            self.bump_revision();
            self.entries.get_mut(index)
        } else {
            None
        }
    }

    /// 当前修订计数器。溢出时回绕，这对缓存失效没问题；
    /// 在单个会话中回绕碰撞的概率是天文学级别的，
    /// 任何误判只导致一次额外的重新渲染。
    #[must_use]
    #[allow(dead_code)] // Used by App::bump_active_cell_revision and future cache wiring.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 递增修订计数器。在条目发生变异时调用。
    pub fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// 向活动单元格添加工具条目。
    ///
    /// 返回条目索引（调用方可记录在 `tool_cells_in_active` 中）。
    /// 如果单元格是探索工具启动且活动组中已存在探索条目，
    /// 则该条目被追加到该聚合中，而不是创建新条目。
    ///
    /// 为新的（或更新的）条目注册 `tool_id`，以便将来的完成查找可以找到它。
    pub fn push_tool(&mut self, tool_id: impl Into<String>, cell: HistoryCell) -> usize {
        let tool_id = tool_id.into();
        // 如果这是探索启动且我们已有探索条目，
        // 则追加到该条目而不是创建新单元格。
        if let HistoryCell::Tool(ToolCell::Exploring(new_cell)) = &cell
            && let Some(entry_idx) = self.exploring_entry
            && let Some(HistoryCell::Tool(ToolCell::Exploring(existing))) =
                self.entries.get_mut(entry_idx)
        {
            // 调用方给我们一个全新的 ExploringCell，其中有一个条目。
            // 将该条目移动到现有的聚合中。
            for explore_entry in &new_cell.entries {
                let _ = existing.insert_entry(explore_entry.clone());
            }
            self.tool_to_entry.insert(tool_id, entry_idx);
            self.bump_revision();
            return entry_idx;
        }

        // 否则，推送一个新条目。
        let entry_idx = self.entries.len();
        if matches!(cell, HistoryCell::Tool(ToolCell::Exploring(_))) {
            self.exploring_entry = Some(entry_idx);
        }
        self.entries.push(cell);
        self.tool_to_entry.insert(tool_id, entry_idx);
        self.bump_revision();
        entry_idx
    }

    /// 推送没有工具 ID 绑定的条目（如果需要，用于非工具分组）。
    /// 当前未使用；为与 Codex 对称而保留，Codex 允许
    /// 例如会话头部单元格存在于 `active_cell` 中。
    #[allow(dead_code)]
    pub fn push_untracked(&mut self, cell: HistoryCell) -> usize {
        let entry_idx = self.entries.len();
        self.entries.push(cell);
        self.bump_revision();
        entry_idx
    }

    /// 推送一个思考条目作为新的活动单元格条目。类似于
    /// [`Self::push_tool`] 但针对 `HistoryCell::Thinking` 内容。
    /// 返回条目索引。思考条目不参与 `tool_to_entry` 或
    /// 探索聚合——每个思考块独立存在。
    ///
    /// P2.3：思考存在于活动单元格中，因此 `Thinking → Tool → Tool`
    /// 序列渲染为一个逻辑"Working…"块，直到下一个
    /// 助手散文块将组刷新到历史中。
    pub fn push_thinking(&mut self, cell: HistoryCell) -> usize {
        debug_assert!(
            matches!(cell, HistoryCell::Thinking { .. }),
            "push_thinking expects HistoryCell::Thinking",
        );
        let entry_idx = self.entries.len();
        self.entries.push(cell);
        self.bump_revision();
        entry_idx
    }

    /// 查找持有给定工具 ID 的条目索引。
    #[must_use]
    #[allow(dead_code)] // Reserved for the Codex-style "exec end target" lookup.
    pub fn entry_index_for_tool(&self, tool_id: &str) -> Option<usize> {
        self.tool_to_entry.get(tool_id).copied()
    }

    /// 将 [`ExploringEntry`] 追加到现有的探索聚合中（如果有），
    /// 将提供的工具 ID 绑定到它。成功时返回
    /// `(entry_index, entry_within_exploring)`。
    ///
    /// 当第二个探索工具在同一活动组中启动时使用：
    /// 我们扩展已存在的条目，而不是在活动组中分配另一个 ExploringCell 条目。
    pub fn append_to_exploring(
        &mut self,
        tool_id: impl Into<String>,
        explore_entry: ExploringEntry,
    ) -> Option<(usize, usize)> {
        let entry_idx = self.exploring_entry?;
        let HistoryCell::Tool(ToolCell::Exploring(cell)) = self.entries.get_mut(entry_idx)? else {
            return None;
        };
        let inner_idx = cell.insert_entry(explore_entry);
        self.tool_to_entry.insert(tool_id.into(), entry_idx);
        self.bump_revision();
        Some((entry_idx, inner_idx))
    }

    /// 确保活动组中存在 [`ExploringCell`]；如果不存在则创建它。返回其条目索引。
    pub fn ensure_exploring(&mut self) -> usize {
        if let Some(idx) = self.exploring_entry {
            return idx;
        }
        let idx = self.entries.len();
        self.entries
            .push(HistoryCell::Tool(ToolCell::Exploring(ExploringCell {
                entries: Vec::new(),
            })));
        self.exploring_entry = Some(idx);
        self.bump_revision();
        idx
    }

    /// 移除条目的工具 ID 绑定而不移除条目本身
    ///（条目保留在活动组中，可能其状态已更新）。
    #[allow(dead_code)] // Reserved for cancellation paths that prune ids without flushing.
    pub fn forget_tool(&mut self, tool_id: &str) -> Option<usize> {
        self.tool_to_entry.remove(tool_id)
    }

    /// 排空每个条目，按插入顺序返回。重置内部
    /// 状态（通过 `bump_revision` 递增修订）。
    ///
    /// 调用方在 `TurnComplete`（或取消）时使用此方法将活动组刷新到 `App.history`。
    pub fn drain(&mut self) -> Vec<HistoryCell> {
        let entries = std::mem::take(&mut self.entries);
        self.tool_to_entry.clear();
        self.exploring_entry = None;
        self.bump_revision();
        entries
    }

    /// 将每个仍在运行的条目标记为 `Failed`（在回合中途取消时使用）。
    /// 已经完成的条目保持不变。
    ///
    /// `Failed` 是最接近"已中断"的现有变体；单元格的
    /// 周围上下文（回合状态横幅）告诉用户这是取消而非工具错误。
    pub fn mark_in_progress_as_interrupted(&mut self) {
        for cell in &mut self.entries {
            mark_running_as_interrupted(cell);
        }
        self.bump_revision();
    }
}

fn mark_running_as_interrupted(cell: &mut HistoryCell) {
    if let HistoryCell::Thinking {
        streaming,
        duration_secs,
        ..
    } = cell
    {
        // 卡在流中间的思考单元格应在回合取消时停止旋转。
        // 如果 `duration_secs` 已填充则保持不变；
        // 否则渲染器简单地省略时长徽章。
        *streaming = false;
        let _ = duration_secs;
        return;
    }
    let HistoryCell::Tool(tool_cell) = cell else {
        return;
    };
    match tool_cell {
        ToolCell::Exec(exec) if exec.status == ToolStatus::Running => {
            exec.status = ToolStatus::Failed;
        }
        ToolCell::Exploring(explore) => {
            for entry in &mut explore.entries {
                if entry.status == ToolStatus::Running {
                    entry.status = ToolStatus::Failed;
                }
            }
        }
        ToolCell::PlanUpdate(plan) if plan.status == ToolStatus::Running => {
            plan.status = ToolStatus::Failed;
        }
        ToolCell::PatchSummary(patch) if patch.status == ToolStatus::Running => {
            patch.status = ToolStatus::Failed;
        }
        ToolCell::Review(review) if review.status == ToolStatus::Running => {
            review.status = ToolStatus::Failed;
        }
        ToolCell::Mcp(mcp) if mcp.status == ToolStatus::Running => {
            mcp.status = ToolStatus::Failed;
        }
        ToolCell::WebSearch(search) if search.status == ToolStatus::Running => {
            search.status = ToolStatus::Failed;
        }
        ToolCell::Generic(generic) if generic.status == ToolStatus::Running => {
            generic.status = ToolStatus::Failed;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::history::{
        ExecCell, ExecSource, ExploringCell, ExploringEntry, GenericToolCell,
    };
    use std::time::Instant;

    fn exec_cell(command: &str) -> HistoryCell {
        HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: command.to_string(),
            status: ToolStatus::Running,
            output: None,
            live_output: None,
            shell_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            started_at: Some(Instant::now()),
            duration_ms: None,
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        }))
    }

    fn exploring_cell_with(label: &str) -> HistoryCell {
        HistoryCell::Tool(ToolCell::Exploring(ExploringCell {
            entries: vec![ExploringEntry {
                label: label.to_string(),
                status: ToolStatus::Running,
            }],
        }))
    }

    fn generic_cell(name: &str) -> HistoryCell {
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: name.to_string(),
            status: ToolStatus::Running,
            input_summary: None,
            output: None,
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        }))
    }

    #[test]
    fn push_tool_records_entry_and_revision_advances() {
        let mut cell = ActiveCell::new();
        let r0 = cell.revision();
        let idx = cell.push_tool("t1", exec_cell("ls"));
        assert_eq!(idx, 0);
        assert_eq!(cell.entry_count(), 1);
        assert!(cell.revision() != r0);
        assert_eq!(cell.entry_index_for_tool("t1"), Some(0));
    }

    #[test]
    fn parallel_exploring_starts_share_one_entry() {
        let mut cell = ActiveCell::new();
        let idx_a = cell.push_tool("a", exploring_cell_with("Read foo.rs"));
        let idx_b = cell.push_tool("b", exploring_cell_with("Read bar.rs"));
        assert_eq!(
            idx_a, idx_b,
            "both exploring starts should land in same entry"
        );
        assert_eq!(cell.entry_count(), 1);
        let HistoryCell::Tool(ToolCell::Exploring(explore)) = &cell.entries()[0] else {
            panic!("expected exploring cell")
        };
        assert_eq!(explore.entries.len(), 2);
    }

    #[test]
    fn drain_resets_state_and_returns_in_order() {
        let mut cell = ActiveCell::new();
        cell.push_tool("a", exec_cell("ls"));
        cell.push_tool("b", generic_cell("foo"));
        let drained = cell.drain();
        assert_eq!(drained.len(), 2);
        assert!(cell.is_empty());
        assert_eq!(cell.entry_index_for_tool("a"), None);
    }

    #[test]
    fn interrupt_marks_running_entries_failed() {
        let mut cell = ActiveCell::new();
        cell.push_tool("a", exec_cell("ls"));
        cell.mark_in_progress_as_interrupted();
        let HistoryCell::Tool(ToolCell::Exec(exec)) = &cell.entries()[0] else {
            panic!("expected exec")
        };
        assert_eq!(exec.status, ToolStatus::Failed);
    }

    fn thinking_cell(content: &str, streaming: bool) -> HistoryCell {
        HistoryCell::Thinking {
            content: content.to_string(),
            streaming,
            duration_secs: None,
        }
    }

    #[test]
    fn push_thinking_records_entry_at_tail() {
        let mut cell = ActiveCell::new();
        let r0 = cell.revision();
        let idx = cell.push_thinking(thinking_cell("planning…", true));
        assert_eq!(idx, 0);
        assert_eq!(cell.entry_count(), 1);
        assert!(cell.revision() != r0);
    }

    #[test]
    fn thinking_then_tools_group_in_one_active_cell() {
        // P2.3：发出 Thinking → Tool → Tool 的回合将所有内容保留在
        // 一个活动单元格中，直到下一个散文块刷新组。
        let mut cell = ActiveCell::new();
        cell.push_thinking(thinking_cell("plan…", true));
        cell.push_tool("t-1", exec_cell("ls"));
        cell.push_tool("t-2", exploring_cell_with("Read foo.rs"));
        assert_eq!(
            cell.entry_count(),
            3,
            "thinking, exec, and exploring entries coexist in one active cell"
        );
        assert!(matches!(cell.entries()[0], HistoryCell::Thinking { .. }));
        assert!(matches!(
            cell.entries()[1],
            HistoryCell::Tool(ToolCell::Exec(_))
        ));
        assert!(matches!(
            cell.entries()[2],
            HistoryCell::Tool(ToolCell::Exploring(_))
        ));
    }

    #[test]
    fn drain_flushes_thinking_alongside_tools_in_order() {
        let mut cell = ActiveCell::new();
        cell.push_thinking(thinking_cell("plan…", false));
        cell.push_tool("t", exec_cell("ls"));
        let drained = cell.drain();
        assert_eq!(drained.len(), 2);
        assert!(matches!(drained[0], HistoryCell::Thinking { .. }));
        assert!(matches!(drained[1], HistoryCell::Tool(ToolCell::Exec(_))));
    }

    #[test]
    fn interrupt_stops_streaming_thinking_spinner() {
        let mut cell = ActiveCell::new();
        cell.push_thinking(thinking_cell("plan…", true));
        cell.mark_in_progress_as_interrupted();
        let HistoryCell::Thinking { streaming, .. } = &cell.entries()[0] else {
            panic!("expected thinking cell")
        };
        assert!(
            !*streaming,
            "interrupted thinking should stop streaming so the spinner exits"
        );
    }
}
