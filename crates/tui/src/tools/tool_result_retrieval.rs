//! `retrieve_tool_result` - 选择性检索溢出的工具输出。
//!
//! 大型成功工具输出由 `tools::truncate` 溢出到
//! `~/.codewhale/tool_outputs/<tool-call-id>.txt`。该工具为模型提供了一种只读的、限定目录范围的方式，
//! 用于获取这些历史输出的摘要或切片，而无需将整个文件回放到每个后续请求中。

use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{
    ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, optional_str, optional_u64,
    required_str,
};

const DEFAULT_MAX_BYTES: usize = 8 * 1024;
const HARD_MAX_BYTES: usize = 128 * 1024;
const DEFAULT_LINE_COUNT: usize = 40;
const HARD_LINE_COUNT: usize = 500;
const DEFAULT_MAX_MATCHES: usize = 20;
const HARD_MAX_MATCHES: usize = 100;
const DEFAULT_CONTEXT_LINES: usize = 1;
const HARD_CONTEXT_LINES: usize = 5;

/// 检索先前溢出的工具结果的摘要或切片。
pub struct RetrieveToolResultTool;

#[async_trait]
impl ToolSpec for RetrieveToolResultTool {
    fn name(&self) -> &'static str {
        "retrieve_tool_result"
    }

    fn description(&self) -> &'static str {
        "Retrieve a previously spilled large tool result. Accepts a tool_call_id (`call_abc123`), artifact id (`art_call_abc123`), SHA reference (`sha:<64-hex>` or bare 64-hex from `<TOOL_RESULT_REF>`), relative filename (`call_abc123.txt`, `artifacts/art_call_abc123.txt`), or absolute path under ~/.codewhale. Modes: summary, head, tail, lines, query."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "ref": {
                    "type": "string",
                    "description": "Tool call id, artifact id (`art_<id>`), SHA ref (`sha:<64-hex>`), spillover filename, or absolute path under ~/.codewhale."
                },
                "mode": {
                    "type": "string",
                    "enum": ["summary", "head", "tail", "lines", "query"],
                    "description": "Retrieval mode. Defaults to summary."
                },
                "query": {
                    "type": "string",
                    "description": "Case-insensitive substring to search for when mode=query."
                },
                "lines": {
                    "type": "string",
                    "description": "Line selector for mode=lines, e.g. \"10\" or \"10-40\"."
                },
                "start_line": {
                    "type": "integer",
                    "description": "1-based first line for mode=lines."
                },
                "end_line": {
                    "type": "integer",
                    "description": "1-based final line for mode=lines."
                },
                "line_count": {
                    "type": "integer",
                    "description": "Number of lines for head/tail modes. Default 40, hard cap 500."
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Maximum bytes of excerpt text returned. Default 8192, hard cap 131072."
                },
                "max_matches": {
                    "type": "integer",
                    "description": "Maximum query matches or signal lines returned. Default 20, hard cap 100."
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Extra lines around each query match. Default 1, hard cap 5."
                }
            },
            "required": ["ref"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let reference = required_str(&input, "ref")?.trim();
        if reference.is_empty() {
            return Err(ToolError::invalid_input("ref cannot be empty"));
        }

        let mode = optional_str(&input, "mode")
            .unwrap_or("summary")
            .trim()
            .to_ascii_lowercase();
        let max_bytes = clamp_u64(
            optional_u64(&input, "max_bytes", DEFAULT_MAX_BYTES as u64),
            1,
            HARD_MAX_BYTES,
        );
        let path = resolve_spillover_reference(reference, &context.state_namespace)?;
        let content = fs::read_to_string(&path).map_err(|err| {
            ToolError::execution_failed(format!("failed to read {}: {err}", path.display()))
        })?;

        let lines: Vec<&str> = content.lines().collect();
        let payload = match mode.as_str() {
            "summary" => {
                build_summary_payload(reference, &path, &content, &lines, &input, max_bytes)
            }
            "head" => build_head_tail_payload(reference, &path, "head", &lines, &input, max_bytes),
            "tail" => build_head_tail_payload(reference, &path, "tail", &lines, &input, max_bytes),
            "lines" => build_lines_payload(reference, &path, &lines, &input, max_bytes)?,
            "query" => build_query_payload(reference, &path, &lines, &input, max_bytes)?,
            other => {
                return Err(ToolError::invalid_input(format!(
                    "unsupported mode `{other}` (expected summary, head, tail, lines, or query)"
                )));
            }
        };

        ToolResult::json(&payload).map_err(|err| {
            ToolError::execution_failed(format!("failed to serialize result: {err}"))
        })
    }
}

/// 将工具结果引用解析为具体的文件路径。
///
/// 接受六种形式：
/// 1. `tool_call_id` — 旧版溢出形式，`tool_outputs/` 下的 `<id>.txt`。
/// 2. `art_<id>` — 当前产物 ID，由 `apply_spillover_with_artifact` 写入。
///    优先尝试会话产物目录，回退到 `<id>.txt`（去除 `art_` 前缀），新旧命名均可工作。
/// 3. `sha:<64-hex>` 或裸 64-hex — 内容寻址的线路去重，`sha_<hex>.txt`。
/// 4. `tool_result:<x>` — `<x>` 是前缀后的上述任意一种形式。
/// 5. `artifacts/<file>.txt` 或 `<file>.txt` — 相对路径。
/// 6. CodeWhale 主目录下的绝对路径。
///
/// 未命中时的错误信息会枚举尝试过的形式，以便模型无需再次盲目猜测即可纠正方向。
fn resolve_spillover_reference(reference: &str, session_id: &str) -> Result<PathBuf, ToolError> {
    let root = crate::tools::truncate::spillover_root().ok_or_else(|| {
        ToolError::execution_failed("could not resolve ~/.codewhale/tool_outputs")
    })?;
    let root_canonical = root.canonicalize().ok();

    // 解析会话的 `artifacts/` 目录。
    // `session_artifact_absolute_path(sid, p)` 返回
    // `~/.codewhale/sessions/<sid>/<p>` — 因此传入字面量
    // `ARTIFACTS_DIR_NAME` ("artifacts") 即可获得真正的产物
    // 根目录。早期草稿传入 `Path::new(".")` 并调用
    // `.parent()`，结果定位高了一层（`<sid>` 而非
    // `<sid>/artifacts`），导致所有裸 `art_<id>` 引用静默失效
    // — 仅旧版溢出回退存活了下来。测试
    // `resolves_art_prefix_to_legacy_spillover_id` 掩盖了此问题，
    // 因为它只写了旧版溢出文件。新测试
    // `resolves_art_prefix_via_session_artifacts` 则覆盖了真实路径。
    let session_artifacts_root = if !session_id.is_empty() {
        crate::artifacts::session_artifact_absolute_path(
            session_id,
            std::path::Path::new(crate::artifacts::ARTIFACTS_DIR_NAME),
        )
    } else {
        None
    };
    let session_artifacts_root_canonical = session_artifacts_root
        .as_ref()
        .and_then(|p| p.canonicalize().ok());

    let trimmed = reference.trim();
    let stripped = trimmed
        .strip_prefix("tool_result:")
        .unwrap_or(trimmed)
        .trim();

    let mut tried: Vec<PathBuf> = Vec::new();
    let try_path = |candidate: PathBuf, tried: &mut Vec<PathBuf>| -> Option<PathBuf> {
        // 始终记录我们尝试过的路径，以便 `not_found` 诊断信息
        // 能枚举所有候选路径，即使其中某些路径的
        // `canonicalize` 返回 ENOENT。否则模型会看到
        // 无用的 "(no valid candidates derived from ref)" 行。
        tried.push(candidate.clone());

        // 在规范化之前拒绝叶子节点的符号链接，这样能够写入
        // `<sid>/artifacts/` 目录的攻击者就无法
        // 布置指向 `/etc/passwd` 的符号链接并通过
        // `retrieve_tool_result` 读取它。canonicalize() 会愉快地
        // 跟随这样的链接，然后由于"先解析后比较"的顺序，
        // `starts_with(root)` 检查也会通过。
        // 主目录级别的 `~/.codewhale/tool_outputs/` 目录仅供引擎使用，
        // 从未有此担忧；而会话产物目录中存放的是
        // 任意工具输出，需要这道防护。
        if let Ok(meta) = std::fs::symlink_metadata(&candidate)
            && meta.file_type().is_symlink()
        {
            return None;
        }

        let canonical = candidate.canonicalize().ok()?;
        if !canonical.is_file() {
            return None;
        }
        let inside_legacy = root_canonical
            .as_ref()
            .is_some_and(|root| canonical.starts_with(root));
        let inside_session = session_artifacts_root_canonical
            .as_ref()
            .is_some_and(|root| canonical.starts_with(root));
        if inside_legacy || inside_session {
            Some(canonical)
        } else {
            None
        }
    };

    // 形式 1/3：绝对路径。验证其位于允许的根目录之一下。
    let raw_path = PathBuf::from(stripped);
    if raw_path.is_absolute() {
        if let Some(found) = try_path(raw_path.clone(), &mut tried) {
            return Ok(found);
        }
        return Err(not_found(
            reference,
            &tried,
            &root,
            session_artifacts_root.as_deref(),
        ));
    }

    // 形式 4：`sha:<hex>` 前缀或裸 64-hex SHA → SHA 寻址的文件。
    let sha_candidate = stripped
        .strip_prefix("sha:")
        .or_else(|| stripped.strip_prefix("sha_"))
        .unwrap_or(stripped)
        .trim();
    if crate::tools::truncate::is_valid_sha256(&sha_candidate.to_ascii_lowercase())
        && let Some(p) = crate::tools::truncate::sha_spillover_path(sha_candidate)
        && let Some(found) = try_path(p, &mut tried)
    {
        return Ok(found);
    }

    // 形式 5：包含分隔符或 `.txt` 后缀的相对路径。
    let looks_like_path = stripped.ends_with(".txt")
        || stripped.contains('/')
        || (std::path::MAIN_SEPARATOR != '/' && stripped.contains(std::path::MAIN_SEPARATOR));
    if looks_like_path {
        // 尝试旧版溢出根目录。
        if let Some(found) = try_path(root.join(stripped), &mut tried) {
            return Ok(found);
        }
        // 会话产物根目录直接指向 `<sid>/artifacts/`。
        // 在拼接之前，从记录路径中去除可选的 `artifacts/` 前缀段。
        if let Some(sa_root) = session_artifacts_root.as_ref() {
            let rel = stripped.strip_prefix("artifacts/").unwrap_or(stripped);
            if let Some(found) = try_path(sa_root.join(rel), &mut tried) {
                return Ok(found);
            }
        }
        return Err(not_found(
            reference,
            &tried,
            &root,
            session_artifacts_root.as_deref(),
        ));
    }

    // 形式 1：裸 id → 旧版 `tool_outputs/<id>.txt`。
    if let Some(p) = crate::tools::truncate::spillover_path(stripped)
        && let Some(found) = try_path(p, &mut tried)
    {
        return Ok(found);
    }
    // 形式 2：`art_<id>` → 去除前缀并尝试两种方式：
    //   a) 会话产物目录 `artifacts/art_<id>.txt`
    //   b) 旧版溢出 `<id>.txt`
    if let Some(stripped_art) = stripped.strip_prefix("art_") {
        if let Some(sa_root) = session_artifacts_root.as_ref() {
            let session_file = sa_root.join(format!("art_{stripped_art}.txt"));
            if let Some(found) = try_path(session_file, &mut tried) {
                return Ok(found);
            }
        }
        if let Some(p) = crate::tools::truncate::spillover_path(stripped_art)
            && let Some(found) = try_path(p, &mut tried)
        {
            return Ok(found);
        }
    }
    // 形式 2b：模型可能传了裸 id，但产物实际在会话产物目录下。尝试 `artifacts/art_<id>.txt`。
    if let Some(sa_root) = session_artifacts_root.as_ref() {
        let session_file = sa_root.join(format!("art_{stripped}.txt"));
        if let Some(found) = try_path(session_file, &mut tried) {
            return Ok(found);
        }
    }

    Err(not_found(
        reference,
        &tried,
        &root,
        session_artifacts_root.as_deref(),
    ))
}

/// 格式化"引用未解析"的错误，提供足够详细的信息，
/// 以便调用者在下次尝试时选择有效的引用形式。
fn not_found(
    reference: &str,
    tried: &[PathBuf],
    legacy_root: &std::path::Path,
    session_artifacts_root: Option<&std::path::Path>,
) -> ToolError {
    let tried_list = if tried.is_empty() {
        "(no valid candidates derived from ref)".to_string()
    } else {
        tried
            .iter()
            .map(|p| format!("  - {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let session_hint = session_artifacts_root
        .map(|p| format!("\nsession artifacts root: {}", p.display()))
        .unwrap_or_default();
    ToolError::execution_failed(format!(
        "spilled tool result `{reference}` not found. Tried:\n{tried_list}\n\
         spillover root: {legacy}{session}\n\
         Accepted ref forms: \
         (a) `<tool_call_id>` for legacy spillover, \
         (b) `art_<tool_call_id>` for session artifacts, \
         (c) `sha:<64-hex>` or bare 64-hex from a <TOOL_RESULT_REF> block, \
         (d) `artifacts/art_<id>.txt` or `<id>.txt` relative paths. \
         If the source was a `<TOOL_RESULT_REF sha=\"...\" />` block, copy the \
         sha value and pass it as `ref=sha:<value>`. \
         If the source was an [artifact ...] block, pass the `id:` field \
         (the `art_<id>` form) directly.",
        legacy = legacy_root.display(),
        session = session_hint,
    ))
}

fn build_summary_payload(
    reference: &str,
    path: &std::path::Path,
    content: &str,
    lines: &[&str],
    input: &Value,
    max_bytes: usize,
) -> Value {
    let max_matches = clamp_u64(
        optional_u64(input, "max_matches", DEFAULT_MAX_MATCHES as u64),
        1,
        HARD_MAX_MATCHES,
    );
    let signal_lines = collect_signal_lines(lines, max_matches);
    let head_count = DEFAULT_LINE_COUNT.min(lines.len());
    let tail_count = DEFAULT_LINE_COUNT.min(lines.len());
    let head = render_numbered_lines(
        lines
            .iter()
            .take(head_count)
            .enumerate()
            .map(|(idx, line)| (idx + 1, *line)),
        max_bytes / 2,
    );
    let tail_start = lines.len().saturating_sub(tail_count);
    let tail = render_numbered_lines(
        lines
            .iter()
            .enumerate()
            .skip(tail_start)
            .map(|(idx, line)| (idx + 1, *line)),
        max_bytes / 2,
    );

    json!({
        "ref": reference,
        "path": path.display().to_string(),
        "mode": "summary",
        "total_bytes": content.len(),
        "total_lines": lines.len(),
        "non_empty_lines": lines.iter().filter(|line| !line.trim().is_empty()).count(),
        "signal_lines": signal_lines,
        "head": head,
        "tail": tail,
        "hint": "Use mode=head, tail, lines, or query to retrieve a narrower slice."
    })
}

fn build_head_tail_payload(
    reference: &str,
    path: &std::path::Path,
    mode: &str,
    lines: &[&str],
    input: &Value,
    max_bytes: usize,
) -> Value {
    let count = clamp_u64(
        optional_u64(input, "line_count", DEFAULT_LINE_COUNT as u64),
        1,
        HARD_LINE_COUNT,
    );
    let selected: Vec<(usize, &str)> = if mode == "head" {
        lines
            .iter()
            .take(count)
            .enumerate()
            .map(|(idx, line)| (idx + 1, *line))
            .collect()
    } else {
        let start = lines.len().saturating_sub(count);
        lines
            .iter()
            .enumerate()
            .skip(start)
            .map(|(idx, line)| (idx + 1, *line))
            .collect()
    };
    let excerpt = render_numbered_lines(selected.iter().copied(), max_bytes);

    json!({
        "ref": reference,
        "path": path.display().to_string(),
        "mode": mode,
        "total_lines": lines.len(),
        "line_count": count,
        "excerpt": excerpt,
    })
}

fn build_lines_payload(
    reference: &str,
    path: &std::path::Path,
    lines: &[&str],
    input: &Value,
    max_bytes: usize,
) -> Result<Value, ToolError> {
    let (start, end) = parse_line_selector(input)?;
    let excerpt = if start > lines.len() {
        String::new()
    } else {
        let end = end.min(lines.len());
        render_numbered_lines(
            lines
                .iter()
                .enumerate()
                .skip(start - 1)
                .take(end.saturating_sub(start) + 1)
                .map(|(idx, line)| (idx + 1, *line)),
            max_bytes,
        )
    };

    Ok(json!({
        "ref": reference,
        "path": path.display().to_string(),
        "mode": "lines",
        "total_lines": lines.len(),
        "start_line": start,
        "end_line": end.min(lines.len()),
        "excerpt": excerpt,
    }))
}

fn build_query_payload(
    reference: &str,
    path: &std::path::Path,
    lines: &[&str],
    input: &Value,
    max_bytes: usize,
) -> Result<Value, ToolError> {
    let query = optional_str(input, "query")
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or_else(|| ToolError::invalid_input("query is required when mode=query"))?;
    let query_lower = query.to_lowercase();
    let max_matches = clamp_u64(
        optional_u64(input, "max_matches", DEFAULT_MAX_MATCHES as u64),
        1,
        HARD_MAX_MATCHES,
    );
    let context_lines = clamp_u64(
        optional_u64(input, "context_lines", DEFAULT_CONTEXT_LINES as u64),
        0,
        HARD_CONTEXT_LINES,
    );

    let mut matched_lines = 0usize;
    let mut results = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if !line.to_lowercase().contains(&query_lower) {
            continue;
        }
        matched_lines += 1;
        if results.len() >= max_matches {
            continue;
        }
        let start = idx.saturating_sub(context_lines);
        let end = (idx + context_lines).min(lines.len().saturating_sub(1));
        let excerpt = render_numbered_lines(
            lines
                .iter()
                .enumerate()
                .skip(start)
                .take(end.saturating_sub(start) + 1)
                .map(|(line_idx, text)| (line_idx + 1, *text)),
            max_bytes / max_matches.max(1),
        );
        results.push(json!({
            "line": idx + 1,
            "excerpt": excerpt,
        }));
    }

    Ok(json!({
        "ref": reference,
        "path": path.display().to_string(),
        "mode": "query",
        "query": query,
        "total_lines": lines.len(),
        "matched_lines": matched_lines,
        "matches_returned": results.len(),
        "results": results,
    }))
}

fn parse_line_selector(input: &Value) -> Result<(usize, usize), ToolError> {
    let explicit_start = input.get("start_line").and_then(Value::as_u64);
    let explicit_end = input.get("end_line").and_then(Value::as_u64);
    if explicit_start.is_some() || explicit_end.is_some() {
        let start = explicit_start.ok_or_else(|| {
            ToolError::invalid_input("start_line is required when end_line is supplied")
        })?;
        let end = explicit_end.unwrap_or(start);
        return validate_line_range(start as usize, end as usize);
    }

    let spec = optional_str(input, "lines")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ToolError::invalid_input(
                "mode=lines requires `lines` (for example \"10-40\") or start_line/end_line",
            )
        })?;

    if let Some((start, end)) = spec.split_once('-') {
        let start = parse_positive_line(start.trim(), "lines start")?;
        let end = parse_positive_line(end.trim(), "lines end")?;
        validate_line_range(start, end)
    } else {
        let line = parse_positive_line(spec, "lines")?;
        validate_line_range(line, line)
    }
}

fn validate_line_range(start: usize, end: usize) -> Result<(usize, usize), ToolError> {
    if start == 0 || end == 0 {
        return Err(ToolError::invalid_input("line numbers are 1-based"));
    }
    if end < start {
        return Err(ToolError::invalid_input(
            "end_line must be greater than or equal to start_line",
        ));
    }
    Ok((start, end))
}

fn parse_positive_line(raw: &str, field: &str) -> Result<usize, ToolError> {
    raw.parse::<usize>().map_err(|_| {
        ToolError::invalid_input(format!("{field} must be a positive integer line number"))
    })
}

fn collect_signal_lines(lines: &[&str], max_matches: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if !is_signal_line(line) {
            continue;
        }
        out.push(json!({
            "line": idx + 1,
            "text": truncate_line(line.trim(), 300),
        }));
        if out.len() >= max_matches {
            break;
        }
    }
    out
}

fn is_signal_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    [
        "error",
        "failed",
        "failure",
        "panic",
        "warning",
        "exception",
        "traceback",
        "assertion",
        "exit code",
        "test result",
        "thread '",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn render_numbered_lines<'a>(
    lines: impl IntoIterator<Item = (usize, &'a str)>,
    max_bytes: usize,
) -> String {
    let mut rendered = String::new();
    for (line_no, line) in lines {
        rendered.push_str(&format!("{line_no}: {line}\n"));
        if rendered.len() > max_bytes {
            break;
        }
    }
    truncate_text(&rendered, max_bytes)
}

fn truncate_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.trim_end_matches('\n').to_string();
    }
    let note = "\n[truncated to max_bytes]";
    let budget = max_bytes.saturating_sub(note.len()).max(1);
    let cut = (0..=budget)
        .rev()
        .find(|idx| text.is_char_boundary(*idx))
        .unwrap_or(0);
    format!("{}{}", text[..cut].trim_end_matches('\n'), note)
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let mut out: String = line.chars().take(max_chars.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

fn clamp_u64(value: u64, min: usize, max: usize) -> usize {
    (value as usize).clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;
    use tempfile::tempdir;

    struct SpilloverRootGuard {
        prior: Option<PathBuf>,
    }

    impl Drop for SpilloverRootGuard {
        fn drop(&mut self) {
            crate::tools::truncate::set_test_spillover_root(self.prior.take());
        }
    }

    fn set_spillover_root(path: PathBuf) -> SpilloverRootGuard {
        let prior = crate::tools::truncate::set_test_spillover_root(Some(path));
        SpilloverRootGuard { prior }
    }

    fn context() -> ToolContext {
        let tmp = tempdir().unwrap();
        ToolContext::new(tmp.path())
    }

    fn test_lock() -> MutexGuard<'static, ()> {
        crate::tools::truncate::TEST_SPILLOVER_GUARD
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    fn execute_tool(input: Value) -> Result<ToolResult, ToolError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(RetrieveToolResultTool.execute(input, &context()))
    }

    #[test]
    fn summary_reads_spillover_by_tool_call_id() {
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let _guard = set_spillover_root(tmp.path().join("tool_outputs"));
        crate::tools::truncate::write_spillover(
            "call-abc",
            "checking crate\nerror[E0425]: missing value\nwarning: unused import\nfinished",
        )
        .unwrap();

        let result = execute_tool(json!({"ref": "call-abc"})).unwrap();

        assert!(result.success);
        let body: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(body["mode"], "summary");
        assert!(body["signal_lines"].to_string().contains("error[E0425]"));
        assert!(body["signal_lines"].to_string().contains("warning"));
    }

    #[test]
    fn query_returns_matching_line_with_context() {
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let _guard = set_spillover_root(tmp.path().join("tool_outputs"));
        crate::tools::truncate::write_spillover(
            "call-query",
            "one\ntwo before\nneedle here\nafter\nlast",
        )
        .unwrap();

        let result = execute_tool(json!({
            "ref": "tool_result:call-query",
            "mode": "query",
            "query": "needle",
            "context_lines": 1
        }))
        .unwrap();

        let body: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(body["matched_lines"], 1);
        let rendered = body["results"].to_string();
        assert!(rendered.contains("2: two before"));
        assert!(rendered.contains("3: needle here"));
        assert!(rendered.contains("4: after"));
    }

    #[test]
    fn lines_mode_accepts_filename_inside_spillover_root() {
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("tool_outputs");
        let _guard = set_spillover_root(root.clone());
        crate::tools::truncate::write_spillover("call-lines", "a\nb\nc\nd").unwrap();

        let result = execute_tool(json!({
            "ref": "call-lines.txt",
            "mode": "lines",
            "lines": "2-3"
        }))
        .unwrap();

        let body: Value = serde_json::from_str(&result.content).unwrap();
        let excerpt = body["excerpt"].as_str().unwrap();
        assert!(excerpt.contains("2: b"));
        assert!(excerpt.contains("3: c"));
        assert!(!excerpt.contains("1: a"));
        assert!(!excerpt.contains("4: d"));
    }

    #[test]
    fn rejects_path_outside_spillover_root() {
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("tool_outputs");
        fs::create_dir_all(&root).unwrap();
        let outside = tmp.path().join("outside.txt");
        fs::write(&outside, "secret").unwrap();
        let _guard = set_spillover_root(root);

        let err = execute_tool(json!({"ref": outside.display().to_string()})).unwrap_err();

        // 新解析器将所有不在批准根目录下的路径归类为 "not found"，
        // 这样我们就不会意外泄露外部路径是否存在于磁盘上。
        let msg = err.to_string();
        assert!(
            msg.contains("not found"),
            "expected `not found` diagnostic, got: {msg}"
        );
    }

    #[test]
    fn resolves_sha_reference_from_wire_dedup() {
        // 基于 SHA 的查找 — 模拟模型看到 `<TOOL_RESULT_REF sha="..." />` 块
        // 并将 SHA 传递给 retrieve_tool_result 时的行为。
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let _guard = set_spillover_root(tmp.path().join("tool_outputs"));
        let body = "checking crate ... error[E0425]: cannot find value\n".repeat(80);
        let sha = crate::hashing::sha256_hex(body.as_bytes());
        crate::tools::truncate::write_sha_spillover(&sha, &body).unwrap();

        // 形式：`sha:<hex>`
        let result = execute_tool(json!({"ref": format!("sha:{sha}")})).unwrap();
        assert!(result.success, "sha:<hex> form should resolve");

        // 形式：裸 64-hex
        let result = execute_tool(json!({"ref": &sha})).unwrap();
        assert!(result.success, "bare 64-hex form should resolve");
    }

    #[test]
    fn resolves_art_prefix_to_legacy_spillover_id() {
        // 模型通常在产物引用块中看到 `id: art_call_xyz`。
        // retrieve_tool_result 应去除 `art_` 前缀，并在不存在
        // 相应的会话产物时找到旧版 `<id>.txt` 文件。
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let _guard = set_spillover_root(tmp.path().join("tool_outputs"));
        crate::tools::truncate::write_spillover("call_xyz", "line1\nline2\nline3").unwrap();

        let result = execute_tool(json!({"ref": "art_call_xyz"})).unwrap();
        assert!(result.success, "art_ prefix should resolve to legacy id");
    }

    #[test]
    fn not_found_error_lists_tried_candidates_and_accepted_forms() {
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let _guard = set_spillover_root(tmp.path().join("tool_outputs"));
        fs::create_dir_all(tmp.path().join("tool_outputs")).unwrap();

        let err = execute_tool(json!({"ref": "definitely_missing_id"})).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not found"), "got: {msg}");
        assert!(
            msg.contains("sha:"),
            "diagnostic should mention sha form: {msg}"
        );
        assert!(
            msg.contains("art_<tool_call_id>"),
            "diagnostic should mention art form: {msg}"
        );
        assert!(
            msg.contains("tool_outputs"),
            "tried list should include the legacy spillover candidate: {msg}"
        );
        assert!(
            !msg.contains("(no valid candidates derived from ref)"),
            "tried list should not be empty: {msg}"
        );
    }

    #[test]
    fn resolves_art_prefix_via_session_artifacts() {
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let _spill_guard = set_spillover_root(tmp.path().join("tool_outputs"));
        let _art_guard = {
            let prior = crate::artifacts::set_test_artifact_sessions_root(Some(
                tmp.path().join("sessions"),
            ));
            scopeguard_for_test(prior)
        };
        let session_id = "session-abc";
        let body = "this is the canonical session artifact body, not a legacy file";
        crate::artifacts::write_session_artifact(session_id, "art_call_real", body).unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let workspace_tmp = tempdir().unwrap();
        let ctx = ToolContext::new(workspace_tmp.path()).with_state_namespace(session_id);
        let result = runtime
            .block_on(RetrieveToolResultTool.execute(json!({"ref": "art_call_real"}), &ctx))
            .expect("art_<id> should resolve via session artifacts");
        assert!(result.success);
        let payload: Value = serde_json::from_str(&result.content).unwrap();
        assert!(
            payload
                .to_string()
                .contains("canonical session artifact body"),
            "summary should pull from session artifact, got: {payload}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_inside_session_artifacts() {
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let _spill_guard = set_spillover_root(tmp.path().join("tool_outputs"));
        let _art_guard = {
            let prior = crate::artifacts::set_test_artifact_sessions_root(Some(
                tmp.path().join("sessions"),
            ));
            scopeguard_for_test(prior)
        };
        let session_id = "session-xyz";
        // 在产物目录外放置一个敏感文件。
        let secret = tmp.path().join("secret.txt");
        fs::write(&secret, "do not leak").unwrap();
        // 创建产物目录，然后在其中放置指向该秘密文件的符号链接。
        let art_dir = tmp
            .path()
            .join("sessions")
            .join(session_id)
            .join("artifacts");
        fs::create_dir_all(&art_dir).unwrap();
        std::os::unix::fs::symlink(&secret, art_dir.join("art_evil.txt")).unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let workspace_tmp = tempdir().unwrap();
        let ctx = ToolContext::new(workspace_tmp.path()).with_state_namespace(session_id);
        let result =
            runtime.block_on(RetrieveToolResultTool.execute(json!({"ref": "art_evil"}), &ctx));
        let err = result.expect_err("symlink artifact must not resolve");
        assert!(
            err.to_string().contains("not found"),
            "expected `not found`, got: {err}"
        );
    }

    struct ArtifactRootGuard {
        prior: Option<PathBuf>,
    }
    impl Drop for ArtifactRootGuard {
        fn drop(&mut self) {
            crate::artifacts::set_test_artifact_sessions_root(self.prior.take());
        }
    }
    fn scopeguard_for_test(prior: Option<PathBuf>) -> ArtifactRootGuard {
        ArtifactRootGuard { prior }
    }
}
