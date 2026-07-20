//! 终端调色板模式和颜色深度检测。

#[cfg(target_os = "macos")]
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Dark,
    Light,
    Grayscale,
    SolarizedLight,
}

impl PaletteMode {
    /// 解析 `COLORFGBG`，其最后一个数值段是终端背景色。值 >= 8 通常表示浅色配置。
    #[must_use]
    pub fn from_colorfgbg(value: &str) -> Option<Self> {
        let bg = value
            .split(';')
            .rev()
            .find_map(|part| part.parse::<u16>().ok())?;
        Some(if bg >= 8 { Self::Light } else { Self::Dark })
    }

    /// 检测当前调色板模式。存在 `COLORFGBG` 时优先；macOS 外观作为省略终端颜色提示的终端的回退。
    /// 缺失或无法解析的值默认为深色，以便现有终端设置保持已调优的主题。
    #[must_use]
    pub fn detect() -> Self {
        Self::detect_from_sources(
            std::env::var("COLORFGBG").ok().as_deref(),
            detect_macos_palette_mode(),
        )
    }

    #[must_use]
    pub(crate) fn detect_from_sources(
        colorfgbg: Option<&str>,
        macos_fallback: Option<Self>,
    ) -> Self {
        colorfgbg
            .and_then(Self::from_colorfgbg)
            .or(macos_fallback)
            .unwrap_or(Self::Dark)
    }
}

#[cfg(target_os = "macos")]
fn detect_macos_palette_mode() -> Option<PaletteMode> {
    let output = Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .ok()?;

    if output.status.success() {
        Some(palette_mode_from_apple_interface_style(
            &String::from_utf8_lossy(&output.stdout),
        ))
    } else {
        Some(PaletteMode::Light)
    }
}

#[cfg(not(target_os = "macos"))]
fn detect_macos_palette_mode() -> Option<PaletteMode> {
    None
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn palette_mode_from_apple_interface_style(value: &str) -> PaletteMode {
    if value.trim().eq_ignore_ascii_case("dark") {
        PaletteMode::Dark
    } else {
        PaletteMode::Light
    }
}
