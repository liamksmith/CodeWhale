//! v0.6.6 会话记录重新设计的工具卡视觉词汇。
//!
//! 工具卡是代理运行 `read_file`、`exec_shell`、`apply_patch` 等时出现的方框。
//! 视觉词汇有意保持稀疏：单个动词字形标识族，左侧轨道将卡片锚定到时间线，
//! 旋转器节奏重用现有工具状态动画。
//!
//! 此模块拥有：
//!
//! - [`ToolFamily`] — 规范语义族以及尚未有族的事物的 `Generic` 回退。
//! - [`tool_family_for_title`] — 将遗留 `render_tool_header` 标题字符串
//!   （`"Shell"`、`"Patch"`、`"Workspace"` 等）映射到族。使现有调用点
//!   可以放入族字形而无需重构每个单元格。
//! - [`family_glyph`] / [`family_label`] — 每个族的动词字形 + 标签。
//!   字形是单个字素；标签是短动词。
//! - [`CardRail`] / [`rail_glyph`] — 锚定到左边距的 `╭ │ ╰` 轨道，
//!   使眼睛可以分组多行卡片。
//!
//! 实际行的组合仍在 `history.rs` 内部进行；此模块是词汇表，不是布局引擎。
//! 保持小型意味着未来的视觉刷新只需触摸这里的常量。

use crate::localization::Locale;

/// 工具族——代理正在执行的动词。用于选择卡片标题的字形和标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolFamily {
    /// 读取、列出、探索。`▷ read`。
    Read,
    /// 编辑、补丁、写入。`◆ patch`。
    Patch,
    /// Shell、子进程。`▶ run`。
    Run,
    /// Grep、模糊文件搜索、网络搜索。`⌕ find`。
    Find,
    /// 单个子代理分发。`◐ delegate`。
    Delegate,
    /// 多代理扇出分发（rlm）。`⋮⋮ fanout`。
    Fanout,
    /// 递归语言模型工作。`⋮⋮ rlm`。
    Rlm,
    /// 验证门控、测试和验证器。`✓ verify`。
    Verify,
    /// 推理/思维链。`… think`。推理有自己的渲染路径
    ///（`history.rs` 中的 `render_thinking`）；
    /// 在此声明族是为了完整性，以便任何未来代码可以访问
    /// 匹配的字形 + 标签词汇。
    #[allow(dead_code)]
    Think,
    /// 任何我们还没有族字形的——回退到中性项目符号，
    /// 以便卡片仍然干净渲染。
    Generic,
}

/// 将遗留工具标题字符串（传递给 `render_tool_header` 的值）映射到族。
/// 任何无法识别的回退到 [`ToolFamily::Generic`]，以便卡片仍然渲染——
/// 它们只是失去动词字形处理，直到族被添加到此。
#[must_use]
pub fn tool_family_for_title(title: &str) -> ToolFamily {
    match title {
        "Shell" => ToolFamily::Run,
        "Patch" | "Diff" => ToolFamily::Patch,
        "Workspace" | "Image" => ToolFamily::Read,
        "Search" => ToolFamily::Find,
        "Plan" | "Strategy" | "Review" => ToolFamily::Generic,
        _ => ToolFamily::Generic,
    }
}

/// 将任意工具名称（如向模型暴露的——例如 `read_file`、`apply_patch`、`agent`）
/// 映射到族。由 `GenericToolCell` 使用，其中 `tool_family_for_title` 快捷方式
/// 不够，因为每个通用单元格共享标题 `"Tool"`。
#[must_use]
pub fn tool_family_for_name(name: &str) -> ToolFamily {
    match name {
        "read_file" | "list_dir" | "view_image" | "git_log" | "git_show" | "git_blame" => {
            ToolFamily::Read
        }
        "edit_file" | "apply_patch" | "write_file" => ToolFamily::Patch,
        "exec_shell"
        | "exec_shell_wait"
        | "exec_shell_interact"
        | "exec_shell_cancel"
        | "task_shell_start"
        | "task_shell_wait" => ToolFamily::Run,
        "grep_files" | "file_search" | "web_search" | "fetch_url" => ToolFamily::Find,
        "agent" => ToolFamily::Delegate,
        "rlm_open" | "rlm_eval" | "rlm_configure" | "rlm_close" | "rlm" => ToolFamily::Rlm,
        "run_tests"
        | "run_verifiers"
        | "task_gate_run"
        | "validate_data"
        | "wait_for_dev_server" => ToolFamily::Verify,
        // 工作流运行是多子活动；重用扇出字形，以便
        // 紧凑历史卡片（#4122）与直接多代理卡片共享视觉词汇，
        // 而不是中性通用项目符号。
        "workflow" => ToolFamily::Fanout,
        _ => ToolFamily::Generic,
    }
}

/// 任意工具名称的用户面向标签。已知工具折叠为语义动词；
/// 未知工具保留其确切名称以便调试。
#[cfg(test)]
#[must_use]
fn tool_display_label_for_name(name: &str) -> String {
    let family = tool_family_for_name(name);
    if matches!(family, ToolFamily::Generic) {
        name.to_string()
    } else {
        family_label(family).to_string()
    }
}

fn family_message_id(family: ToolFamily) -> crate::localization::MessageId {
    match family {
        ToolFamily::Read => crate::localization::MessageId::ToolFamilyRead,
        ToolFamily::Patch => crate::localization::MessageId::ToolFamilyPatch,
        ToolFamily::Run => crate::localization::MessageId::ToolFamilyRun,
        ToolFamily::Find => crate::localization::MessageId::ToolFamilyFind,
        ToolFamily::Delegate => crate::localization::MessageId::ToolFamilyDelegate,
        ToolFamily::Fanout => crate::localization::MessageId::ToolFamilyFanout,
        ToolFamily::Rlm => crate::localization::MessageId::ToolFamilyRlm,
        ToolFamily::Verify => crate::localization::MessageId::ToolFamilyVerify,
        ToolFamily::Think => crate::localization::MessageId::ToolFamilyThink,
        ToolFamily::Generic => crate::localization::MessageId::ToolFamilyGeneric,
    }
}

/// 任意工具名称的紧凑活动/状态标签。已知内置工具使用语义动词；
/// 未知工具保留 `tool NAME` 形式。
#[must_use]
pub fn tool_activity_label_for_name(name: &str, locale: Locale) -> String {
    let family = tool_family_for_name(name);
    let mid = family_message_id(family);
    if matches!(family, ToolFamily::Generic) {
        format!("{} {name}", crate::localization::tr(locale, mid))
    } else {
        crate::localization::tr(locale, mid).to_string()
    }
}

/// 从公共工具名称和已清洗的参数摘要为工具标题构建紧凑语义摘要。
#[must_use]
pub fn tool_header_summary_for_name(name: &str, input_summary: Option<&str>) -> Option<String> {
    let family = tool_family_for_name(name);
    let summary = input_summary
        .map(str::trim)
        .filter(|summary| !summary.is_empty());

    let preferred_keys = match family {
        ToolFamily::Read | ToolFamily::Patch => ["path", "file", "target", "content"].as_slice(),
        ToolFamily::Run => ["command", "cmd", "script"].as_slice(),
        ToolFamily::Find => ["query", "pattern", "path", "scope"].as_slice(),
        ToolFamily::Delegate | ToolFamily::Fanout | ToolFamily::Rlm => {
            ["prompt", "task", "model"].as_slice()
        }
        ToolFamily::Verify => ["profile", "level", "command", "args", "path"].as_slice(),
        ToolFamily::Think | ToolFamily::Generic => {
            ["query", "path", "command", "prompt"].as_slice()
        }
    };

    let selected_summary = summary.and_then(|summary| {
        for key in preferred_keys {
            if let Some(value) = summary_value(summary, key) {
                return Some(value);
            }
        }

        if summary_is_noisy_control_only(summary) {
            None
        } else {
            Some(summary.to_string())
        }
    });

    if should_show_tool_name_in_header(name, family) {
        let tool_name = name.trim();
        if tool_name.is_empty() {
            return selected_summary;
        }
        return Some(match selected_summary {
            Some(summary) if summary != tool_name => format!("{tool_name} · {summary}"),
            _ => tool_name.to_string(),
        });
    }

    selected_summary
}

fn summary_value(summary: &str, key: &str) -> Option<String> {
    for part in summary.split(", ") {
        let Some((part_key, value)) = part.split_once(':') else {
            continue;
        };
        if part_key.trim() == key {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn should_show_tool_name_in_header(name: &str, family: ToolFamily) -> bool {
    (matches!(family, ToolFamily::Generic) && !is_known_metadata_tool_name(name))
        || matches!(name, "git_log" | "git_show" | "git_blame")
}

fn is_known_metadata_tool_name(name: &str) -> bool {
    matches!(
        name,
        "update_plan"
            | "work_update"
            | "todo_write"
            | "todo_add"
            | "todo_update"
            | "checklist_write"
            | "checklist_add"
            | "checklist_update"
            | "checklist_list"
    )
}

fn summary_is_noisy_control_only(summary: &str) -> bool {
    let mut saw_control = false;
    for part in summary.split(", ") {
        let Some((key, value)) = part.split_once(':') else {
            return false;
        };
        if value.trim().is_empty() {
            continue;
        }
        if !is_noisy_summary_key(key.trim()) {
            return false;
        }
        saw_control = true;
    }
    saw_control
}

fn is_noisy_summary_key(key: &str) -> bool {
    matches!(
        key,
        "limit"
            | "max_count"
            | "max_output_tokens"
            | "offset"
            | "page"
            | "page_size"
            | "per_page"
            | "response_length"
            | "timeout_ms"
            | "yield_time_ms"
    )
}

/// 族的动词字形。单字素，使得 `render_tool_header` 中的标题布局计算
/// 保持简单（一个单元格宽）。
#[must_use]
pub fn family_glyph(family: ToolFamily) -> &'static str {
    match family {
        ToolFamily::Read => "\u{25B7}",           // ▷
        ToolFamily::Patch => "\u{25C6}",          // ◆
        ToolFamily::Run => "\u{25B6}",            // ▶
        ToolFamily::Find => "\u{2315}",           // ⌕
        ToolFamily::Delegate => "\u{25D0}",       // ◐
        ToolFamily::Fanout => "\u{22EE}\u{22EE}", // ⋮⋮（两个单元格）
        ToolFamily::Rlm => "\u{22EE}\u{22EE}",    // ⋮⋮（两个单元格）
        ToolFamily::Verify => "\u{2713}",
        ToolFamily::Think => "\u{2026}",   // …
        ToolFamily::Generic => "\u{2022}", // •
    }
}

/// 族的简短动词标签——出现在卡片标题中，位于字形旁边。
/// 有意小写；动词字形 + 标签是新的卡片标题词汇。
#[must_use]
pub fn family_label(family: ToolFamily) -> &'static str {
    match family {
        ToolFamily::Read => "read",
        ToolFamily::Patch => "patch",
        ToolFamily::Run => "run",
        ToolFamily::Find => "find",
        ToolFamily::Delegate => "delegate",
        ToolFamily::Fanout => "fanout",
        ToolFamily::Rlm => "rlm",
        ToolFamily::Verify => "verify",
        ToolFamily::Think => "think",
        ToolFamily::Generic => "tool",
    }
}

/// 多行卡片中某行的位置——驱动左侧轨道字形，使从顶部到底部的方框
/// 读作一个连续组。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CardRail {
    /// 卡片的第一个行——标题。`╭`。
    Top,
    /// 任何中间行——正文内容。`│`。
    Middle,
    /// 卡片的最后一行。`╰`。
    Bottom,
    /// 单行卡片——完全没有轨道。
    Single,
}

/// 将 [`CardRail`] 位置映射到其轨道字形。返回为 `&str`，
/// 因为调用者将其粘贴到 span 中。
#[must_use]
#[allow(dead_code)]
pub fn rail_glyph(rail: CardRail) -> &'static str {
    match rail {
        CardRail::Top => "\u{256D}",    // ╭
        CardRail::Middle => "\u{2502}", // │
        CardRail::Bottom => "\u{2570}", // ╰
        CardRail::Single => "",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CardRail, ToolFamily, family_glyph, family_label, rail_glyph, tool_activity_label_for_name,
        tool_display_label_for_name, tool_family_for_name, tool_family_for_title,
        tool_header_summary_for_name,
    };
    use crate::localization::{Locale, MessageId, tr};

    #[test]
    fn legacy_titles_route_to_expected_families() {
        assert_eq!(tool_family_for_title("Shell"), ToolFamily::Run);
        assert_eq!(tool_family_for_title("Patch"), ToolFamily::Patch);
        assert_eq!(tool_family_for_title("Workspace"), ToolFamily::Read);
        assert_eq!(tool_family_for_title("Search"), ToolFamily::Find);
        assert_eq!(tool_family_for_title("Diff"), ToolFamily::Patch);
        assert_eq!(tool_family_for_title("Plan"), ToolFamily::Generic);
        assert_eq!(tool_family_for_title("Strategy"), ToolFamily::Generic);
        assert_eq!(tool_family_for_title("unknown title"), ToolFamily::Generic);
    }

    #[test]
    fn tool_names_route_to_families_by_verb() {
        assert_eq!(tool_family_for_name("read_file"), ToolFamily::Read);
        assert_eq!(tool_family_for_name("apply_patch"), ToolFamily::Patch);
        assert_eq!(tool_family_for_name("exec_shell"), ToolFamily::Run);
        assert_eq!(tool_family_for_name("task_shell_start"), ToolFamily::Run);
        assert_eq!(tool_family_for_name("grep_files"), ToolFamily::Find);
        assert_eq!(tool_family_for_name("git_log"), ToolFamily::Read);
        assert_eq!(tool_family_for_name("agent"), ToolFamily::Delegate);
        assert_eq!(tool_family_for_name("rlm_eval"), ToolFamily::Rlm);
        assert_eq!(tool_family_for_name("run_verifiers"), ToolFamily::Verify);
        assert_eq!(
            tool_family_for_name("wait_for_dev_server"),
            ToolFamily::Verify
        );
        assert_eq!(
            tool_family_for_name("totally_new_tool"),
            ToolFamily::Generic
        );
    }

    #[test]
    fn tool_display_label_collapses_known_tools_to_user_verbs() {
        assert_eq!(tool_display_label_for_name("exec_shell"), "run");
        assert_eq!(tool_display_label_for_name("run_verifiers"), "verify");
        assert_eq!(tool_display_label_for_name("file_search"), "find");
        assert_eq!(
            tool_display_label_for_name("future_private_tool"),
            "future_private_tool"
        );

        assert_eq!(
            tool_activity_label_for_name("exec_shell", Locale::En),
            "run"
        );
        assert_eq!(
            tool_activity_label_for_name("run_verifiers", Locale::En),
            "verify"
        );
        assert_eq!(
            tool_activity_label_for_name("future_private_tool", Locale::En),
            "tool future_private_tool"
        );
    }

    #[test]
    fn tool_header_summary_prefers_family_specific_arguments() {
        assert_eq!(
            tool_header_summary_for_name("read_file", Some("path: src/main.rs, limit: 20"))
                .as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            tool_header_summary_for_name("exec_shell", Some("command: cargo test, cwd: /repo"))
                .as_deref(),
            Some("cargo test")
        );
        assert_eq!(
            tool_header_summary_for_name("grep_files", Some("pattern: TODO, path: crates"))
                .as_deref(),
            Some("TODO")
        );
        assert_eq!(
            tool_header_summary_for_name("run_verifiers", Some("profile: auto, level: quick"))
                .as_deref(),
            Some("auto")
        );
        assert_eq!(
            tool_header_summary_for_name("unknown", Some("alpha: beta")).as_deref(),
            Some("unknown · alpha: beta")
        );
        assert_eq!(
            tool_header_summary_for_name("git_log", Some("max_count: 15")).as_deref(),
            Some("git_log")
        );
        assert_eq!(
            tool_header_summary_for_name("future_private_tool", Some("max_count: 15")).as_deref(),
            Some("future_private_tool")
        );
        assert_eq!(
            tool_header_summary_for_name("future_private_tool", None).as_deref(),
            Some("future_private_tool")
        );
        assert_eq!(
            tool_header_summary_for_name("todo_write", Some("items: <2 items>")).as_deref(),
            Some("items: <2 items>")
        );
    }

    #[test]
    fn each_family_has_a_glyph_and_label() {
        // 烟雾测试——发现未来重构中意外的空值。
        for family in [
            ToolFamily::Read,
            ToolFamily::Patch,
            ToolFamily::Run,
            ToolFamily::Find,
            ToolFamily::Delegate,
            ToolFamily::Fanout,
            ToolFamily::Rlm,
            ToolFamily::Verify,
            ToolFamily::Think,
            ToolFamily::Generic,
        ] {
            assert!(
                !family_glyph(family).is_empty(),
                "族 {family:?} 有空字形",
            );
            assert!(
                !family_label(family).is_empty(),
                "族 {family:?} 有空标签",
            );
        }
    }

    #[test]
    fn card_rail_glyphs_form_a_box() {
        assert_eq!(rail_glyph(CardRail::Top), "\u{256D}");
        assert_eq!(rail_glyph(CardRail::Middle), "\u{2502}");
        assert_eq!(rail_glyph(CardRail::Bottom), "\u{2570}");
        assert!(rail_glyph(CardRail::Single).is_empty());
    }

    #[test]
    fn tool_family_labels_localized_no_english_leak() {
        let checks: &[(MessageId, &str, &str)] = &[
            (MessageId::ToolFamilyRead, "read", "đọc,读,読,读取,ler,leer"),
            (
                MessageId::ToolFamilyPatch,
                "patch",
                "vá,補,パ,修补,corrigir,parchear",
            ),
            (
                MessageId::ToolFamilyRun,
                "run",
                "chạy,執,実,运行,executar,ejecutar",
            ),
            (
                MessageId::ToolFamilyFind,
                "find",
                "tìm,搜,検,搜索,buscar,buscar",
            ),
            (
                MessageId::ToolFamilyDelegate,
                "delegate",
                "ủy,委,委,委,delegar,delegar",
            ),
            (
                MessageId::ToolFamilyVerify,
                "verify",
                "xác minh,驗,検,验,verificar,verificar",
            ),
            (
                MessageId::ToolFamilyThink,
                "think",
                "suy nghĩ,思,思,思,pensar,pensar",
            ),
            (
                MessageId::ToolFamilyGeneric,
                "tool",
                "công cụ,工具,ツール,工具,ferramenta,herramienta",
            ),
        ];
        for locale in [
            Locale::Ja,
            Locale::ZhHans,
            Locale::ZhHant,
            Locale::PtBr,
            Locale::Es419,
            Locale::Vi,
        ] {
            for (id, eng, _) in checks {
                let msg = tr(locale, *id);
                assert!(
                    !msg.eq_ignore_ascii_case(eng),
                    "{} 泄漏了精确英文 '{}' 对于 '{:?}': {msg}",
                    locale.tag(),
                    eng,
                    id
                );
            }
        }
    }

    #[test]
    fn tool_family_activity_label_localized_no_english_leak() {
        let known = [
            "exec_shell",
            "read_file",
            "apply_patch",
            "grep_files",
            "run_verifiers",
        ];
        let english_labels = ["run", "read", "patch", "find", "verify"];
        for locale in [
            Locale::Ja,
            Locale::ZhHans,
            Locale::ZhHant,
            Locale::PtBr,
            Locale::Es419,
            Locale::Vi,
        ] {
            for (tool, eng) in known.iter().zip(english_labels.iter()) {
                let label = tool_activity_label_for_name(tool, locale);
                assert!(
                    !label.eq_ignore_ascii_case(eng),
                    "{} 泄漏了英文 '{}' 对于工具 '{tool}': {label}",
                    locale.tag(),
                    eng,
                );
            }
        }
    }
}
