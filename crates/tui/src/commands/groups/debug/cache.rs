//! `/cache` 命令——每轮前缀缓存遥测和检查。

use std::time::Instant;

use super::CommandResult;
use crate::client::{CacheWarmupKey, PromptInspection, inspect_prompt_for_request};
use crate::localization::{Locale, MessageId, tr};
use crate::models::MessageRequest;
use crate::tui::app::{App, AppAction, TurnCacheRecord};

/// 显示最后 N 轮的每轮 DeepSeek 前缀缓存遥测（#263）。
///
/// `arg` 被解析为数量覆盖（默认 10，上限为环形缓冲区大小）。
/// 渲染用户可粘贴到错误报告中的固定宽度表格。
pub fn cache(app: &mut App, arg: Option<&str>) -> CommandResult {
    let arg = arg.map(str::trim).filter(|s| !s.is_empty());
    if let Some(flags) = arg.and_then(|a| a.strip_prefix("inspect")) {
        let flags = flags.trim();
        let verbose = flags.split_whitespace().any(|flag| flag == "--verbose");
        let json_mode = flags.split_whitespace().any(|flag| flag == "--json");
        return CommandResult::message(format_cache_inspect(app, verbose, json_mode));
    }
    if matches!(arg, Some("warmup")) {
        return CommandResult::action(AppAction::CacheWarmup);
    }
    if matches!(arg, Some("stats")) {
        return CommandResult::message(format_cache_stats(app));
    }
    if matches!(arg, Some("zones")) {
        return CommandResult::message(format_cache_zones(app));
    }

    let want = arg.and_then(|s| s.parse::<usize>().ok()).unwrap_or(10);
    let cap = app.session.turn_cache_history.len();
    let count = want
        .min(cap)
        .min(crate::tui::app::App::TURN_CACHE_HISTORY_CAP);

    if cap == 0 {
        return CommandResult::message(tr(app.ui_locale, MessageId::CmdCacheNoData));
    }

    CommandResult::message(format_cache_history(app, count, app.ui_locale))
}

fn format_cache_inspect(app: &mut App, verbose: bool, json_mode: bool) -> String {
    if verbose && json_mode {
        return "cache inspect: --json and --verbose cannot be combined".to_string();
    }

    let reasoning_effort = if app.reasoning_effort == crate::tui::app::ReasoningEffort::Auto {
        app.last_effective_reasoning_effort
            .and_then(|effort| effort.api_value_for_provider(app.api_provider))
            .map(str::to_string)
    } else {
        app.reasoning_effort
            .api_value_for_provider(app.api_provider)
            .map(str::to_string)
    };
    let request = MessageRequest {
        model: app.model.clone(),
        messages: app.api_messages.clone(),
        max_tokens: 0,
        system: app.system_prompt.clone(),
        tools: app.session.last_tool_catalog.clone(),
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort,
        stream: Some(true),
        temperature: None,
        top_p: None,
    };
    let inspection = inspect_prompt_for_request(&request);
    let previous = app.session.last_cache_inspection.as_ref();
    let current_warmup_key = CacheWarmupKey::from_inspection(
        &format!("{:?}", app.api_provider),
        &app.model,
        app.session.last_base_url.as_deref().unwrap_or_default(),
        &inspection,
    );
    let warmup_status =
        format_warmup_status(app.session.last_warmup_key.as_ref(), &current_warmup_key);
    if json_mode {
        let output = serde_json::to_value(&inspection)
            .and_then(|mut value| {
                if let serde_json::Value::Object(ref mut object) = value {
                    object.insert(
                        "current_warmup_key".to_string(),
                        serde_json::to_value(&current_warmup_key)?,
                    );
                    object.insert(
                        "warmup_status".to_string(),
                        serde_json::Value::String(warmup_status.trim_end().to_string()),
                    );
                }
                serde_json::to_string_pretty(&value)
            })
            .unwrap_or_else(|_| {
                "{\"error\":\"cache inspection serialization failed\"}".to_string()
            });
        app.session.last_cache_inspection = Some(inspection);
        return output;
    }

    let mut out = String::new();
    out.push_str("Cache Inspect\n");
    out.push_str("Full prompt text is not printed. Hashes are SHA-256 of each rendered layer.\n");
    out.push_str(&format!(
        "Base static prefix hash: {}\n",
        inspection.base_static_prefix_hash
    ));
    out.push_str(&format!(
        "Full request prefix hash: {}\n",
        inspection.full_request_prefix_hash
    ));
    out.push_str(&format!(
        "Tool catalog hash: {}\n",
        if inspection.tool_catalog_hash.is_empty() {
            "(no tools registered)".to_string()
        } else {
            inspection.tool_catalog_hash.clone()
        }
    ));
    out.push_str(&format_static_prefix_status(previous, &inspection));
    out.push_str(&format_first_divergence(previous, &inspection));
    out.push_str(&warmup_status);
    let total_tokens: usize = inspection
        .layers
        .iter()
        .map(|layer| layer.token_estimate)
        .sum();
    out.push_str(&format!("Estimated reusable tokens: ~{total_tokens}\n"));
    out.push('\n');

    for layer in &inspection.layers {
        let mut line = format!(
            "{}: {}, chars={}, bytes={}, ~{}tok, hash={}\n",
            layer.name,
            layer.stability.label(),
            layer.char_len,
            layer.byte_len,
            layer.token_estimate,
            layer.sha256
        );
        if let Some(tool_result) = &layer.tool_result {
            let trimmed = line.trim_end_matches('\n').to_string();
            line = format!(
                "{trimmed}, original_chars={}, sent_chars={}, truncated={}, deduplicated={}\n",
                tool_result.original_chars,
                tool_result.sent_chars,
                tool_result.truncated,
                tool_result.deduplicated
            );
        }
        if let Some(turn_meta) = &layer.turn_meta {
            let trimmed = line.trim_end_matches('\n').to_string();
            line = format!(
                "{trimmed}, turn_meta_original_chars={}, turn_meta_sent_chars={}, turn_meta_deduplicated={}, turn_meta_sha256={}\n",
                turn_meta.original_chars,
                turn_meta.sent_chars,
                turn_meta.deduplicated,
                turn_meta.sha256
            );
        }
        out.push_str(&line);
    }
    if verbose {
        out.push_str("\nVerbose diff\n");
        if let Some(previous) = previous {
            out.push_str(&format_verbose_diff(previous, &inspection));
        } else {
            out.push_str("No previous inspection to compare against.\n");
        }
    }
    app.session.last_cache_inspection = Some(inspection);
    out
}

pub(crate) fn format_warmup_status(
    last_warmup: Option<&CacheWarmupKey>,
    current: &CacheWarmupKey,
) -> String {
    match last_warmup {
        None => format!(
            "Warmup status: no previous warmup (current key: {})\n",
            current.hash_short()
        ),
        Some(previous) if previous == current => {
            format!(
                "Warmup status: valid (key {} matches)\n",
                current.hash_short()
            )
        }
        Some(previous) => {
            let mut reasons = Vec::new();
            if previous.provider != current.provider {
                reasons.push("provider changed");
            }
            if previous.model != current.model {
                reasons.push("model changed");
            }
            if previous.base_url != current.base_url {
                reasons.push("base URL changed");
            }
            if previous.static_prefix_hash != current.static_prefix_hash {
                reasons.push("static prefix changed");
            }
            if previous.tool_catalog_hash != current.tool_catalog_hash {
                reasons.push("tool catalog changed");
            }
            if previous.project_pack_hash != current.project_pack_hash {
                reasons.push("project pack changed");
            }
            if previous.skills_hash != current.skills_hash {
                reasons.push("skills changed");
            }
            let reason_text = if reasons.is_empty() {
                "unknown prefix input changed".to_string()
            } else {
                reasons.join(", ")
            };
            format!(
                "Warmup status: invalid ({} -> {}; {})\n",
                previous.hash_short(),
                current.hash_short(),
                reason_text
            )
        }
    }
}

fn format_verbose_diff(previous: &PromptInspection, current: &PromptInspection) -> String {
    let mut out = String::new();
    let max_len = previous.layers.len().max(current.layers.len());
    for index in 0..max_len {
        match (previous.layers.get(index), current.layers.get(index)) {
            (Some(prev), Some(curr)) if prev == curr => {
                out.push_str(&format!("  [{index}] {} unchanged\n", curr.name));
            }
            (Some(prev), Some(curr)) => {
                out.push_str(&format!("  [{index}] {} changed\n", curr.name));
                if prev.name != curr.name {
                    out.push_str(&format!("    name: {} -> {}\n", prev.name, curr.name));
                }
                if prev.stability != curr.stability {
                    out.push_str(&format!(
                        "    stability: {} -> {}\n",
                        prev.stability.label(),
                        curr.stability.label()
                    ));
                }
                if prev.char_len != curr.char_len {
                    out.push_str(&format!(
                        "    chars: {} -> {} ({:+})\n",
                        prev.char_len,
                        curr.char_len,
                        curr.char_len as i64 - prev.char_len as i64
                    ));
                }
                if prev.sha256 != curr.sha256 {
                    out.push_str(&format!(
                        "    hash: {} -> {}\n",
                        short_hash(&prev.sha256),
                        short_hash(&curr.sha256)
                    ));
                }
            }
            (None, Some(curr)) => {
                out.push_str(&format!("  [{index}] {} added\n", curr.name));
            }
            (Some(prev), None) => {
                out.push_str(&format!("  [{index}] {} removed\n", prev.name));
            }
            (None, None) => unreachable!("index is within max_len"),
        }
    }
    out
}

fn short_hash(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

/// 渲染 `/cache stats` 的前缀缓存稳定性和健康摘要。
///
/// 显示当前前缀指纹、稳定性比率、变更历史以及从每轮遥测
/// 聚合的缓存命中摘要。当前缀已更改时，包含显着的警告，
/// 以便用户可以将缓存未命中与前缀漂移相关联。
fn format_cache_stats(app: &App) -> String {
    let mut out = String::new();
    out.push_str("Cache Stats\n");

    // ── 前缀稳定性 ──────────────────────────────────────────────
    out.push_str("\n── 前缀稳定性\n");
    match app.prefix_stability_pct {
        Some(pct) => {
            let checks = app.prefix_checks_total;
            let changes = app.prefix_change_count;
            let stable_checks = checks.saturating_sub(changes);

            if changes == 0 {
                out.push_str(&format!(
                    "  稳定性: {pct}% ({stable_checks}/{checks} 次检查)\n"
                ));
                out.push_str("  状态:    稳定（此会话中无前缀变更）\n");
            } else {
                out.push_str(&format!(
                    "  稳定性: {pct}% ({stable_checks}/{checks} 次检查，{changes} 次变更)\n",
                ));
                out.push_str("  状态:    警告——前缀已变更\n");
                if let Some(ref desc) = app.last_prefix_change_desc {
                    out.push_str(&format!("  上次变更: {desc}\n"));
                }
            }
        }
        None => {
            out.push_str("  稳定性: 未知（尚未记录检查）\n");
            out.push_str("  先运行一轮以收集前缀稳定性数据。\n");
        }
    }

    // ── 前缀指纹 ────────────────────────────────────────────
    out.push_str("\n── 前缀指纹\n");
    match &app.last_pinned_prefix_hash {
        Some(hash) => {
            out.push_str(&format!("  固定哈希: {hash}\n"));
            let short = if hash.len() >= 12 { &hash[..12] } else { hash };
            out.push_str(&format!("  短 ID:    {short}\n"));
            if app.prefix_change_count > 0 {
                out.push_str("  漂移:     警告——哈希在此会话期间已变更\n");
                out.push_str(&format!(
                    "               （检测到 {change} 次变更）\n",
                    change = app.prefix_change_count,
                ));
            } else {
                out.push_str("  漂移:     无（哈希稳定）\n");
            }
        }
        None => {
            out.push_str("  固定哈希: 不可用\n");
            out.push_str("  先运行一轮，或使用 /cache inspect。\n");
        }
    }

    // ── 缓存命中率摘要 ────────────────────────────────────────
    out.push_str("\n── 缓存命中率\n");
    let history = &app.session.turn_cache_history;
    if history.is_empty() {
        out.push_str("  尚未记录轮次遥测。\n");
    } else {
        // 仅聚合启用缓存的轮次；跳过提供者未报告缓存遥测的轮次
        //（cache_hit_tokens 为 None）。
        // 当 cache_miss_tokens 为 None 时，推断为
        //   input_tokens − cache_hit_tokens（匹配 /cache 表逻辑）。
        let mut turns = 0u64;
        let (hit, miss, input) = app.session.turn_cache_history.iter().fold(
            (0u64, 0u64, 0u64),
            |(hit, miss, input), rec| {
                let Some(hit_tokens) = rec.cache_hit_tokens else {
                    return (hit, miss, input);
                };
                let h = u64::from(hit_tokens);
                let m = u64::from(
                    rec.cache_miss_tokens
                        .unwrap_or(rec.input_tokens.saturating_sub(hit_tokens)),
                );
                turns += 1;
                (hit + h, miss + m, input + u64::from(rec.input_tokens))
            },
        );
        let total_cache = hit + miss;
        let avg_pct = if total_cache > 0 {
            (hit as f64 / total_cache as f64 * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        out.push_str(&format!("  记录轮次: {turns}\n"));
        out.push_str(&format!(
            "  缓存命中 token:   {hit}（占 {total_cache} 个缓存感知 token 的 {avg_pct:.1}%）\n",
            hit = format_tokens(hit),
            total_cache = format_tokens(total_cache),
        ));
        out.push_str(&format!(
            "  缓存未命中 token: {miss}\n",
            miss = format_tokens(miss),
        ));
        out.push_str(&format!(
            "  总输入 token: {input}\n",
            input = format_tokens(input),
        ));
        if avg_pct < 80.0 {
            out.push_str("  注意：缓存命中率偏低（< 80%）。请检查上方的前缀稳定性或考虑 /compact。\n");
        }
    }

    out
}

/// 渲染 `/cache zones` 的三区域前缀契约状态（#2264）。
///
/// 显示 PinnedPrefix 指纹、AppendLog 大小和 TurnScratch
/// 状态。这些区域仅为类型脚手架（阶段 1）——尚未在请求时
/// 强制执行完整契约。
fn format_cache_zones(app: &App) -> String {
    let mut out = String::new();
    out.push_str("Cache Zones (#2264 three-zone contract, Phase 1 foundation)\n");

    // ── PinnedPrefix ─────────────────────────────────────────────────
    out.push_str("\n── PinnedPrefix（系统 + 工具，冻结基线）\n");
    match &app.last_pinned_prefix_hash {
        Some(hash) => {
            let short = if hash.len() >= 12 { &hash[..12] } else { hash };
            out.push_str(&format!("  短 ID: {short}\n"));
            if app.prefix_change_count > 0 {
                out.push_str(&format!(
                    "  状态:    警告——检测到 {change} 次漂移\n",
                    change = app.prefix_change_count,
                ));
            } else {
                out.push_str("  状态:    稳定（此会话中无漂移）\n");
            }
            if let Some(pct) = app.prefix_stability_pct {
                out.push_str(&format!("  稳定性: {pct}%\n"));
            }
        }
        None => {
            out.push_str("  状态:    不可用（尚未冻结）\n");
            out.push_str("  先运行一轮以冻结基线。\n");
        }
    }

    // ── AppendLog ────────────────────────────────────────────────────
    out.push_str("\n── AppendLog（对话历史，仅追加）\n");
    out.push_str("  状态:      阶段 1 脚手架——尚未接入引擎\n");
    let msg_count = app.api_messages.len();
    out.push_str(&format!("  消息数:    {msg_count}\n"));
    let history_count = app
        .api_messages
        .iter()
        .filter(|m| m.role != "system")
        .count();
    out.push_str(&format!("  历史消息数: {history_count}\n"));

    // ── TurnScratch ──────────────────────────────────────────────────
    out.push_str("\n── TurnScratch（每轮临时数据）\n");
    out.push_str("  状态:      阶段 1 脚手架——尚未接入引擎\n");

    // ── 区域契约摘要 ────────────────────────────────────────
    out.push_str("\n── 契约状态\n");
    let has_drift = app.prefix_change_count > 0;
    out.push_str(&format!(
        "  PinnedPrefix: {}\n",
        if app.last_pinned_prefix_hash.is_some() {
            if has_drift {
                "警告——已漂移"
            } else {
                "OK"
            }
        } else {
            "未冻结"
        }
    ));
    out.push_str("  AppendLog:    阶段 1 基础\n");
    out.push_str("  TurnScratch:  阶段 1 基础\n");

    out
}

/// 使用紧凑后缀格式化 u64 token 计数：K 表示千，M 表示百万。从不返回科学计数法。
pub(crate) fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_static_prefix_status(
    previous: Option<&PromptInspection>,
    current: &PromptInspection,
) -> String {
    let Some(previous) = previous else {
        return "Static base prefix stability: no previous request\n".to_string();
    };
    if previous.base_static_prefix_hash == current.base_static_prefix_hash {
        return "Static base prefix stability: OK\n".to_string();
    }

    let changed = changed_static_layers(previous, current);
    if changed.is_empty() {
        "Static base prefix stability: WARNING (base hash changed)\n".to_string()
    } else {
        format!(
            "Static base prefix stability: WARNING changed layers: {}\n",
            changed.join(", ")
        )
    }
}

fn format_first_divergence(
    previous: Option<&PromptInspection>,
    current: &PromptInspection,
) -> String {
    let Some(previous) = previous else {
        return "First divergence from previous request: unavailable\n".to_string();
    };
    let max_len = previous.layers.len().max(current.layers.len());
    for index in 0..max_len {
        match (previous.layers.get(index), current.layers.get(index)) {
            (Some(prev), Some(curr)) if prev.name == curr.name && prev.sha256 == curr.sha256 => {}
            (Some(prev), Some(curr)) if prev.name == curr.name => {
                return format!("First divergence from previous request: {}\n", curr.name);
            }
            (Some(_), Some(curr)) => {
                return format!("First divergence from previous request: {}\n", curr.name);
            }
            (None, Some(curr)) => {
                return format!("First divergence from previous request: {}\n", curr.name);
            }
            (Some(prev), None) => {
                return format!(
                    "First divergence from previous request: {} removed\n",
                    prev.name
                );
            }
            (None, None) => break,
        }
    }
    "First divergence from previous request: none\n".to_string()
}

fn changed_static_layers(previous: &PromptInspection, current: &PromptInspection) -> Vec<String> {
    current
        .layers
        .iter()
        .filter(|layer| layer.stability.label() == "static")
        .filter(|layer| {
            previous
                .layers
                .iter()
                .find(|previous_layer| previous_layer.name == layer.name)
                .is_none_or(|previous_layer| previous_layer.sha256 != layer.sha256)
        })
        .map(|layer| layer.name.clone())
        .collect()
}

fn format_cache_history(app: &App, count: usize, locale: Locale) -> String {
    let total = app.session.turn_cache_history.len();
    let start = total.saturating_sub(count);
    let rows: Vec<&TurnCacheRecord> = app.session.turn_cache_history.iter().skip(start).collect();

    let mut totals_input: u64 = 0;
    let mut totals_hit: u64 = 0;
    let mut totals_miss: u64 = 0;
    let mut header = tr(locale, MessageId::CmdCacheHeader)
        .replace("{count}", &rows.len().to_string())
        .replace("{total}", &total.to_string())
        .replace("{model}", &app.model);
    header.push_str(&"─".repeat(96));
    header.push('\n');
    header.push_str(
        "turn  route                       in    out    hit   miss  replay   ratio   age\n",
    );
    header.push_str(&"─".repeat(96));
    header.push('\n');

    let now = Instant::now();
    let mut body = String::new();
    let absolute_start = total.saturating_sub(rows.len());
    for (i, rec) in rows.iter().enumerate() {
        let turn_index = absolute_start + i + 1;
        totals_input += u64::from(rec.input_tokens);

        let replay_cell = rec
            .reasoning_replay_tokens
            .map_or_else(|| "—".to_string(), |t| t.to_string());
        let route_cell = format_turn_cache_route(rec);
        let age = humanize_age(now.saturating_duration_since(rec.recorded_at));

        // 无缓存遥测 → 所有地方渲染 `—` 且不污染总计为推断的零。
        // 某些提供者（以及 DeepSeek 内的某些路由）跳过缓存字段；
        // 为这些轮次包含合成的 0/N 会使每个聚合比率看起来损坏。
        let Some(hit) = rec.cache_hit_tokens else {
            body.push_str(&format!(
                "{turn:>4}  {route:<24}  {input:>5}  {output:>5}  {hit:>5}  {miss:>5}  {replay:>6}   {ratio:>6}   {age}\n",
                turn = turn_index,
                route = route_cell,
                input = rec.input_tokens,
                output = rec.output_tokens,
                hit = "—",
                miss = "—",
                replay = replay_cell,
                ratio = "—",
                age = age,
            ));
            continue;
        };

        let miss_reported = rec.cache_miss_tokens;
        let miss = miss_reported.unwrap_or_else(|| rec.input_tokens.saturating_sub(hit));
        let accounted = u64::from(hit) + u64::from(miss);
        let ratio = if accounted == 0 {
            "    —".to_string()
        } else {
            format!("{:>5.1}%", 100.0 * f64::from(hit) / accounted as f64)
        };
        totals_hit += u64::from(hit);
        totals_miss += u64::from(miss);

        let miss_cell = match miss_reported {
            Some(_) => format!("{miss}"),
            None => format!("{miss}*"),
        };

        body.push_str(&format!(
            "{turn:>4}  {route:<24}  {input:>5}  {output:>5}  {hit:>5}  {miss:>5}  {replay:>6}   {ratio}   {age}\n",
            turn = turn_index,
            route = route_cell,
            input = rec.input_tokens,
            output = rec.output_tokens,
            hit = hit,
            miss = miss_cell,
            replay = replay_cell,
            ratio = ratio,
            age = age,
        ));
    }

    let totals_accounted = totals_hit + totals_miss;
    let avg_ratio = if totals_accounted == 0 {
        "—".to_string()
    } else {
        format!(
            "{:.1}%",
            100.0 * totals_hit as f64 / totals_accounted as f64
        )
    };

    let mut footer = String::new();
    footer.push_str(&"─".repeat(96));
    footer.push('\n');
    footer.push_str(
        &tr(locale, MessageId::CmdCacheTotals)
            .replace("{sum_in}", &totals_input.to_string())
            .replace("{sum_hit}", &totals_hit.to_string())
            .replace("{sum_miss}", &totals_miss.to_string())
            .replace("{avg}", &avg_ratio),
    );
    footer.push_str(&tr(locale, MessageId::CmdCacheFootnote));
    footer.push_str(&tr(locale, MessageId::CmdCacheAdvice));

    format!("{header}{body}{footer}")
}

fn format_turn_cache_route(rec: &TurnCacheRecord) -> String {
    let Some(model) = rec.model.as_deref().filter(|model| !model.is_empty()) else {
        return "—".to_string();
    };
    let provider = rec
        .provider
        .map(|provider| provider.as_str())
        .unwrap_or("?");
    let route = if rec.auto_model {
        format!("auto:{provider}/{model}")
    } else {
        format!("{provider}/{model}")
    };
    truncate_route_cell(&route, 24)
}

fn truncate_route_cell(route: &str, max_chars: usize) -> String {
    if route.chars().count() <= max_chars {
        return route.to_string();
    }
    if max_chars <= 3 {
        return route.chars().take(max_chars).collect();
    }
    let mut out: String = route.chars().take(max_chars - 3).collect();
    out.push_str("...");
    out
}

fn humanize_age(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}
