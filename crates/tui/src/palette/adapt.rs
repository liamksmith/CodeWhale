//! 针对调色板模式、社区主题和终端深度的颜色适配。

use ratatui::style::Color;

use super::detect::PaletteMode;
use super::themes::{ThemeId, UiTheme};
use super::tokens::*;

#[must_use]
pub fn adapt_fg_for_palette_mode(color: Color, _bg: Color, mode: PaletteMode) -> Color {
    match mode {
        PaletteMode::Dark => color,
        PaletteMode::Light => adapt_fg_for_light_palette(color),
        PaletteMode::Grayscale => adapt_fg_for_grayscale_palette(color),
        PaletteMode::SolarizedLight => adapt_fg_for_solarized_light_palette(color),
    }
}

#[must_use]
pub fn adapt_bg_for_palette_mode(color: Color, mode: PaletteMode) -> Color {
    match mode {
        PaletteMode::Dark => color,
        PaletteMode::Light => adapt_bg_for_light_palette(color),
        PaletteMode::Grayscale => adapt_bg_for_grayscale_palette(color),
        PaletteMode::SolarizedLight => adapt_bg_for_solarized_light_palette(color),
    }
}

fn adapt_fg_for_light_palette(color: Color) -> Color {
    if color == TEXT_BODY || color == SELECTION_TEXT || color == Color::White {
        LIGHT_TEXT_BODY
    } else if color == TEXT_SECONDARY || color == TEXT_MUTED {
        LIGHT_TEXT_MUTED
    } else if color == TEXT_HINT || color == TEXT_DIM {
        LIGHT_TEXT_HINT
    } else if color == TEXT_SOFT || color == TEXT_TOOL_OUTPUT {
        LIGHT_TEXT_SOFT
    } else if color == BORDER_COLOR {
        LIGHT_BORDER
    } else if color == TEXT_ACCENT || color == WHALE_INFO || color == ACCENT_TOOL_LIVE {
        WHALE_ACCENT_PRIMARY
    } else if color == TEXT_REASONING || color == ACCENT_REASONING_LIVE {
        Color::Rgb(146, 64, 14)
    } else if color == ACCENT_TOOL_ISSUE {
        Color::Rgb(159, 18, 57)
    } else if color == DIFF_ADDED {
        Color::Rgb(22, 101, 52)
    } else if color == USER_BODY {
        LIGHT_USER_BODY
    } else {
        color
    }
}

fn adapt_bg_for_light_palette(color: Color) -> Color {
    if color == WHALE_BG || color == BACKGROUND_DARK {
        LIGHT_SURFACE
    } else if color == WHALE_PANEL
        || color == COMPOSER_BG
        || color == SURFACE_PANEL
        || color == SURFACE_TOOL
    {
        LIGHT_PANEL
    } else if color == SURFACE_ELEVATED || color == SURFACE_TOOL_ACTIVE {
        LIGHT_ELEVATED
    } else if color == SURFACE_REASONING
        || color == SURFACE_REASONING_TINT
        || color == SURFACE_REASONING_ACTIVE
    {
        LIGHT_REASONING
    } else if color == SURFACE_SUCCESS {
        LIGHT_SUCCESS
    } else if color == SURFACE_ERROR {
        LIGHT_ERROR
    } else if color == DIFF_ADDED_BG {
        LIGHT_SUCCESS
    } else if color == DIFF_DELETED_BG {
        LIGHT_ERROR
    } else if color == SELECTION_BG {
        LIGHT_SELECTION_BG
    } else {
        color
    }
}

fn adapt_fg_for_solarized_light_palette(color: Color) -> Color {
    if color == TEXT_BODY || color == SELECTION_TEXT || color == Color::White {
        SOLARIZED_TEXT_BODY
    } else if color == TEXT_SECONDARY || color == TEXT_MUTED {
        SOLARIZED_TEXT_MUTED
    } else if color == TEXT_HINT || color == TEXT_DIM {
        SOLARIZED_TEXT_HINT
    } else if color == TEXT_SOFT || color == TEXT_TOOL_OUTPUT {
        SOLARIZED_TEXT_SOFT
    } else if color == BORDER_COLOR {
        SOLARIZED_BORDER
    } else if color == TEXT_ACCENT || color == WHALE_INFO || color == ACCENT_TOOL_LIVE {
        SOLARIZED_BLUE
    } else if color == TEXT_REASONING || color == ACCENT_REASONING_LIVE {
        SOLARIZED_ORANGE
    } else if color == ACCENT_TOOL_ISSUE {
        SOLARIZED_RED
    } else if color == DIFF_ADDED || color == USER_BODY {
        SOLARIZED_GREEN
    } else {
        color
    }
}

fn adapt_bg_for_solarized_light_palette(color: Color) -> Color {
    if color == WHALE_BG || color == BACKGROUND_DARK {
        SOLARIZED_SURFACE
    } else if color == WHALE_PANEL
        || color == COMPOSER_BG
        || color == SURFACE_PANEL
        || color == SURFACE_TOOL
    {
        SOLARIZED_PANEL
    } else if color == SURFACE_ELEVATED || color == SURFACE_TOOL_ACTIVE {
        SOLARIZED_ELEVATED
    } else if color == SURFACE_REASONING
        || color == SURFACE_REASONING_TINT
        || color == SURFACE_REASONING_ACTIVE
    {
        SOLARIZED_PANEL
    } else if color == SURFACE_SUCCESS || color == DIFF_ADDED_BG {
        SOLARIZED_DIFF_ADDED_BG
    } else if color == SURFACE_ERROR {
        SOLARIZED_ERROR_SURFACE
    } else if color == DIFF_DELETED_BG {
        SOLARIZED_DIFF_DELETED_BG
    } else if color == SELECTION_BG {
        SOLARIZED_SELECT_BG
    } else {
        color
    }
}

// === 社区主题重映射 ===
//
// 此 crate 中的绝大多数渲染点直接使用 `palette::TEXT_*`、
// `palette::WHALE_BG`、`palette::BORDER_COLOR` 等，而不是
// 查找 `app.ui_theme`。为了使社区主题预设（Catppuccin、
// Tokyo Night……）在视觉上真正产生影响，我们在后端层截获颜色
//（参见 `tui::color_compat::ColorCompatBackend`）并将每个
// 已知暗调色板常量重映射到活跃预设的等效 UiTheme 槽位。
// 对于 `System`、`Whale` 和 `WhaleLight`，重映射是无操作的——
// 现有的暗/亮管线处理它们。

/// 每预设的绿色强调色，用于即使在主题化后也语义上*应*保持绿色的内容
///（diff "+" 行、用户输入正文）。现在委托给活跃 UiTheme 的 diff_added_fg。
#[must_use]
const fn theme_green(ui: &UiTheme) -> Color {
    ui.diff_added_fg
}

/// 每预设的红色强调色，用于 diff "−" 行前景色（当存在时）。
#[must_use]
#[allow(dead_code)]
const fn theme_red(ui: &UiTheme) -> Color {
    ui.diff_deleted_fg
}

/// 每预设的深绿色 diff 添加背景色调。
#[must_use]
const fn theme_diff_added_bg(ui: &UiTheme) -> Color {
    ui.diff_added_bg
}

/// 每预设的深红色 diff 删除背景色调。
#[must_use]
const fn theme_diff_deleted_bg(ui: &UiTheme) -> Color {
    ui.diff_deleted_bg
}

/// 如果预设参与单元格级重映射，则返回 `true`。默认
/// Whale 和 System 主题不变地通过，因此整个阶段在
/// 热路径上编译为单个加载+比较。
#[inline]
#[must_use]
pub const fn theme_remap_active(theme: ThemeId) -> bool {
    matches!(
        theme,
        ThemeId::Terminal
            | ThemeId::CatppuccinMocha
            | ThemeId::TokyoNight
            | ThemeId::Dracula
            | ThemeId::GruvboxDark
            | ThemeId::Claude
            | ThemeId::Matrix
            | ThemeId::SolarizedLight
    )
}

/// 为社区主题预设重映射前景色。镜像了
/// [`adapt_fg_for_palette_mode`] 的结构——相同的源集合，不同的
/// 目的地，源自预设的 [`UiTheme`]。
///
/// `ui` 参数是 `App` 上携带的*活跃* UiTheme——
/// `ThemeId.ui_theme()` 已应用用户的 `background_color` 覆盖。
/// 将其传递进去（而不是在此函数内从 `theme` 重新解析）保留了
/// 该覆盖；否则用户将 `background_color = "#..."` 与社区主题
/// 结合使用，会在每次单元格重映射时看到其覆盖被预设的
/// surface_bg 静默覆盖。
#[must_use]
pub fn adapt_fg_for_theme(color: Color, theme: ThemeId, ui: &UiTheme) -> Color {
    if !theme_remap_active(theme) {
        return color;
    }

    if color == TEXT_BODY || color == SELECTION_TEXT || color == Color::White {
        ui.text_body
    } else if color == TEXT_SECONDARY || color == TEXT_MUTED {
        ui.text_muted
    } else if color == TEXT_HINT || color == TEXT_DIM {
        ui.text_hint
    } else if color == TEXT_SOFT || color == TEXT_TOOL_OUTPUT {
        ui.text_soft
    } else if color == BORDER_COLOR {
        ui.border
    } else if color == TEXT_ACCENT || color == WHALE_INFO || color == ACCENT_TOOL_LIVE {
        ui.status_working
    } else if color == TEXT_REASONING || color == ACCENT_REASONING_LIVE {
        if theme == ThemeId::Matrix {
            Color::Rgb(0x00, 0x55, 0x00) // #005500
        } else {
            ui.mode_plan
        }
    } else if color == ACCENT_TOOL_ISSUE {
        ui.mode_yolo
    } else if color == STATUS_WARNING {
        ui.warning
    } else if color == STATUS_ERROR || color == WHALE_ERROR {
        ui.error_fg
    } else if color == DIFF_ADDED || color == USER_BODY {
        theme_green(ui)
    } else if color == WHALE_ACCENT_PRIMARY {
        ui.mode_agent
    } else {
        color
    }
}

/// 为社区主题预设重映射背景色。参见
/// `adapt_fg_for_theme` 上的 `ui` 说明——此处约定相同。
#[must_use]
pub fn adapt_bg_for_theme(color: Color, theme: ThemeId, ui: &UiTheme) -> Color {
    if !theme_remap_active(theme) {
        return color;
    }

    if color == WHALE_BG || color == BACKGROUND_DARK {
        ui.surface_bg
    } else if color == WHALE_PANEL
        || color == COMPOSER_BG
        || color == SURFACE_PANEL
        || color == SURFACE_TOOL
    {
        ui.panel_bg
    } else if color == SURFACE_ELEVATED || color == SURFACE_TOOL_ACTIVE {
        ui.elevated_bg
    } else if color == SURFACE_REASONING
        || color == SURFACE_REASONING_TINT
        || color == SURFACE_REASONING_ACTIVE
    {
        ui.panel_bg
    } else if color == SURFACE_SUCCESS {
        ui.diff_added_bg
    } else if color == SURFACE_ERROR {
        ui.error_surface
    } else if color == SELECTION_BG {
        ui.selection_bg
    } else if color == DIFF_ADDED_BG {
        theme_diff_added_bg(ui)
    } else if color == DIFF_DELETED_BG {
        theme_diff_deleted_bg(ui)
    } else {
        color
    }
}

fn adapt_fg_for_grayscale_palette(color: Color) -> Color {
    if color == Color::Reset {
        return color;
    }
    if color == TEXT_BODY
        || color == SELECTION_TEXT
        || color == LIGHT_TEXT_BODY
        || color == Color::White
        || color == WHALE_ERROR
        || color == STATUS_ERROR
        || color == MODE_YOLO
    {
        GRAYSCALE_TEXT_BODY
    } else if color == TEXT_SOFT
        || color == TEXT_TOOL_OUTPUT
        || color == LIGHT_TEXT_SOFT
        || color == TEXT_ACCENT
        || color == WHALE_INFO
        || color == WHALE_ACCENT_PRIMARY
        || color == ACCENT_TOOL_LIVE
        || color == STATUS_SUCCESS
        || color == STATUS_INFO
        || color == MODE_AGENT
    {
        GRAYSCALE_TEXT_SOFT
    } else if color == TEXT_SECONDARY
        || color == TEXT_MUTED
        || color == LIGHT_TEXT_MUTED
        || color == TEXT_REASONING
        || color == ACCENT_REASONING_LIVE
        || color == STATUS_WARNING
        || color == MODE_PLAN
        || color == USER_BODY
        || color == LIGHT_USER_BODY
        || color == DIFF_ADDED
    {
        GRAYSCALE_TEXT_MUTED
    } else if color == TEXT_HINT
        || color == TEXT_DIM
        || color == LIGHT_TEXT_HINT
        || color == BORDER_COLOR
        || color == LIGHT_BORDER
        || color == ACCENT_TOOL_ISSUE
    {
        GRAYSCALE_TEXT_HINT
    } else {
        match color {
            Color::Black => GRAYSCALE_TEXT_BODY,
            Color::Gray | Color::DarkGray => GRAYSCALE_TEXT_HINT,
            Color::Red
            | Color::LightRed
            | Color::Green
            | Color::LightGreen
            | Color::Yellow
            | Color::LightYellow
            | Color::Blue
            | Color::LightBlue
            | Color::Magenta
            | Color::LightMagenta
            | Color::Cyan
            | Color::LightCyan => GRAYSCALE_TEXT_SOFT,
            Color::Rgb(r, g, b) => grayscale_fg_from_luma(luma(r, g, b)),
            Color::Indexed(_) => color,
            _ => color,
        }
    }
}

fn adapt_bg_for_grayscale_palette(color: Color) -> Color {
    if color == Color::Reset {
        return color;
    }
    if color == WHALE_BG || color == BACKGROUND_DARK || color == LIGHT_SURFACE {
        GRAYSCALE_SURFACE
    } else if color == WHALE_PANEL
        || color == COMPOSER_BG
        || color == SURFACE_PANEL
        || color == SURFACE_TOOL
        || color == LIGHT_PANEL
    {
        GRAYSCALE_PANEL
    } else if color == SURFACE_ELEVATED
        || color == SURFACE_TOOL_ACTIVE
        || color == LIGHT_ELEVATED
        || color == SELECTION_BG
        || color == LIGHT_SELECTION_BG
    {
        GRAYSCALE_ELEVATED
    } else if color == SURFACE_REASONING
        || color == SURFACE_REASONING_TINT
        || color == SURFACE_REASONING_ACTIVE
        || color == LIGHT_REASONING
    {
        GRAYSCALE_REASONING
    } else if color == SURFACE_SUCCESS || color == DIFF_ADDED_BG || color == LIGHT_SUCCESS {
        GRAYSCALE_SUCCESS
    } else if color == SURFACE_ERROR || color == DIFF_DELETED_BG || color == LIGHT_ERROR {
        GRAYSCALE_ERROR
    } else {
        match color {
            Color::Black => GRAYSCALE_SURFACE,
            Color::White | Color::Gray => GRAYSCALE_ELEVATED,
            Color::DarkGray => GRAYSCALE_PANEL,
            Color::Red
            | Color::LightRed
            | Color::Green
            | Color::LightGreen
            | Color::Yellow
            | Color::LightYellow
            | Color::Blue
            | Color::LightBlue
            | Color::Magenta
            | Color::LightMagenta
            | Color::Cyan
            | Color::LightCyan => GRAYSCALE_ELEVATED,
            Color::Rgb(r, g, b) => grayscale_bg_from_luma(luma(r, g, b)),
            Color::Indexed(_) => color,
            _ => color,
        }
    }
}

fn grayscale_fg_from_luma(luma: u8) -> Color {
    match luma {
        0..=95 => GRAYSCALE_TEXT_HINT,
        96..=155 => GRAYSCALE_TEXT_MUTED,
        156..=215 => GRAYSCALE_TEXT_SOFT,
        _ => GRAYSCALE_TEXT_BODY,
    }
}

fn grayscale_bg_from_luma(luma: u8) -> Color {
    match luma {
        0..=28 => GRAYSCALE_SURFACE,
        29..=95 => GRAYSCALE_PANEL,
        96..=185 => GRAYSCALE_ELEVATED,
        _ => GRAYSCALE_REASONING,
    }
}

pub(crate) fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114 + 500) / 1000) as u8
}
// === 颜色深度 + 亮度辅助函数（v0.6.6 UI 重新设计）===

/// 终端颜色深度，用于在无法忠实渲染它们的终端上
/// 限制真彩色表面（例如思考背景色调）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// 16 色终端（macOS Terminal.app 默认值、简陋的 tmux 设置）。
    /// 背景色调会扭曲命名调色板映射，因此我们丢弃它们。
    Ansi16,
    /// 256 色终端——RGB→256 回退足够忠实。
    Ansi256,
    /// 真彩色（24 位）——逐字渲染调色板。
    TrueColor,
}

impl ColorDepth {
    /// 检测活跃终端的颜色深度。首先检查 `COLORTERM`
    ///（truecolor / 24bit），然后回退到 `TERM`。默认为
    /// `TrueColor`，因为大多数现代终端支持它；保守的
    /// 回退是 `Ansi16`，这样背景色调会安全消失。
    #[must_use]
    pub fn detect() -> Self {
        if let Ok(ct) = std::env::var("COLORTERM") {
            let ct = ct.to_ascii_lowercase();
            if ct.contains("truecolor") || ct.contains("24bit") {
                return Self::TrueColor;
            }
        }
        if std::env::var_os("WT_SESSION").is_some() {
            return Self::TrueColor;
        }
        if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
            let term_program = term_program.to_ascii_lowercase();
            if term_program.contains("iterm")
                || term_program.contains("wezterm")
                || term_program.contains("vscode")
                || term_program.contains("warp")
            {
                return Self::TrueColor;
            }
        }
        let term = std::env::var("TERM").unwrap_or_default();
        let term = term.to_ascii_lowercase();
        if term.contains("truecolor") || term.contains("24bit") {
            Self::TrueColor
        } else if term.contains("256") {
            Self::Ansi256
        } else if term.is_empty() || term == "dumb" {
            Self::Ansi16
        } else {
            // 未知的 TERM 字符串默认不应接收 24 位 SGR。
            // 较老的 macOS/远程终端可能将真彩色背景渲染为
            // 亮青色块；256 色输出是更安全的折中。
            Self::Ansi256
        }
    }
}

/// 适配前景色到终端的颜色深度。
///
/// 在 TrueColor 上，`color` 直接通过。在 Ansi256 上，我们让 ratatui 的
/// 渲染器降级转换（它已经这样做了）。在 Ansi16 上，我们将 RGB 剥离为
/// 接近的命名颜色，这样语义意图即使在旧终端上也能存活。
#[allow(dead_code)]
#[must_use]
pub fn adapt_color(color: Color, depth: ColorDepth) -> Color {
    match (color, depth) {
        (_, ColorDepth::TrueColor) => color,
        (Color::Rgb(r, g, b), ColorDepth::Ansi256) => Color::Indexed(rgb_to_ansi256(r, g, b)),
        (Color::Rgb(r, g, b), ColorDepth::Ansi16) => nearest_ansi16(r, g, b),
        _ => color,
    }
}

/// 适配背景色。在 Ansi16 终端上，背景色调有噪声，
/// 因此我们将其降为 `Color::Reset`，而不是尝试粗略的命名颜色
/// 匹配——安静的背景比错误的背景更清晰。
#[allow(dead_code)]
#[must_use]
pub fn adapt_bg(color: Color, depth: ColorDepth) -> Color {
    match (color, depth) {
        (_, ColorDepth::TrueColor) => color,
        (Color::Rgb(r, g, b), ColorDepth::Ansi256) => Color::Indexed(rgb_to_ansi256(r, g, b)),
        (_, ColorDepth::Ansi256) => color,
        (_, ColorDepth::Ansi16) => Color::Reset,
    }
}

/// 在 `alpha` 下混合两种 RGB 颜色（0.0 = `bg`，1.0 = `fg`）。
/// 任何非 RGB 的颜色回退到 `fg`——在命名调色板条目上
/// 没有有意义的 alpha 混合。
#[allow(dead_code)]
#[must_use]
pub fn blend(fg: Color, bg: Color, alpha: f32) -> Color {
    let alpha = alpha.clamp(0.0, 1.0);
    match (fg, bg) {
        (Color::Rgb(fr, fg_, fb), Color::Rgb(br, bg_, bb)) => {
            let mix = |a: u8, b: u8| -> u8 {
                let a = f32::from(a);
                let b = f32::from(b);
                (b + (a - b) * alpha).round().clamp(0.0, 255.0) as u8
            };
            Color::Rgb(mix(fr, br), mix(fg_, bg_), mix(fb, bb))
        }
        _ => fg,
    }
}

/// 返回能忠实渲染背景色的终端的专用思考表面色调。
/// ANSI-16 终端禁用此色调，因为最接近的命名背景
/// 对于这种微妙的处理来说过于粗糙。
#[must_use]
pub fn reasoning_surface_tint(depth: ColorDepth) -> Option<Color> {
    match depth {
        ColorDepth::Ansi16 => None,
        _ => Some(adapt_bg(SURFACE_REASONING_TINT, depth)),
    }
}

/// 基于 `now_ms`（纪元毫秒）在 2 秒周期上将 `color` 在
/// 30% 和 100% 亮度之间脉冲。最小值使字形在低谷时保持可读；
/// 最大值是原始源颜色。它们之间的线性插值看起来像
/// 缓慢的心跳。
#[must_use]
pub fn pulse_brightness(color: Color, now_ms: u64) -> Color {
    // 2 秒 = 2000 ms 完整周期；sin 给出平滑的 0..1..0 摆动。
    let phase = (now_ms % 2000) as f32 / 2000.0;
    let t = (phase * std::f32::consts::TAU).sin() * 0.5 + 0.5; // 0..1
    let alpha = 0.30 + t * 0.70; // 30%..100%
    match color {
        Color::Rgb(r, g, b) => {
            let s = |c: u8| -> u8 { ((f32::from(c)) * alpha).round().clamp(0.0, 255.0) as u8 };
            Color::Rgb(s(r), s(g), s(b))
        }
        other => other,
    }
}

/// 将 RGB 三元组映射到最接近的 ANSI-16 命名颜色。仅由
/// `adapt_color` 在 Ansi16 终端上使用；我们依赖色调主导 +
/// 亮度，使品牌颜色落在明显相关的命名条目上（天空→青色，
/// 蓝色→蓝色，红色→红色等），而不是在灰色周围抖动。
#[allow(dead_code)]
pub(crate) fn nearest_ansi16(r: u8, g: u8, b: u8) -> Color {
    let lum = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    if lum < 24 {
        return Color::Black;
    }
    if r > 220 && g > 220 && b > 220 {
        return Color::White;
    }
    let bright = lum > 144;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max.saturating_sub(min) < 16 {
        return if bright { Color::Gray } else { Color::DarkGray };
    }
    if r >= g && r >= b {
        if g > b + 24 {
            if bright {
                Color::LightYellow
            } else {
                Color::Yellow
            }
        } else if b > r.saturating_sub(24) {
            if bright {
                Color::LightMagenta
            } else {
                Color::Magenta
            }
        } else if bright {
            Color::LightRed
        } else {
            Color::Red
        }
    } else if g >= r && g >= b {
        if b > r + 24 {
            if bright {
                Color::LightCyan
            } else {
                Color::Cyan
            }
        } else if bright {
            Color::LightGreen
        } else {
            Color::Green
        }
    } else if r.saturating_add(48) >= b && r > g + 24 {
        if bright {
            Color::LightMagenta
        } else {
            Color::Magenta
        }
    } else if g.saturating_add(48) >= b && g > r + 24 {
        if bright {
            Color::LightCyan
        } else {
            Color::Cyan
        }
    } else if bright {
        Color::LightBlue
    } else {
        Color::Blue
    }
}

/// 将 RGB 颜色映射到最近的 xterm 256 色调色板索引。我们只使用
/// 稳定的 6x6x6 立方体和灰度斜坡（16..255），而不是终端的
/// 用户可配置的 0..15 颜色。
#[allow(dead_code)]
pub(crate) fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

    fn nearest_cube_level(channel: u8) -> usize {
        CUBE_LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, level)| channel.abs_diff(**level))
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    fn dist_sq(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
        let dr = i32::from(a.0) - i32::from(b.0);
        let dg = i32::from(a.1) - i32::from(b.1);
        let db = i32::from(a.2) - i32::from(b.2);
        (dr * dr + dg * dg + db * db) as u32
    }

    let ri = nearest_cube_level(r);
    let gi = nearest_cube_level(g);
    let bi = nearest_cube_level(b);
    let cube_rgb = (CUBE_LEVELS[ri], CUBE_LEVELS[gi], CUBE_LEVELS[bi]);
    let cube_index = 16 + (36 * ri) as u8 + (6 * gi) as u8 + bi as u8;

    let avg = ((u16::from(r) + u16::from(g) + u16::from(b)) / 3) as u8;
    let gray_i = if avg <= 8 {
        0
    } else if avg >= 238 {
        23
    } else {
        ((u16::from(avg) - 8 + 5) / 10).min(23) as u8
    };
    let gray = 8 + 10 * gray_i;
    let gray_index = 232 + gray_i;

    if dist_sq((r, g, b), (gray, gray, gray)) < dist_sq((r, g, b), cube_rgb) {
        gray_index
    } else {
        cube_index
    }
}
