//! 构建状态栏/底部 chips 和一次性信息消息的小型字符串辅助函数。
//!
//! 每个辅助函数都是对 `App` 或响应数据的小切片的纯函数。
//! 集中在这里，这样 composer/footer 渲染器就不需要滚动过它们的主体，
//! 并且标签可以独立地进行单元测试。

use crate::models::Usage;
use crate::tui::app::App;

/// 构建前缀缓存预热回合完成后显示的多行"Cache warmup complete: …"状态消息。
/// 处理 `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens`
/// 存在或缺失的所有四种组合，这样我们就不会在未上报遥测的 API 调用上
/// 报告"0% cache hit"。
pub(super) fn cache_warmup_result(usage: &Usage) -> String {
    let cache = match (
        usage.prompt_cache_hit_tokens,
        usage.prompt_cache_miss_tokens,
    ) {
        (Some(hit), Some(miss)) => format!("Cache warmup complete: hit {hit} | miss {miss}"),
        (Some(hit), None) => format!("Cache warmup complete: hit {hit} | miss unavailable"),
        (None, Some(miss)) => format!("Cache warmup complete: hit unavailable | miss {miss}"),
        (None, None) => "Cache warmup complete: cache telemetry unavailable".to_string(),
    };
    format!(
        "{cache}\nNote: the first warmup is usually a miss. Later requests that reuse the same stable prefix may hit the provider cache; a hit is not guaranteed."
    )
}

/// 为可选的 TUI 底部 footer chip 格式化前缀稳定性信息。
pub(super) fn prefix_stability_chip(app: &App) -> Option<(String, ratatui::style::Color)> {
    let pct = app.prefix_stability_pct?;
    let changes = app.prefix_change_count;

    let color = if changes == 0 {
        // 完全稳定：绿色
        ratatui::style::Color::Green
    } else if pct >= 95 {
        // 优秀：绿色
        ratatui::style::Color::Green
    } else if pct >= 80 {
        // 良好：黄色
        ratatui::style::Color::Yellow
    } else {
        // 较差：红色 — 缓存正在频繁变更
        ratatui::style::Color::Red
    };

    let label = if changes == 0 {
        format!("cache prefix {pct}%")
    } else {
        format!(
            "cache prefix {pct}% ({changes} change{})",
            if changes == 1 { "" } else { "s" }
        )
    };

    Some((label, color))
}

/// 为 `/models` / `models list` 渲染响应正文 — 当前模型会被标星，
/// 其他可用模型列在其下方。
pub(super) fn available_models_message(current_model: &str, models: &[String]) -> String {
    let mut lines = vec![format!("Available models ({})", models.len())];
    for model in models {
        if model == current_model {
            lines.push(format!("* {model} (current)"));
        } else {
            lines.push(format!("  {model}"));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_models_message_marks_current_model() {
        let models = vec![
            "deepseek-v4-pro".to_string(),
            "deepseek-v4-flash".to_string(),
        ];
        let msg = available_models_message("deepseek-v4-pro", &models);
        assert!(msg.contains("* deepseek-v4-pro (current)"), "got: {msg}");
        assert!(msg.contains("  deepseek-v4-flash"), "got: {msg}");
        assert!(msg.starts_with("Available models (2)"), "got: {msg}");
    }

    #[test]
    fn cache_warmup_result_handles_missing_telemetry() {
        let usage = Usage {
            prompt_cache_hit_tokens: None,
            prompt_cache_miss_tokens: None,
            ..Default::default()
        };
        let msg = cache_warmup_result(&usage);
        assert!(msg.contains("cache telemetry unavailable"), "got: {msg}");
    }
}
