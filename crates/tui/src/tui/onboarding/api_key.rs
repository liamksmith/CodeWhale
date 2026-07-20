//! 新手引导的 API 密钥输入界面。

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::localization::MessageId;
use crate::palette;
use crate::tui::app::App;

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let provider = app.onboarding_provider;
    let mut lines = vec![
        Line::from(Span::styled(
            app.tr(MessageId::OnboardApiKeyTitle).to_string(),
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "{} ({})",
                app.tr(MessageId::OnboardApiKeyStep1),
                provider.display_name()
            ),
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
    ];
    if let Some(url) = provider.credential_url() {
        lines.push(Line::from(Span::styled(
            url.to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            app.tr(MessageId::OnboardApiKeyLocalHint).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )));
    }
    let saved_hint = app
        .tr(MessageId::OnboardApiKeySavedHint)
        .replace("{path}", &effective_config_path_display(app));
    lines.extend([
        Line::from(Span::styled(
            app.tr(MessageId::OnboardApiKeyStep2).to_string(),
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
        Line::from(""),
        Line::from(Span::styled(
            saved_hint,
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardApiKeyFormatHint).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
    ]);

    let masked = mask_key(&app.api_key_input);
    let placeholder = app.tr(MessageId::OnboardApiKeyPlaceholder).to_string();
    let display = if masked.is_empty() {
        placeholder
    } else {
        masked
    };
    lines.push(Line::from(vec![
        Span::styled(
            app.tr(MessageId::OnboardApiKeyLabel).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        ),
        Span::styled(
            display,
            Style::default()
                .fg(palette::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));

    if let Some(message) = app.status_message.as_deref() {
        lines.push(Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(palette::STATUS_WARNING),
        )));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        app.tr(MessageId::OnboardApiKeyFooter).to_string(),
        Style::default().fg(palette::TEXT_MUTED),
    )));

    lines
}

fn mask_key(input: &str) -> String {
    let trimmed = input.trim();
    let len = trimmed.chars().count();
    if len == 0 {
        return String::new();
    }
    if len <= 4 {
        return "*".repeat(len);
    }
    let visible: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{}{}", "*".repeat(len - 4), visible)
}

/// 显示有效的 config.toml 路径 (#3986)。
///
/// 优先使用 App 会话的 `config_path` 覆盖，然后是持久化使用的相同解析
/// （`CODEWHALE_CONFIG_PATH` / `CODEWHALE_HOME` / 默认值）。
/// 当路径在进程 home 目录下时，将 `$HOME` 折叠为 `~`。
fn effective_config_path_display(app: &App) -> String {
    let path = app
        .config_path
        .clone()
        .or_else(|| crate::config_persistence::config_toml_path(None).ok())
        .unwrap_or_else(|| std::path::PathBuf::from("~/.codewhale/config.toml"));
    collapse_home_prefix(&path)
}

fn collapse_home_prefix(path: &std::path::Path) -> String {
    if let Some(home) = crate::config::effective_home_dir()
        && let Ok(rel) = path.strip_prefix(&home)
    {
        if rel.as_os_str().is_empty() {
            return "~".to_string();
        }
        return format!("~/{}", rel.display());
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiProvider, Config};
    use crate::localization::Locale;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    fn test_app_with_locale(locale: Locale) -> App {
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
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = locale;
        app.onboarding_provider = ApiProvider::Zai;
        app
    }

    #[test]
    fn api_key_saved_hint_uses_effective_config_path() {
        // 隔离安装会设置 CODEWHALE_CONFIG_PATH；UI 不得硬编码
        // ~/.codewhale/config.toml (#3986)。
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = tmp.path().join("isolated-config.toml");
        let _cfg = crate::test_support::EnvVarGuard::set(
            "CODEWHALE_CONFIG_PATH",
            config.to_string_lossy().as_ref(),
        );
        let mut app = test_app_with_locale(Locale::En);
        app.config_path = Some(config.clone());
        let body: String = lines(&app)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains(config.to_string_lossy().as_ref())
                || body.contains("isolated-config.toml"),
            "保存提示应显示有效路径，body 为：\n{body}"
        );
        assert!(
            !body.contains("~/.codewhale/config.toml"),
            "隔离时不得硬编码默认 home 路径：{body}"
        );
    }

    #[test]
    fn api_key_screen_renders_in_selected_locale() {
        // 缺失新手引导本地化时最明显的回归：用户在步骤 2 选择了简体中文后，
        // 步骤 3 原本保持为英文。确保渲染的行实际包含我们发布的每个语言环境的翻译字符串。
        let zh = test_app_with_locale(Locale::ZhHans);
        let body: String = lines(&zh)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("连接你的 API 密钥"),
            "标题应提供商中立且已本地化为简体中文"
        );
        assert!(
            body.contains("z.ai/model-api"),
            "期望得到默认提供商凭据 URL，实际：{body}"
        );
        assert!(
            body.contains("密钥"),
            "期望简体中文'密钥'标签，实际：{body}"
        );
        assert!(
            body.contains("Enter 保存"),
            "期望简体中文底部提示，实际：{body}"
        );

        let ja = test_app_with_locale(Locale::Ja);
        let body: String = lines(&ja)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("キー"),
            "期望日语'キー'标签，实际：{body}"
        );

        let en = test_app_with_locale(Locale::En);
        let body: String = lines(&en)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("Press Enter to save"),
            "期望英文底部提示，实际：{body}"
        );
    }
}
