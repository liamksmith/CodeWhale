//! 首次运行引导的语言选择器 (#566)。
//!
//! 展示 TUI 所发布翻译的每个语言区域，以及一个 `auto` 选项，
//! 该选项会委托给 `LC_ALL` / `LANG`。选择会立即通过 `Settings::save` 持久化，
//! 因此引导的其余部分（以及后续每个会话）都能读取所选的标签。

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::localization::MessageId;
use crate::palette;
use crate::tui::app::App;

/// 选择器中显示的区域设置选项。顺序与键盘快捷键匹配。
/// 每个条目为 `(hotkey, settings_tag, native_name, english_label)`。
/// `settings_tag` 是 `Settings::set("locale", …)` 所接受的，
/// 也是 `localization::Locale` 在下次读取时解析的值。
pub const LANGUAGE_OPTIONS: &[(char, &str, &str, &str)] = &[
    ('1', "auto", "Auto-detect", "(LC_ALL / LANG)"),
    ('2', "en", "English", ""),
    ('3', "ja", "日本語", "(Japanese)"),
    ('4', "zh-Hans", "简体中文", "(Simplified Chinese)"),
    ('5', "zh-Hant", "繁體中文", "(Traditional Chinese)"),
    ('6', "pt-BR", "Português (Brasil)", "(Brazilian Portuguese)"),
    (
        '7',
        "es-419",
        "Español (Latinoamérica)",
        "(Latin American Spanish)",
    ),
    ('8', "vi", "Tiếng Việt", "(Vietnamese)"),
];

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let current_owned = app.current_locale_tag();
    let current = current_owned.as_str();

    let mut out: Vec<Line<'static>> = vec![
        Line::from(Span::styled(
            app.tr(MessageId::OnboardLanguageTitle).to_string(),
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardLanguageBlurb).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
    ];

    for (hotkey, tag, native, english) in LANGUAGE_OPTIONS {
        let is_current = current == *tag;
        let bullet = if is_current { "●" } else { "○" };
        let bullet_color = if is_current {
            palette::WHALE_ACCENT_PRIMARY
        } else {
            palette::TEXT_MUTED
        };
        let mut spans: Vec<Span<'static>> = vec![
            Span::styled(format!("  {bullet}  "), Style::default().fg(bullet_color)),
            Span::styled(
                format!("[{hotkey}] "),
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                native.to_string(),
                Style::default().fg(palette::TEXT_PRIMARY),
            ),
        ];
        if !english.is_empty() {
            spans.push(Span::styled(
                format!(" {english}"),
                Style::default().fg(palette::TEXT_MUTED),
            ));
        }
        out.push(Line::from(spans));
    }

    out.push(Line::from(""));
    out.push(Line::from(Span::styled(
        app.tr(MessageId::OnboardLanguageFooter).to_string(),
        Style::default().fg(palette::TEXT_MUTED),
    )));

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localization::Locale;

    /// 我们拥有翻译的每个语言区域都必须在选择器中提供，
    /// 否则底部会宣传无法选择任何内容的快捷键，
    /// 用户就永远无法切换到受支持的 UI 语言 (#3929)。
    #[test]
    fn picker_offers_every_shipped_locale() {
        let offered: Vec<&str> = LANGUAGE_OPTIONS.iter().map(|(_, tag, _, _)| *tag).collect();
        assert!(
            offered.contains(&"auto"),
            "picker must keep the auto-detect entry"
        );
        for locale in Locale::shipped() {
            let tag = locale.tag();
            assert!(
                offered.contains(&tag),
                "shipped locale {tag} is not offered in the language picker"
            );
        }
    }

    /// 快捷键必须是连续的数字 `1..=N`，这样底部的"1-N"范围始终保持真实，
    /// 并且 `KeyCode::Char` 查找能够解析。
    #[test]
    fn picker_hotkeys_are_contiguous_digits() {
        for (idx, (hotkey, tag, _, _)) in LANGUAGE_OPTIONS.iter().enumerate() {
            let expected = char::from_digit((idx + 1) as u32, 10).expect("digit");
            assert_eq!(
                *hotkey, expected,
                "option {tag} should use hotkey {expected}, not {hotkey}"
            );
        }
    }
}
