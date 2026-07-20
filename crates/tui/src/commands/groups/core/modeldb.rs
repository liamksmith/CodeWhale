//! `/modeldb` 命令——浏览事实模型参考数据库。
//!
//! 打开一个只读分页器，列出每个目录模型的声明属性：
//! 供应商 + 类型、模型 ID（原样）、上下文窗口、最大输出、
//! 模态（文本 vs 多模态）和价格。这仅显示标签——从不
//! 选择、路由或分级模型（#3205, #2300）。目录中未声明的
//! 属性显示为 `unknown`，绝不猜测。

use codewhale_config::model_reference::ModelReferenceDatabase;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;
use crate::tui::pager::PagerView;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "modeldb",
    aliases: &["model-reference", "modelref"],
    usage: "/modeldb",
    description_id: MessageId::CmdModelDbDescription,
};

pub(in crate::commands) struct ModelDbCmd;

impl RegisterCommand for ModelDbCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        let db = ModelReferenceDatabase::bundled();
        let title = format!(
            "模型参考表——{} 款产品 · {} 家供应商",
            db.len(),
            db.providers().len()
        );
        app.view_stack
            .push(PagerView::new(title, reference_lines(&db)));
        CommandResult::ok()
    }
}

/// 将参考数据库渲染为对齐的、可浏览的分页器行。
///
/// 卡片按供应商/类型标题分组（数据库已按
/// `(provider, model id)` 排序），因此每行只需显示模型级别的
/// 列。列宽在整个表格中计算，以保持对齐稳定。
fn reference_lines(db: &ModelReferenceDatabase) -> Vec<Line<'static>> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::styled(
        "打包的精选模型参考目录。".to_string(),
        bold,
    ));
    lines.push(Line::styled(
        "属性是声明的事实；\"unknown\"表示目录未声明（绝不猜测）。"
            .to_string(),
        dim,
    ));
    lines.push(Line::from(String::new()));

    if db.is_empty() {
        lines.push(Line::from("（目录中无模型）".to_string()));
        return lines;
    }

    let cards = db.cards();
    let id_w = cards
        .iter()
        .map(|card| card.model_id.chars().count())
        .chain(std::iter::once("模型 ID".len()))
        .max()
        .unwrap_or(8)
        .clamp(8, 46);
    let ctx_w = cards
        .iter()
        .map(|card| card.context_window_label().chars().count())
        .chain(std::iter::once("上下文".len()))
        .max()
        .unwrap_or(3);
    let out_w = cards
        .iter()
        .map(|card| card.max_output_label().chars().count())
        .chain(std::iter::once("最大输出".len()))
        .max()
        .unwrap_or(7);
    // "multimodal"（10）是最宽的可能的标签，超过"模态"。
    let mod_w = "multimodal".len();

    lines.push(Line::styled(
        format!(
            "  {}  {}  {}  {}  {}",
            pad("模型 ID", id_w),
            pad("上下文", ctx_w),
            pad("最大输出", out_w),
            pad("模态", mod_w),
            "价格（USD/Mtok）"
        ),
        bold,
    ));

    let mut current_provider: Option<&str> = None;
    for card in cards {
        if current_provider != Some(card.provider.as_str()) {
            lines.push(Line::from(String::new()));
            lines.push(Line::styled(
                format!(
                    "{}   ·   类型：{}",
                    card.provider,
                    card.provider_kind_label()
                ),
                bold,
            ));
            current_provider = Some(card.provider.as_str());
        }
        lines.push(Line::from(format!(
            "  {}  {}  {}  {}  {}",
            pad(&truncate_to(&card.model_id, id_w), id_w),
            pad(&card.context_window_label(), ctx_w),
            pad(&card.max_output_label(), out_w),
            pad(card.modality.as_str(), mod_w),
            card.price_label(),
        )));
    }

    lines
}

/// 将 `s` 左对齐到 `width` 显示列（按 `char` 计数）。
fn pad(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

/// 将 `s` 截断至最多 `width` 个字符，用 `…` 标记省略。
fn truncate_to(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count <= width {
        return s.to_string();
    }
    if width <= 1 {
        return s.chars().take(width).collect();
    }
    let mut truncated: String = s.chars().take(width - 1).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(db: &ModelReferenceDatabase) -> Vec<String> {
        reference_lines(db)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn bundled_reference_lists_models_with_factual_columns() {
        let db = ModelReferenceDatabase::bundled();
        let text = rendered(&db).join("\n");

        // 图例说明了诚实性契约。
        assert!(text.contains("never guessed"));
        // 列键存在。
        assert!(text.contains("模型 ID"));
        assert!(text.contains("模态"));
        assert!(text.contains("价格（USD/Mtok）"));
        // 一个供应商标题和一个原样的模型 ID 行。
        assert!(text.contains("类型：deepseek"));
        assert!(text.contains("deepseek-v4-pro"));
        // 声明的模态和一个诚实的 unknown 价格都存在。
        assert!(text.contains("text"));
        assert!(text.contains("unknown"));
        // 一个有价格的行显示具体费率。
        assert!(text.contains("$0.30 / $1.20 per Mtok"));
    }

    #[test]
    fn empty_database_renders_placeholder_not_a_crash() {
        let db = ModelReferenceDatabase::from_offerings(&[]);
        let text = rendered(&db).join("\n");
        assert!(text.contains("（目录中无模型）"));
    }

    #[test]
    fn pad_and_truncate_are_width_safe() {
        assert_eq!(pad("ab", 5), "ab   ");
        assert_eq!(pad("abcde", 3), "abcde");
        assert_eq!(truncate_to("short", 10), "short");
        assert_eq!(truncate_to("abcdefghij", 5), "abcd…");
    }
}
