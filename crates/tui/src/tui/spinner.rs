//! 运行状态 UI 装饰的共享动画帧。
//!
//! 将盲文旋转器放在一个地方，以便转录工具卡片、侧边栏和任何未来的正在运行的任务表面以相同的节奏推进。

use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// 用于运行中工具和后台作业的盲文"鲸鱼喷水"帧。
///
/// 该循环上升、达到顶峰、然后下降，而不是使用默认的顺时针点。
/// 在共享的重绘节奏下，它呈现为连续的水雾柱。
pub(crate) const BRAILLE_SPINNER_FRAMES: [&str; 12] = [
    "\u{2840}", "\u{2844}", "\u{2846}", "\u{28C6}", "\u{28E6}", "\u{28F6}", "\u{28F2}", "\u{28B2}",
    "\u{2832}", "\u{2830}", "\u{2820}", "\u{2810}",
];

/// 匹配实时 UI 重绘节奏，使运行中的字形在每个 tick 上推进。
pub(crate) const BRAILLE_SPINNER_FRAME_MS: u64 = 50;

#[must_use]
pub(crate) fn braille_spinner_frame_for_elapsed_ms(
    elapsed_ms: u128,
    low_motion: bool,
) -> &'static str {
    if low_motion {
        return BRAILLE_SPINNER_FRAMES[0];
    }
    let idx = elapsed_ms
        .checked_div(u128::from(BRAILLE_SPINNER_FRAME_MS))
        .map_or(0, |frame| frame % BRAILLE_SPINNER_FRAMES.len() as u128);
    BRAILLE_SPINNER_FRAMES[usize::try_from(idx).unwrap_or_default()]
}

#[must_use]
pub(crate) fn braille_spinner_frame_for_duration_ms(
    duration_ms: u64,
    low_motion: bool,
) -> &'static str {
    braille_spinner_frame_for_elapsed_ms(u128::from(duration_ms), low_motion)
}

#[must_use]
pub(crate) fn braille_spinner_frame(started_at: Option<Instant>, low_motion: bool) -> &'static str {
    let elapsed_ms = started_at.map_or_else(
        || {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis())
        },
        |started| started.elapsed().as_millis(),
    );
    braille_spinner_frame_for_elapsed_ms(elapsed_ms, low_motion)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braille_spinner_advances_at_shared_cadence() {
        // 针对帧表断言节奏行为，而非特定字形，以便可以在不破坏此处代码的情况下重新调整鲸鱼喷水模式。
        assert_eq!(
            braille_spinner_frame_for_elapsed_ms(0, false),
            BRAILLE_SPINNER_FRAMES[0]
        );
        assert_eq!(
            braille_spinner_frame_for_elapsed_ms(u128::from(BRAILLE_SPINNER_FRAME_MS) - 1, false),
            BRAILLE_SPINNER_FRAMES[0]
        );
        assert_eq!(
            braille_spinner_frame_for_elapsed_ms(u128::from(BRAILLE_SPINNER_FRAME_MS), false),
            BRAILLE_SPINNER_FRAMES[1]
        );
    }

    #[test]
    fn braille_spinner_respects_low_motion() {
        assert_eq!(
            braille_spinner_frame_for_elapsed_ms(u128::from(BRAILLE_SPINNER_FRAME_MS) * 3, true),
            BRAILLE_SPINNER_FRAMES[0]
        );
    }
}
