//! 活跃单元格的流式推理生命周期。
//!
//! DeepSeek V4 在最终答案之前发出 `reasoning_content` 块。
//! 这些在每个回合的活跃单元格内作为"思考"条目渲染。
//! 此模块是以下内容的唯一事实来源：
//!
//! - 在第一个块上创建流式思考条目
//! - 将块追加到实时条目
//! - 翻译进行中时显示本地化占位符
//!   （并动画化其已用时间/旋转器后缀）
//! - 翻译到达时替换占位符
//! - 思考块结束时最终确定条目（停止旋转器，标记持续时间）
//! - 将推理缓冲区存入 `app.last_reasoning`，以便摘要能在压缩后存活

use std::time::Duration;
use std::time::Instant;

use crate::tui::active_cell::ActiveCell;
use crate::tui::app::App;
use crate::tui::history::HistoryCell;

/// 思考块流式传输时活跃单元格修订版的防抖窗口（#1620）。
/// 推理增量到达速度远快于人眼能跟上的速度，
/// 每次修订版碰撞都会使活跃单元格的换行缓存失效，
/// 强制完全重新换行实时尾部。将中间碰撞合并为
/// 每窗口一次，使感知到的流保持流畅，而无需每个字符都重新换行。
/// ~100ms ≈ 每秒 10 次中间重绘，远低于 120 FPS
/// 帧率上限（参见 `frame_rate_limiter`），但无法感知为延迟。
///
/// 正确性：这仅跳过 *中间* 重绘。追加的内容
/// 永远不会被丢弃——它立即可达单元格——且 finalize 始终
/// 强制一次碰撞，以便最终的推理文本被完全渲染。
const THINKING_REVISION_THROTTLE: Duration = Duration::from_millis(100);

/// 为流式思考变异碰撞活跃单元格修订版，但最多
/// 每个 [`THINKING_REVISION_THROTTLE`] 窗口一次。返回是否
/// 实际发出了碰撞。跳过的碰撞合并到下一个
///（或强制 finalize 碰撞），因此没有内容会丢失——只有冗余的
/// 中间重新换行被丢弃。
fn bump_thinking_revision_throttled(app: &mut App, now: Instant) -> bool {
    let due = app
        .thinking_revision_last_bump_at
        .is_none_or(|last| now.saturating_duration_since(last) >= THINKING_REVISION_THROTTLE);
    if due {
        app.thinking_revision_last_bump_at = Some(now);
        app.bump_active_cell_revision();
    }
    due
}

/// 确保 `active_cell` 中存在进行中的思考条目并返回其
/// 条目索引。如果没有思考条目正在流式传输，则推送一个新的。
/// P2.3：思考与后续工具调用共享活跃单元格，以便
/// 两者作为一个逻辑 "工作中…" 块渲染。
pub(super) fn ensure_active_entry(app: &mut App) -> usize {
    if let Some(idx) = app.streaming_thinking_active_entry {
        return idx;
    }
    if app.active_cell.is_none() {
        app.active_cell = Some(ActiveCell::new());
    }
    let active = app.active_cell.as_mut().expect("active_cell just ensured");
    let entry_idx = active.push_thinking(HistoryCell::Thinking {
        content: String::new(),
        streaming: true,
        duration_secs: None,
    });
    app.streaming_thinking_active_entry = Some(entry_idx);
    app.bump_active_cell_revision();
    entry_idx
}

/// 将文本追加到 `active_cell` 内的流式思考条目。文本被
/// 立即提交到单元格；触发实时尾部重新换行的活跃单元格修订版碰撞
/// 被防抖，最多每 [`THINKING_REVISION_THROTTLE`] 窗口一次（#1620）。
/// 跳过的碰撞合并到下一次追加或强制 finalize 碰撞，
/// 因此没有内容会丢失。
pub(super) fn append(app: &mut App, entry_idx: usize, text: &str) {
    append_at(app, entry_idx, text, Instant::now());
}

/// 可注入时钟的 `append`，以便防抖可以确定性地测试。
fn append_at(app: &mut App, entry_idx: usize, text: &str, now: Instant) {
    if text.is_empty() {
        return;
    }
    let mutated = if let Some(active) = app.active_cell.as_mut()
        && let Some(HistoryCell::Thinking { content, .. }) = active.entry_mut(entry_idx)
    {
        content.push_str(text);
        true
    } else {
        false
    };
    if mutated {
        bump_thinking_revision_throttled(app, now);
    }
}

/// 构建翻译进行中时思考条目中显示的旋转器装饰占位符
///（`思考中… (1.2s |)`）。
pub(super) fn translation_placeholder_frame(app: &App) -> String {
    let base = crate::localization::thinking_translation_placeholder(app.ui_locale);
    let elapsed = app
        .thinking_started_at
        .or(app.turn_started_at)
        .map(|started| started.elapsed().as_secs_f32())
        .unwrap_or_default();
    let frame = match (elapsed.mul_add(2.0, 0.0) as usize) % 4 {
        0 => "|",
        1 => "/",
        2 => "-",
        _ => "\\",
    };
    format!("{base} ({elapsed:.1}s {frame})")
}

/// 如果给定条目为空或仍显示翻译占位符前缀，
/// 将其替换为最新的动画帧。
pub(super) fn set_placeholder(app: &mut App, entry_idx: usize) {
    let base = crate::localization::thinking_translation_placeholder(app.ui_locale);
    let next = translation_placeholder_frame(app);
    let mutated = if let Some(active) = app.active_cell.as_mut()
        && let Some(HistoryCell::Thinking { content, .. }) = active.entry_mut(entry_idx)
        && (content.is_empty() || content.starts_with(base))
    {
        if *content != next {
            *content = next;
            true
        } else {
            false
        }
    } else {
        false
    };
    if mutated {
        app.bump_active_cell_revision();
    }
}

/// 推进 `active_cell` 中每个现有翻译占位符上的旋转器后缀。
/// 当至少一个单元格被更新时返回 true，以便调度循环可以安排另一个 tick。
pub(super) fn animate_pending_translation(app: &mut App, translation_pending: bool) -> bool {
    if !app.translation_enabled {
        return false;
    }
    let thinking_streaming = app.streaming_thinking_active_entry.is_some();
    if !translation_pending && !thinking_streaming {
        return false;
    }
    let base = crate::localization::thinking_translation_placeholder(app.ui_locale);
    let next = translation_placeholder_frame(app);

    if let Some(active) = app.active_cell.as_mut() {
        for idx in (0..active.entry_count()).rev() {
            if let Some(HistoryCell::Thinking { content, .. }) = active.entry_mut(idx)
                && content.starts_with(base)
                && *content != next
            {
                *content = next.clone();
                app.bump_active_cell_revision();
                return true;
            }
        }
    }
    false
}

/// 用完成的翻译文本替换翻译占位符。
/// 首先搜索活跃单元格，然后搜索已定稿的历史记录（覆盖
/// 翻译在思考块已移入历史记录后到达的情况）。
pub(super) fn replace_pending_translation(
    app: &mut App,
    placeholder: &str,
    translated_text: String,
) {
    if let Some(active) = app.active_cell.as_mut() {
        for idx in (0..active.entry_count()).rev() {
            if let Some(HistoryCell::Thinking { content, .. }) = active.entry_mut(idx)
                && content.starts_with(placeholder)
            {
                *content = translated_text;
                app.bump_active_cell_revision();
                return;
            }
        }
    }

    for idx in (0..app.history.len()).rev() {
        if let Some(HistoryCell::Thinking { content, .. }) = app.history.get_mut(idx)
            && content.starts_with(placeholder)
        {
            *content = translated_text;
            app.bump_history_cell(idx);
            return;
        }
    }
}

/// 开始一个新的流式思考块。如果另一个思考块仍然活跃，
/// 首先排空其待处理的 UI 尾部，以便迟到的块边界不能
/// 丢弃 `StreamingState` 内缓冲的内容。
pub(super) fn start_block(app: &mut App) -> bool {
    let finalized_previous = if app.streaming_thinking_active_entry.is_some() {
        let finalized = finalize_current(app);
        stash_reasoning_buffer_into_last_reasoning(app);
        finalized
    } else {
        false
    };

    app.reasoning_buffer.clear();
    app.reasoning_header = None;
    app.thinking_started_at = Some(Instant::now());
    app.streaming_state.reset();
    app.streaming_state.start_thinking(0, None);
    let _ = ensure_active_entry(app);
    finalized_previous
}

/// 定稿当前流式思考条目：排空待处理的状态缓冲区，
/// 计算已用持续时间，停止旋转器。
pub(super) fn finalize_current(app: &mut App) -> bool {
    let duration = app
        .thinking_started_at
        .take()
        .map(|t| t.elapsed().as_secs_f32());
    let remaining = app.streaming_state.finalize_block_text(0);
    finalize_active_entry(app, duration, &remaining)
}

/// 将进行中的推理缓冲区移到 `app.last_reasoning` 上，以便
/// 摘要能在压缩或记录修剪后存活。
pub(super) fn stash_reasoning_buffer_into_last_reasoning(app: &mut App) {
    if app.reasoning_buffer.is_empty() {
        return;
    }

    if let Some(existing) = app.last_reasoning.as_mut()
        && !existing.is_empty()
    {
        if !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(&app.reasoning_buffer);
    } else {
        app.last_reasoning = Some(app.reasoning_buffer.clone());
    }
    app.reasoning_buffer.clear();
}

/// 定稿 `active_cell` 中进行中的思考条目：追加
/// 收集器的剩余缓冲文本，停止旋转器，并标记
/// 持续时间。当思考条目被定稿时返回 `true`（以便
/// 调度循环知道记录被修改）。如果没有思考条目
/// 正在流式传输，则为无操作。
pub(super) fn finalize_active_entry(app: &mut App, duration: Option<f32>, remaining: &str) -> bool {
    let Some(entry_idx) = app.streaming_thinking_active_entry.take() else {
        return false;
    };
    if !remaining.is_empty() {
        append(app, entry_idx, remaining);
    }
    if let Some(active) = app.active_cell.as_mut()
        && let Some(HistoryCell::Thinking {
            streaming,
            duration_secs,
            ..
        }) = active.entry_mut(entry_idx)
    {
        *streaming = false;
        *duration_secs = duration;
    }
    // 红线（#1620）：finalize 必须强制一次碰撞，以便最终的推理文本
    // 即使最后一个追加的块被节流也被完全渲染。重置
    // 防抖窗口，以便下一个思考块的第一个块
    // 立即渲染，而不是被合并到过时的窗口中。
    app.thinking_revision_last_bump_at = None;
    app.bump_active_cell_revision();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    fn test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: true,
            skip_onboarding: false,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    fn thinking_content(app: &App, entry_idx: usize) -> String {
        match app
            .active_cell
            .as_ref()
            .and_then(|active| active.entries().get(entry_idx))
        {
            Some(HistoryCell::Thinking { content, .. }) => content.clone(),
            other => panic!("expected a Thinking entry at {entry_idx}, got {other:?}"),
        }
    }

    /// #1620：一个节流窗口内的一连串推理块必须
    /// 合并为一次活跃单元格修订版碰撞（以便渲染器
    /// 每秒重新换行实时尾部约 10 次，而不是每个字符一次），同时
    /// 每个字节的内容都被保留，finalize 强制最终一次碰撞。
    #[test]
    fn issue_1620_throttles_thinking_bumps_without_losing_content() {
        let mut app = test_app();
        let entry = ensure_active_entry(&mut app);
        // `ensure_active_entry` 在创建时碰撞了一次；从干净的节流窗口
        // 开始测量，以便第一次追加立即渲染。
        app.thinking_revision_last_bump_at = None;
        let rev_before = app.active_cell_revision;

        let t0 = Instant::now();
        let chunks = [
            "Hel", "lo, ", "this", " is", " a", " lo", "ng", " re", "ason", "ing",
        ];
        // 所有十个块都位于一个 100ms 窗口内（每 5ms 一个）。
        for (i, chunk) in chunks.iter().enumerate() {
            append_at(
                &mut app,
                entry,
                chunk,
                t0 + Duration::from_millis(i as u64 * 5),
            );
        }
        assert_eq!(
            app.active_cell_revision.wrapping_sub(rev_before),
            1,
            "rapid chunks within one throttle window must coalesce to one bump"
        );

        // 窗口到期后的块允许再次碰撞。
        append_at(
            &mut app,
            entry,
            " stream",
            t0 + THINKING_REVISION_THROTTLE + Duration::from_millis(10),
        );
        assert_eq!(
            app.active_cell_revision.wrapping_sub(rev_before),
            2,
            "a chunk past the throttle window should bump once more"
        );

        // 尽管跳过了中间碰撞，没有内容被丢弃。
        let expected = format!("{} stream", chunks.concat());
        assert_eq!(thinking_content(&app, entry), expected);

        // 红线：finalize 强制恰好一次碰撞并刷新尾部。
        let rev_pre_final = app.active_cell_revision;
        let finalized = finalize_active_entry(&mut app, Some(1.5), " [end]");
        assert!(finalized, "finalize should report it finalized an entry");
        assert_eq!(
            app.active_cell_revision,
            rev_pre_final.wrapping_add(1),
            "finalize must always force exactly one revision bump"
        );
        assert_eq!(
            thinking_content(&app, entry),
            format!("{expected} [end]"),
            "finalize must not drop the trailing reasoning text"
        );
        assert!(
            app.thinking_revision_last_bump_at.is_none(),
            "finalize should reset the throttle window for the next block"
        );
    }
}
