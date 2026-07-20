//! 斜杠命令自动补全 + 弹出菜单辅助函数。
//!
//! 从 `tui/ui.rs` (P1.2) 中提取。屏幕上的弹出框本身由 composer 组件渲染；
//! 这些辅助函数负责提供条目、应用选择，以及在弹出框未打开时处理 Tab 补全。
//!
//! 有意与 `tui::file_mention` 分开，尽管两者都展示类似的弹出框——
//! 但触发字符、排序和选中后的行为差异足够大，需要保持分离。

use crate::commands;

use super::app::{App, looks_like_slash_command_input};
use super::model_picker::provider_scoped_model_completion_ids;
use super::widgets::SlashMenuEntry;
use super::widgets::slash_completion_hints_with_model_candidates;

/// 返回 composer 应显示的斜杠菜单条目，尊重
/// `slash_menu_hidden`（当用户按 Esc 关闭弹出框时设置）。
pub fn visible_slash_menu_entries(app: &App, limit: usize) -> Vec<SlashMenuEntry> {
    if app.slash_menu_hidden {
        return Vec::new();
    }
    if let Some((byte_start, partial)) =
        partial_inline_skill_mention_at_cursor(&app.input, app.cursor_position)
    {
        let trigger = app.input[byte_start..].chars().next().unwrap_or('/');
        return skill_mention_entries(&partial, trigger, limit, &app.cached_skills);
    }
    let model_candidates = provider_scoped_model_completion_ids(app);
    slash_completion_hints_with_model_candidates(
        &app.input,
        limit,
        &app.cached_skills,
        app.ui_locale,
        Some(&app.workspace),
        &model_candidates,
    )
}

/// 将当前选中的斜杠菜单条目应用到 composer 输入中。
/// 当命令需要参数时，可选地追加一个尾随空格，
/// 这样用户无需额外按键即可继续输入。
pub fn apply_slash_menu_selection(
    app: &mut App,
    entries: &[SlashMenuEntry],
    append_space: bool,
) -> bool {
    if entries.is_empty() {
        return false;
    }

    let selected_idx = app.slash_menu_selected.min(entries.len().saturating_sub(1));
    let selected = &entries[selected_idx];

    if selected.is_skill
        && let Some((byte_start, partial)) =
            partial_inline_skill_mention_at_cursor(&app.input, app.cursor_position)
        && let Some(skill_name) = skill_name_from_menu_entry(selected)
    {
        let trigger = app.input[byte_start..].chars().next().unwrap_or('/');
        replace_inline_skill_mention(app, byte_start, trigger, &partial, &skill_name);
        app.slash_menu_hidden = false;
        app.status_message = Some(format!("Skill selected: {trigger}{skill_name}"));
        return true;
    }

    let mut command = selected.name.clone();

    if append_space
        && !command.ends_with(' ')
        && !command.contains(char::is_whitespace)
        && let Some(info) = commands::get_command_info(command.trim_start_matches('/'))
        && info.name != "change"
        && (info.usage.contains('<') || info.usage.contains('['))
    {
        command.push(' ');
    }

    app.input = command;
    app.cursor_position = app.input.chars().count();
    app.slash_menu_hidden = false;
    app.status_message = Some(format!("Command selected: {}", app.input.trim_end()));
    true
}

/// 当在普通消息中用作内联提及（mention）时，返回光标下的 `/<skill>` 或 `$<skill>` 令牌。
/// composer 开头的 `/` 或 `$`，即使前面有空白字符，仍保留给斜杠命令使用
///（由 `slash_completion_hints` 处理）。
pub(crate) fn partial_inline_skill_mention_at_cursor(
    input: &str,
    cursor_chars: usize,
) -> Option<(usize, String)> {
    if looks_like_slash_command_input(input) {
        return None;
    }

    let chars: Vec<char> = input.chars().collect();
    if cursor_chars > chars.len() {
        return None;
    }

    let mut start_chars = cursor_chars;
    while start_chars > 0 {
        let prev = chars[start_chars - 1];
        if prev == '/' || prev == '$' {
            start_chars -= 1;
            break;
        }
        if prev.is_whitespace() {
            return None;
        }
        start_chars -= 1;
    }

    if start_chars == cursor_chars {
        return None;
    }
    let trigger = *chars.get(start_chars)?;
    if trigger != '/' && trigger != '$' {
        return None;
    }
    if !is_inline_skill_mention_start(&chars, start_chars) {
        return None;
    }

    let byte_start: usize = chars[..start_chars].iter().map(|c| c.len_utf8()).sum();
    if input[..byte_start].trim().is_empty() {
        return None;
    }

    let mut end_chars = start_chars + 1;
    while end_chars < chars.len() && !chars[end_chars].is_whitespace() {
        end_chars += 1;
    }
    let partial: String = chars[start_chars + 1..end_chars].iter().collect();
    if partial.contains('/') || partial.contains('$') {
        return None;
    }

    Some((byte_start, partial))
}

fn is_inline_skill_mention_start(chars: &[char], idx: usize) -> bool {
    if idx == 0 {
        return false;
    }
    chars
        .get(idx.saturating_sub(1))
        .is_some_and(|ch| ch.is_whitespace() || matches!(ch, '(' | '[' | '{' | '<' | '"' | '\''))
}

fn skill_mention_entries(
    partial: &str,
    trigger: char,
    limit: usize,
    cached_skills: &[(String, String)],
) -> Vec<SlashMenuEntry> {
    if limit == 0 {
        return Vec::new();
    }
    let partial_lower = partial.to_ascii_lowercase();
    let mut entries = cached_skills
        .iter()
        .filter(|(skill_name, _)| skill_name.to_ascii_lowercase().starts_with(&partial_lower))
        .map(|(skill_name, skill_desc)| SlashMenuEntry {
            name: format!("{trigger}{skill_name}"),
            description: skill_desc.clone(),
            is_skill: true,
            alias_hint: None,
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries.dedup_by(|a, b| a.name == b.name);
    entries.into_iter().take(limit).collect()
}

fn skill_name_from_menu_entry(entry: &SlashMenuEntry) -> Option<String> {
    if !entry.is_skill {
        return None;
    }
    if let Some(name) = entry.name.strip_prefix("/skill ") {
        return Some(name.trim().to_string());
    }
    entry
        .name
        .strip_prefix('/')
        .or_else(|| entry.name.strip_prefix('$'))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
}

fn replace_inline_skill_mention(
    app: &mut App,
    byte_start: usize,
    trigger: char,
    partial: &str,
    skill_name: &str,
) {
    let original_token_len = trigger.len_utf8() + partial.len();
    let original_token_end = byte_start + original_token_len;
    let mut new_input =
        String::with_capacity(app.input.len() - original_token_len + 1 + skill_name.len());
    new_input.push_str(&app.input[..byte_start]);
    new_input.push(trigger);
    new_input.push_str(skill_name);
    if original_token_end < app.input.len() {
        new_input.push_str(&app.input[original_token_end..]);
    }
    let new_cursor_chars = app.input[..byte_start].chars().count() + 1 + skill_name.chars().count();
    app.input = new_input;
    app.cursor_position = new_cursor_chars;
}

/// 对类似斜杠命令的输入进行 Tab 补全。将输入扩展到最长的无歧义前缀；
/// 如果恰好有一个命令匹配，则完整补全它（带尾随空格）。
/// 如果存在歧义，则发布一条状态提示，列出最多五个候选项。
/// 同时将技能名称作为补全候选项。
pub fn try_autocomplete_slash_command(app: &mut App) -> bool {
    if !looks_like_slash_command_input(&app.input) {
        return false;
    }

    let model_candidates = provider_scoped_model_completion_ids(app);
    let candidates = slash_completion_hints_with_model_candidates(
        &app.input,
        128,
        &app.cached_skills,
        app.ui_locale,
        Some(&app.workspace),
        &model_candidates,
    )
    .into_iter()
    .map(|entry| entry.name)
    .collect::<Vec<_>>();

    if candidates.is_empty() {
        return false;
    }

    let prefix = app.input.trim_start_matches('/');
    let refs: Vec<&str> = candidates
        .iter()
        .map(|name| name.trim_start_matches('/'))
        .collect();
    let shared = crate::tui::file_mention::longest_common_prefix(&refs);

    if !shared.is_empty() && shared.len() > prefix.len() {
        app.input = format!("/{shared}");
        app.cursor_position = app.input.chars().count();
        app.slash_menu_hidden = false;
        app.status_message = Some(format!("Autocomplete: /{shared}"));
        return true;
    }

    if candidates.len() == 1 {
        let mut completed = candidates[0].clone();
        if !completed.ends_with(' ') {
            completed.push(' ');
        }
        app.input = completed.clone();
        app.cursor_position = completed.chars().count();
        app.slash_menu_hidden = false;
        app.status_message = Some(format!("Command completed: {}", completed.trim_end()));
        return true;
    }

    let preview = candidates
        .iter()
        .take(5)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    app.status_message = Some(format!("Suggestions: {preview}"));
    true
}
