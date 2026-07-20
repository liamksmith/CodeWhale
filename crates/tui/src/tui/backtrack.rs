//! Esc-Esc 回溯状态机（issue #133）。
//!
//! 让用户将当前对话回退到之前的用户消息。
//! 和弦有意设计为两步，这样单个意外的 `Esc` 在关闭弹出窗口后
//! 不会意外地回退一个回合：
//!
//! 1. **第一次 Esc**（没有弹出窗口、没有流式传输、没有要清除的内容）——将
//!    `Inactive` 转换为 `Primed`。编辑器显示一个临时提示
//!    （"再次按 Esc 来回退"）。在预备窗口内的第二次 Esc
//!    会打开覆盖层。任何其他按键路径可以在之后取消
//!    预备状态。
//! 2. **第二次 Esc**——将 `Primed` 转换为 `Selecting { selected_idx: 0 }`。
//!    实时记录覆盖层打开，最近的一条用户消息
//!    高亮显示。左/右键浏览先前的用户消息。
//! 3. **Enter**——确认选择：产生所选的 `selected_idx`
//!    （从尾部开始的深度偏移，其中 `0` = 最新的用户回合）。将
//!    状态机重置为 `Inactive`。调用者然后分叉线程，用
//!    回退的文本填充编辑器，并修剪记录。
//!
//! 状态机对应用程序的其他部分一无所知——它只存储
//!    选择正确用户回合所需的小型记账信息。UI
//!    路由（弹出窗口检测、流式传输保护、分叉副作用）位于
//! `tui::ui` 中。

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BacktrackPhase {
    /// 没有正在进行的预备状态；Esc 行为正常。
    #[default]
    Inactive,
    /// 第一次 Esc 已捕获。下一次 Esc 转换为 `Selecting`；任何
    /// 其他 Esc 等效的取消操作回到 `Inactive`。
    Primed,
    /// 覆盖层已打开。`selected_idx` 是高亮用户消息
    /// 从尾部开始的深度（`0` = 最新）。`total` 是
    /// 可浏览的用户消息数量，在进入时捕获，
    /// 以便即使记录在覆盖层下发生变化，边界检查也保持稳定
    /// （它确实会变，因为引擎从不暂停）。
    Selecting { selected_idx: usize, total: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// 向更早的用户消息移动（增加 `selected_idx`）。
    Left,
    /// 向更新的用户消息移动（减少 `selected_idx`）。
    Right,
}

/// 调用者应对单次 `Esc` 按键做出响应的操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscEffect {
    /// 不回退操作——调用者应运行其正常的 Esc 路径。
    None,
    /// 从 `Inactive` 移动到 `Primed`。调用者应显示
    /// 临时预备提示。
    Prime,
    /// 取消预备状态而不进入 Selecting。调用者应
    /// 清除预备提示。
    Cancel,
    /// 打开回溯覆盖层（我们转换 `Primed` → `Selecting`）。
    /// 调用者应以 `BacktrackPreview` 模式推送
    /// 实时记录覆盖层。
    OpenOverlay,
}

/// 挂在 `App` 上的小型记账结构体。只拥有状态机——
/// 没有记录快照，没有 UI 句柄。调用者负责在进入
/// `Selecting` 时告诉状态机有多少用户消息，
/// 这避免了将此模块绑定到任何特定的
/// 记录表示形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BacktrackState {
    pub phase: BacktrackPhase,
}

impl BacktrackState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: BacktrackPhase::Inactive,
        }
    }

    /// 当用户已武装或打开回溯时为 `true`。UI 使用
    /// 此值在覆盖层已打开时跳过预备提示，并知道
    /// 方向键是否应驱动选择。
    #[allow(dead_code)] // 为未来 UI 消费者和测试暴露的辅助方法。
    #[must_use]
    pub fn is_active(&self) -> bool {
        !matches!(self.phase, BacktrackPhase::Inactive)
    }

    /// 仅当覆盖层打开且左/右键应浏览
    /// 先前的用户消息时为 `true`。`Primed` 被有意排除——
    /// 在预备窗口期间箭头仍然滚动记录。
    #[allow(dead_code)] // 为未来 UI 消费者和测试暴露的辅助方法。
    #[must_use]
    pub fn is_selecting(&self) -> bool {
        matches!(self.phase, BacktrackPhase::Selecting { .. })
    }

    /// 当前从尾部开始的深度偏移（如果有）。方便那些
    /// 需要高亮索引而不匹配枚举的渲染器。
    #[must_use]
    pub fn selected_idx(&self) -> Option<usize> {
        match self.phase {
            BacktrackPhase::Selecting { selected_idx, .. } => Some(selected_idx),
            _ => None,
        }
    }

    /// 处理 Esc 按键。
    ///
    /// `total_user_messages` 是当前实时记录中的用户回合数。
    /// 仅在 `Primed` → `Selecting` 转换时使用；
    /// 值为 `0` 时短路并取消预备状态
    ///（没有可回溯的内容）。
    pub fn handle_esc(&mut self, total_user_messages: usize) -> EscEffect {
        match self.phase {
            BacktrackPhase::Inactive => {
                if total_user_messages == 0 {
                    // 没有可回溯的内容——甚至不预备。
                    return EscEffect::None;
                }
                self.phase = BacktrackPhase::Primed;
                EscEffect::Prime
            }
            BacktrackPhase::Primed => {
                if total_user_messages == 0 {
                    self.phase = BacktrackPhase::Inactive;
                    return EscEffect::Cancel;
                }
                self.phase = BacktrackPhase::Selecting {
                    selected_idx: 0,
                    total: total_user_messages,
                };
                EscEffect::OpenOverlay
            }
            BacktrackPhase::Selecting { .. } => {
                // Selecting 期间的 Esc 通过模态框自身的手柄关闭覆盖层；
                // 它不应该被路由回这里。通过取消来防御
                // 意外的路由。
                self.phase = BacktrackPhase::Inactive;
                EscEffect::Cancel
            }
        }
    }

    /// 在 `Selecting` 状态下步进选择。在任何其他阶段无操作。
    /// `Left` 向后走（更早），`Right` 向前走（更新）。
    /// 边界检查：`selected_idx` 被限制在 `[0, total - 1]`。
    pub fn step(&mut self, dir: Direction) {
        if let BacktrackPhase::Selecting {
            selected_idx,
            total,
        } = self.phase
        {
            if total == 0 {
                return;
            }
            let last = total.saturating_sub(1);
            let new_idx = match dir {
                Direction::Left => selected_idx.saturating_add(1).min(last),
                Direction::Right => selected_idx.saturating_sub(1),
            };
            self.phase = BacktrackPhase::Selecting {
                selected_idx: new_idx,
                total,
            };
        }
    }

    /// 确认当前选择。成功后返回从尾部开始的深度偏移
    ///（0 = 最新的用户回合）并重置为 `Inactive`。
    /// 如果当前不在 selecting 状态则返回 `None`——调用者应将其
    /// 视为无操作。
    pub fn confirm(&mut self) -> Option<usize> {
        match self.phase {
            BacktrackPhase::Selecting { selected_idx, .. } => {
                self.phase = BacktrackPhase::Inactive;
                Some(selected_idx)
            }
            _ => None,
        }
    }

    /// 强制状态机回到 `Inactive`。当弹出窗口
    /// 抢走焦点、开始流式传输、覆盖层在未确认的情况下关闭
    /// 以及在 `Primed` 期间收到任何非箭头/非 Enter 键时，由 UI 使用。
    pub fn reset(&mut self) {
        self.phase = BacktrackPhase::Inactive;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_inactive() {
        let s = BacktrackState::new();
        assert!(!s.is_active());
        assert!(!s.is_selecting());
        assert_eq!(s.selected_idx(), None);
    }

    #[test]
    fn first_esc_primes() {
        let mut s = BacktrackState::new();
        let effect = s.handle_esc(3);
        assert_eq!(effect, EscEffect::Prime);
        assert!(matches!(s.phase, BacktrackPhase::Primed));
        assert!(s.is_active());
        assert!(!s.is_selecting());
    }

    #[test]
    fn first_esc_with_no_user_messages_is_noop() {
        let mut s = BacktrackState::new();
        let effect = s.handle_esc(0);
        assert_eq!(effect, EscEffect::None);
        assert!(matches!(s.phase, BacktrackPhase::Inactive));
    }

    #[test]
    fn double_esc_enters_selecting() {
        let mut s = BacktrackState::new();
        assert_eq!(s.handle_esc(5), EscEffect::Prime);
        let effect = s.handle_esc(5);
        assert_eq!(effect, EscEffect::OpenOverlay);
        assert_eq!(
            s.phase,
            BacktrackPhase::Selecting {
                selected_idx: 0,
                total: 5,
            }
        );
        assert!(s.is_selecting());
    }

    #[test]
    fn primed_with_zero_messages_cancels() {
        // 如果在第一次和第二次 Esc 之间记录变空（例如
        // /clear 在另一个路径中运行），第二次 Esc 必须取消
        // 而不是打开一个空的覆盖层。
        let mut s = BacktrackState::new();
        s.phase = BacktrackPhase::Primed;
        let effect = s.handle_esc(0);
        assert_eq!(effect, EscEffect::Cancel);
        assert!(matches!(s.phase, BacktrackPhase::Inactive));
    }

    #[test]
    fn step_left_walks_back_in_time() {
        let mut s = BacktrackState::new();
        s.phase = BacktrackPhase::Selecting {
            selected_idx: 0,
            total: 3,
        };
        s.step(Direction::Left);
        assert_eq!(s.selected_idx(), Some(1));
        s.step(Direction::Left);
        assert_eq!(s.selected_idx(), Some(2));
        // 边界：不能超过 `total - 1`。
        s.step(Direction::Left);
        assert_eq!(s.selected_idx(), Some(2));
    }

    #[test]
    fn step_right_walks_forward_in_time() {
        let mut s = BacktrackState::new();
        s.phase = BacktrackPhase::Selecting {
            selected_idx: 2,
            total: 3,
        };
        s.step(Direction::Right);
        assert_eq!(s.selected_idx(), Some(1));
        s.step(Direction::Right);
        assert_eq!(s.selected_idx(), Some(0));
        // 边界：saturating_sub 将下限保持在 0。
        s.step(Direction::Right);
        assert_eq!(s.selected_idx(), Some(0));
    }

    #[test]
    fn step_in_inactive_or_primed_is_noop() {
        let mut s = BacktrackState::new();
        s.step(Direction::Left);
        assert!(matches!(s.phase, BacktrackPhase::Inactive));
        s.phase = BacktrackPhase::Primed;
        s.step(Direction::Right);
        assert!(matches!(s.phase, BacktrackPhase::Primed));
    }

    #[test]
    fn step_with_total_one_clamps_at_zero() {
        let mut s = BacktrackState::new();
        s.phase = BacktrackPhase::Selecting {
            selected_idx: 0,
            total: 1,
        };
        s.step(Direction::Left);
        assert_eq!(s.selected_idx(), Some(0));
        s.step(Direction::Right);
        assert_eq!(s.selected_idx(), Some(0));
    }

    #[test]
    fn confirm_yields_index_and_resets() {
        let mut s = BacktrackState::new();
        s.phase = BacktrackPhase::Selecting {
            selected_idx: 2,
            total: 5,
        };
        let idx = s.confirm();
        assert_eq!(idx, Some(2));
        assert!(matches!(s.phase, BacktrackPhase::Inactive));
    }

    #[test]
    fn confirm_outside_selecting_returns_none() {
        let mut s = BacktrackState::new();
        assert_eq!(s.confirm(), None);
        s.phase = BacktrackPhase::Primed;
        assert_eq!(s.confirm(), None);
        assert!(matches!(s.phase, BacktrackPhase::Primed));
    }

    #[test]
    fn reset_returns_to_inactive_from_any_phase() {
        let mut s = BacktrackState::new();
        s.phase = BacktrackPhase::Primed;
        s.reset();
        assert!(matches!(s.phase, BacktrackPhase::Inactive));

        s.phase = BacktrackPhase::Selecting {
            selected_idx: 1,
            total: 3,
        };
        s.reset();
        assert!(matches!(s.phase, BacktrackPhase::Inactive));
    }

    #[test]
    fn esc_during_selecting_resets_defensively() {
        // 在已经在 selecting 时将 Esc 路由通过状态机
        // 不应进入第四种状态——它会取消。覆盖层自己的
        // Esc 处理器是规范的关闭路径，但我们防御
        // 错误路由的调用点。
        let mut s = BacktrackState::new();
        s.phase = BacktrackPhase::Selecting {
            selected_idx: 1,
            total: 3,
        };
        let effect = s.handle_esc(3);
        assert_eq!(effect, EscEffect::Cancel);
        assert!(matches!(s.phase, BacktrackPhase::Inactive));
    }

    #[test]
    fn primed_then_step_then_second_esc_reaches_selecting() {
        // 在 Primed 状态下到达的步骤应该对阶段无操作，因此
        // 后续的 Esc 仍然完成和弦。（实际中这
        // 对于用户在预备提示可见时按下
        // 方向键的情况很重要。）
        let mut s = BacktrackState::new();
        assert_eq!(s.handle_esc(2), EscEffect::Prime);
        s.step(Direction::Left); // 无操作
        assert!(matches!(s.phase, BacktrackPhase::Primed));
        assert_eq!(s.handle_esc(2), EscEffect::OpenOverlay);
        assert_eq!(s.selected_idx(), Some(0));
    }

    #[test]
    fn full_walk_then_confirm_returns_chosen_index() {
        let mut s = BacktrackState::new();
        assert_eq!(s.handle_esc(4), EscEffect::Prime);
        assert_eq!(s.handle_esc(4), EscEffect::OpenOverlay);
        s.step(Direction::Left); // 0 -> 1
        s.step(Direction::Left); // 1 -> 2
        assert_eq!(s.confirm(), Some(2));
        assert!(matches!(s.phase, BacktrackPhase::Inactive));
    }
}
