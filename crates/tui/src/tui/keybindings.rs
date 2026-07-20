//! 每个面向用户的按键绑定的纯文档目录。
//!
//! 此模块是帮助覆盖层渲染的快捷键的 *唯一事实来源*。
//! 实际的按键处理器位于 `tui/ui.rs`（以及一些兄弟模块）中；
//! 它们直接从 crossterm 事件流读取键，并且有意 **不** 查阅此目录。
//! 目录的存在是为了：
//!
//! 1. 帮助覆盖层（`tui/views/help.rs`）不必维护一个在添加或移动处理器时
//!    悄然腐烂的并行列表。
//! 2. 新贡献者在回答"哪些键被绑定，它们去了哪里？"时有一个查看的地方。
//!
//! 当你在 `ui.rs` 中添加或更改绑定时，**在此处添加或更新匹配的条目**。
//! 忘记的编译唯一副作用是过期的帮助屏幕；没有运行时崩溃，
//! 因此纪律存在于代码审查中。
//!
//! 条目按 `KeybindingSection` 分组。`chord` 字段是一个
//! 人类可读的字符串，格式与帮助中应该显示的方式完全相同——
//! 我们避免直接存储 `KeyBinding` 值，因为许多快捷键是
//! 对（`↑/↓`）或族（`1-8`），它们不能干净地映射到单个和弦。

use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeybindingSection {
    Navigation,
    Editing,
    Submission,
    Modes,
    Sessions,
    Clipboard,
    Help,
}

impl KeybindingSection {
    pub fn label(self, locale: crate::localization::Locale) -> Cow<'static, str> {
        use crate::localization::{MessageId, tr};
        let id = match self {
            Self::Navigation => MessageId::HelpSectionNavigation,
            Self::Editing => MessageId::HelpSectionEditing,
            Self::Submission => MessageId::HelpSectionActions,
            Self::Modes => MessageId::HelpSectionModes,
            Self::Sessions => MessageId::HelpSectionSessions,
            Self::Clipboard => MessageId::HelpSectionClipboard,
            Self::Help => MessageId::HelpSectionHelp,
        };
        tr(locale, id)
    }

    /// 帮助渲染的稳定排序 —— 匹配变体声明顺序；
    /// 显式指定，以便添加部分时强制审慎定位。
    pub fn rank(self) -> u8 {
        match self {
            Self::Navigation => 0,
            Self::Editing => 1,
            Self::Submission => 2,
            Self::Modes => 3,
            Self::Sessions => 4,
            Self::Clipboard => 5,
            Self::Help => 6,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct KeybindingEntry {
    pub chord: &'static str,
    pub description_id: crate::localization::MessageId,
    pub section: KeybindingSection,
}

/// 帮助覆盖层中显示的键绑定的规范列表。
///
/// 字符串使用现有帮助屏幕使用的相同表示法编写，以便
/// 阅读者可以与文档交叉引用：`Ctrl+X`、`Alt+X`、
/// `Shift+X`、`↑/↓`、`PgUp/PgDn` 等。帮助渲染器可能在渲染时应用
/// 平台特定的替换（例如 macOS 上 Alt 的 `⌥`），但目录本身
/// 存储可移植形式。
pub const KEYBINDINGS: &[KeybindingEntry] = &[
    // --- 导航 ---
    KeybindingEntry {
        chord: "↑ / ↓",
        description_id: crate::localization::MessageId::KbScrollTranscript,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "Ctrl+↑ / Ctrl+↓",
        description_id: crate::localization::MessageId::KbNavigateHistory,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "Alt+↑ / Alt+↓",
        description_id: crate::localization::MessageId::KbScrollTranscriptAlt,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "Shift+↑ / Shift+↓",
        description_id: crate::localization::MessageId::KbBrowseHistory,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "PgUp / PgDn",
        description_id: crate::localization::MessageId::KbScrollPage,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "Ctrl+Home / Ctrl+End",
        description_id: crate::localization::MessageId::KbJumpTopBottom,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "g / G",
        description_id: crate::localization::MessageId::KbJumpTopBottomEmpty,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "[ / ]",
        description_id: crate::localization::MessageId::KbJumpToolBlocks,
        section: KeybindingSection::Navigation,
    },
    // --- 编辑 ---
    KeybindingEntry {
        chord: "← / →",
        description_id: crate::localization::MessageId::KbMoveCursor,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Home / End",
        description_id: crate::localization::MessageId::KbJumpLineStartEnd,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Ctrl+A / Ctrl+E",
        description_id: crate::localization::MessageId::KbJumpLineStartEnd,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Backspace / Delete",
        description_id: crate::localization::MessageId::KbDeleteChar,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Ctrl+U",
        description_id: crate::localization::MessageId::KbClearDraft,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Ctrl+S",
        description_id: crate::localization::MessageId::KbStashDraft,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Alt+R",
        description_id: crate::localization::MessageId::KbSearchHistory,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Ctrl+J / Alt+Enter / Shift+Enter",
        description_id: crate::localization::MessageId::KbInsertNewline,
        section: KeybindingSection::Editing,
    },
    // --- 提交/操作 ---
    KeybindingEntry {
        chord: "Enter",
        description_id: crate::localization::MessageId::KbSendDraft,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Esc",
        description_id: crate::localization::MessageId::KbCloseMenu,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+C",
        description_id: crate::localization::MessageId::KbCancelOrExit,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+B",
        description_id: crate::localization::MessageId::KbShellControls,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+D",
        description_id: crate::localization::MessageId::KbExitEmpty,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+K",
        description_id: crate::localization::MessageId::KbCommandPalette,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+X (Activity sidebar)",
        description_id: crate::localization::MessageId::KbCancelBackgroundShellJobs,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+P",
        description_id: crate::localization::MessageId::KbFuzzyFilePicker,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Alt+C",
        description_id: crate::localization::MessageId::KbCompactInspector,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "l",
        description_id: crate::localization::MessageId::KbLastMessagePager,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "v",
        description_id: crate::localization::MessageId::KbSelectedDetails,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+O",
        description_id: crate::localization::MessageId::KbThinkingPager,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+Shift+T",
        description_id: crate::localization::MessageId::KbLiveTranscript,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+T",
        description_id: crate::localization::MessageId::KbCycleThinking,
        section: KeybindingSection::Modes,
    },
    KeybindingEntry {
        chord: "Esc Esc",
        description_id: crate::localization::MessageId::KbBacktrackMessage,
        section: KeybindingSection::Submission,
    },
    // --- 模式 ---
    KeybindingEntry {
        chord: "Tab",
        description_id: crate::localization::MessageId::KbCompleteCycleModes,
        section: KeybindingSection::Modes,
    },
    KeybindingEntry {
        chord: "Shift+Tab",
        description_id: crate::localization::MessageId::KbCyclePermissions,
        section: KeybindingSection::Modes,
    },
    KeybindingEntry {
        chord: "Alt+1-8",
        description_id: crate::localization::MessageId::KbJumpPlanAgentYolo,
        section: KeybindingSection::Modes,
    },
    KeybindingEntry {
        chord: "Alt+P / Alt+A / Alt+Y",
        description_id: crate::localization::MessageId::KbAltJumpPlanAgentYolo,
        section: KeybindingSection::Modes,
    },
    KeybindingEntry {
        chord: "Alt+! / Alt+@ / Alt+# / Alt+$ / Alt+0 / Ctrl+Alt+0",
        description_id: crate::localization::MessageId::KbFocusSidebar,
        section: KeybindingSection::Modes,
    },
    // --- 会话 ---
    KeybindingEntry {
        chord: "Ctrl+R",
        description_id: crate::localization::MessageId::KbSessionPicker,
        section: KeybindingSection::Sessions,
    },
    // --- 剪贴板 ---
    KeybindingEntry {
        chord: "Ctrl+V",
        description_id: crate::localization::MessageId::KbPasteAttach,
        section: KeybindingSection::Clipboard,
    },
    KeybindingEntry {
        chord: "Ctrl+Shift+C",
        description_id: crate::localization::MessageId::KbCopySelection,
        section: KeybindingSection::Clipboard,
    },
    KeybindingEntry {
        chord: "Right click",
        description_id: crate::localization::MessageId::KbContextMenu,
        section: KeybindingSection::Clipboard,
    },
    KeybindingEntry {
        chord: "@path",
        description_id: crate::localization::MessageId::KbAttachPath,
        section: KeybindingSection::Clipboard,
    },
    // --- 帮助 ---
    KeybindingEntry {
        chord: "?",
        description_id: crate::localization::MessageId::KbHelpOverlay,
        section: KeybindingSection::Help,
    },
    KeybindingEntry {
        chord: "F1",
        description_id: crate::localization::MessageId::KbToggleHelp,
        section: KeybindingSection::Help,
    },
    KeybindingEntry {
        chord: "Ctrl+/",
        description_id: crate::localization::MessageId::KbToggleHelp,
        section: KeybindingSection::Help,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_non_empty_and_sections_have_entries() {
        assert!(KEYBINDINGS.iter().any(|entry| !entry.chord.is_empty()));
        // 每个声明的部分应至少在目录中出现一次，
        // 否则帮助覆盖层将渲染空标题。
        let sections = [
            KeybindingSection::Navigation,
            KeybindingSection::Editing,
            KeybindingSection::Submission,
            KeybindingSection::Modes,
            KeybindingSection::Sessions,
            KeybindingSection::Clipboard,
            KeybindingSection::Help,
        ];
        for section in sections {
            assert!(
                KEYBINDINGS.iter().any(|entry| entry.section == section),
                "no entries for section {section:?}"
            );
        }
    }

    #[test]
    fn help_section_documents_question_mark() {
        // #93 的全部意义在于 `?` 打开此覆盖层；如果条目
        // 曾经消失，用户面向的可发现性承诺就被打破了。
        assert!(
            KEYBINDINGS
                .iter()
                .any(|entry| entry.chord.contains('?') && entry.section == KeybindingSection::Help),
            "`?` must remain documented as the help-toggle chord"
        );
    }

    #[test]
    fn ctrl_o_help_copy_matches_turn_inspector_behavior() {
        let ctrl_o = KEYBINDINGS
            .iter()
            .find(|entry| entry.chord == "Ctrl+O")
            .expect("Ctrl+O keybinding should be documented");

        // Ctrl+O 现在打开整个回合的 Turn Inspector（#4104），而不是
        // 单个单元格的 Activity Detail。消息 ID 有意保留
        // （`KbThinkingPager`）以避免重命名现有符号；只有
        // 文案改变。
        assert_eq!(
            ctrl_o.description_id,
            crate::localization::MessageId::KbThinkingPager
        );
        assert_eq!(
            crate::localization::tr(crate::localization::Locale::En, ctrl_o.description_id,),
            "Open Turn Inspector"
        );
    }

    #[test]
    fn ctrl_x_activity_sidebar_cancel_all_is_documented() {
        let ctrl_x_activity = KEYBINDINGS
            .iter()
            .find(|entry| entry.chord == "Ctrl+X (Activity sidebar)")
            .expect("Ctrl+X Activity sidebar keybinding should be documented");

        assert_eq!(
            ctrl_x_activity.description_id,
            crate::localization::MessageId::KbCancelBackgroundShellJobs
        );
    }

    #[test]
    fn tool_details_help_documents_bare_v_without_alt_v() {
        let selected_details = KEYBINDINGS
            .iter()
            .filter(|entry| {
                entry.description_id == crate::localization::MessageId::KbSelectedDetails
            })
            .map(|entry| entry.chord)
            .collect::<Vec<_>>();

        assert_eq!(selected_details, vec!["v"]);
        let legacy_modified_details_chord = ["Alt", "V"].join("+");
        assert!(
            KEYBINDINGS
                .iter()
                .all(|entry| entry.chord != legacy_modified_details_chord),
            "help should advertise the bare v details shortcut"
        );
    }

    #[test]
    fn section_rank_is_a_total_order() {
        let sections = [
            KeybindingSection::Navigation,
            KeybindingSection::Editing,
            KeybindingSection::Submission,
            KeybindingSection::Modes,
            KeybindingSection::Sessions,
            KeybindingSection::Clipboard,
            KeybindingSection::Help,
        ];
        let mut ranks: Vec<u8> = sections.iter().map(|s| s.rank()).collect();
        ranks.sort_unstable();
        ranks.dedup();
        assert_eq!(ranks.len(), sections.len(), "ranks must be unique");
    }
}
