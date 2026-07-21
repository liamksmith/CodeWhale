//! 搜索工具：用于代码搜索的 `grep_files`
//!
//! 这些工具在工作区内提供强大的代码搜索能力，
//! 类似于 ripgrep/grep 的功能。

use super::spec::{
    ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, optional_bool, optional_str,
    optional_u64, required_str,
};
use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// 返回结果的最大数量，避免输出过多
const MAX_RESULTS: usize = 100;

/// 可搜索的最大文件大小（跳过大型二进制文件）
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB

/// 单次 grep_files 运行的硬性上限。目录遍历和每个文件的正则匹配
/// 是同步阻塞操作；没有这个限制，在大目录树上可能会运行数分钟。
/// 与 file_search 工具保持一致，使两个阻塞搜索的行为相同。
const GREP_FILES_TIMEOUT: Duration = Duration::from_secs(30);

/// grep 匹配的结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    pub file: String,
    pub line_number: usize,
    pub line: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

/// 使用正则表达式模式搜索文件的工具
pub struct GrepFilesTool;

#[async_trait]
impl ToolSpec for GrepFilesTool {
    fn name(&self) -> &'static str {
        "grep_files"
    }

    fn description(&self) -> &'static str {
        "Search for a regex pattern in workspace files. Use this instead of `grep -r`, `rg`, or `find ... -exec grep` in `exec_shell` — pure-Rust, faster, and respects `.gitignore`. Returns matching lines with context (default: 2 lines before/after each match)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search (relative to workspace, default: .)"
                },
                "include": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Glob patterns for files to include (e.g., ['*.rs', '*.ts'])"
                },
                "exclude": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Glob patterns for files to exclude (e.g., ['*.min.js', 'node_modules/*'])"
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Number of context lines before and after each match (default: 2)"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Whether to perform case-insensitive matching (default: false)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 100)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let pattern_str = required_str(&input, "pattern")?;
        let path_str = optional_str(&input, "path").unwrap_or(".");
        let context_lines = usize::try_from(optional_u64(&input, "context_lines", 2))
            .unwrap_or(usize::MAX)
            .min(1000);
        let case_insensitive = optional_bool(&input, "case_insensitive", false);
        let max_results = usize::try_from(optional_u64(&input, "max_results", MAX_RESULTS as u64))
            .unwrap_or(MAX_RESULTS);

        // 解析包含模式
        let include_patterns: Vec<String> = input
            .get("include")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // 解析排除模式
        let exclude_patterns: Vec<String> =
            input.get("exclude").and_then(|v| v.as_array()).map_or_else(
                || {
                    // 常用非代码目录的默认排除项。
                    // 裸目录名完全跳过该目录的遍历；
                    // 如果目录已在遍历中，`dir/*` 则过滤内部文件（双重保障——参见 #2200）。
                    vec![
                        "node_modules".to_string(),
                        "node_modules/*".to_string(),
                        ".git".to_string(),
                        ".git/*".to_string(),
                        "target".to_string(),
                        "target/*".to_string(),
                        "*.min.js".to_string(),
                        "*.min.css".to_string(),
                        "dist".to_string(),
                        "dist/*".to_string(),
                        "build".to_string(),
                        "build/*".to_string(),
                        "__pycache__".to_string(),
                        "__pycache__/*".to_string(),
                        ".venv".to_string(),
                        ".venv/*".to_string(),
                        "venv".to_string(),
                        "venv/*".to_string(),
                    ]
                },
                |arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                },
            );

        // 构建正则表达式
        let regex_pattern = if case_insensitive {
            format!("(?i){pattern_str}")
        } else {
            pattern_str.to_string()
        };

        let regex = Regex::new(&regex_pattern)
            .map_err(|e| ToolError::invalid_input(format!("Invalid regex pattern: {e}")))?;

        // 解析搜索路径
        let search_path = context.resolve_path(path_str)?;

        let workspace = context.workspace.clone();
        let cancel_token = context.cancel_token.clone();
        let follow_symlinks = context.follow_symlinks;

        // 目录遍历和逐文件正则匹配是同步阻塞操作。
        // 在受硬性超时限制的阻塞工作线程上运行它们，这样大目录树
        // 就不会卡住异步运行时导致停止按钮无响应。
        let result = run_blocking_grep(GREP_FILES_TIMEOUT, cancel_token.clone(), move || {
            let cancel_token = cancel_token.as_ref();

            // 流式遍历：每个文件在被发现时立即搜索，
            // 一旦匹配预算耗尽就停止遍历。
            // 文件从不整体加载到大 Vec 中，文件内容逐行读取，
            // 因此内存占用受结果集大小限制。
            let mut results: Vec<GrepMatch> = Vec::new();
            let mut files_searched = 0;
            let mut total_matches = 0;

            visit_files(
                &search_path,
                &include_patterns,
                &exclude_patterns,
                cancel_token,
                follow_symlinks,
                &mut |file_path| {
                    if results.len() >= max_results {
                        return Ok(WalkControl::Stop);
                    }
                    check_cancelled(cancel_token)?;

                    // 跳过过大的文件
                    if let Ok(metadata) = fs::metadata(file_path)
                        && metadata.len() > MAX_FILE_SIZE
                    {
                        return Ok(WalkControl::Continue);
                    }

                    // 获取相对于工作区的路径
                    let relative_path = file_path
                        .strip_prefix(&workspace)
                        .unwrap_or(file_path)
                        .to_string_lossy()
                        .to_string();

                    let budget = max_results - results.len();
                    let Some(file_matches) = search_file_streaming(
                        file_path,
                        &relative_path,
                        &regex,
                        context_lines,
                        budget,
                        cancel_token,
                    )?
                    else {
                        return Ok(WalkControl::Continue); // 跳过二进制或不可读文件
                    };

                    files_searched += 1;
                    total_matches += file_matches.len();
                    results.extend(file_matches);
                    Ok(WalkControl::Continue)
                },
            )?;

            let matches_json: Vec<Value> = results
                .iter()
                .map(|item| grep_match_to_json(item, context_lines))
                .collect();

            // 构建结果。当 context_lines == 1 时，将单个上下文行
            // 作为字符串返回，而不是单元素数组。这使常见的
            // "仅显示相邻行"情况对模型调用者更易读取。
            Ok(json!({
                "matches": matches_json,
                "total_matches": total_matches,
                "files_searched": files_searched,
                "truncated": total_matches > max_results,
            }))
        })
        .await?;

        ToolResult::json(&result).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

/// 在阻塞工作线程上运行同步 grep 遍历，可通过 token 取消
/// 并由 `timeout` 限制。镜像 `run_blocking_file_search`。
async fn run_blocking_grep<F>(
    timeout: Duration,
    cancel_token: Option<CancellationToken>,
    search: F,
) -> Result<Value, ToolError>
where
    F: FnOnce() -> Result<Value, ToolError> + Send + 'static,
{
    if cancel_token
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Err(grep_cancelled());
    }

    let task = tokio::task::spawn_blocking(search);
    let result = match cancel_token {
        Some(token) => {
            tokio::select! {
                biased;
                () = token.cancelled() => return Err(grep_cancelled()),
                result = tokio::time::timeout(timeout, task) => result,
            }
        }
        None => tokio::time::timeout(timeout, task).await,
    };

    let joined = result.map_err(|_| grep_timeout(timeout))?;
    joined.map_err(|err| {
        ToolError::execution_failed(format!("grep_files worker failed before completion: {err}"))
    })?
}

fn grep_cancelled() -> ToolError {
    ToolError::execution_failed("grep_files cancelled before completion")
}

fn grep_timeout(timeout: Duration) -> ToolError {
    ToolError::Timeout {
        seconds: timeout.as_secs().max(1),
    }
}

fn grep_match_to_json(item: &GrepMatch, context_lines: usize) -> Value {
    if context_lines == 1 {
        json!({
            "file": item.file,
            "line_number": item.line_number,
            "line": item.line,
            "context_before": item.context_before.first().cloned().unwrap_or_default(),
            "context_after": item.context_after.first().cloned().unwrap_or_default(),
        })
    } else {
        json!(item)
    }
}

/// 使用小型环形缓冲区逐行搜索单个文件以保存前向上下文，
/// 因此文件内容永远不会整体加载到内存中。
///
/// 当文件不可读或包含无效 UTF-8 时返回 `Ok(None)`
/// —— 与之前的 `read_to_string` 实现具有相同的"跳过二进制或不可读文件"语义，
/// 该实现在贡献任何匹配之前要求整个文件有效。
/// 最多记录 `budget` 个匹配；扫描仍然运行到文件末尾，
/// 以便后续无效字节使文件失效并完成待处理的后向上下文。
fn search_file_streaming(
    path: &Path,
    relative_path: &str,
    regex: &Regex,
    context_lines: usize,
    budget: usize,
    cancel_token: Option<&CancellationToken>,
) -> Result<Option<Vec<GrepMatch>>, ToolError> {
    let Ok(file) = fs::File::open(path) else {
        return Ok(None);
    };
    let mut reader = std::io::BufReader::new(file);
    let mut raw: Vec<u8> = Vec::new();
    let mut before: VecDeque<String> = VecDeque::new();
    let mut matches: Vec<GrepMatch> = Vec::new();
    // 仍在等待后向上下文行的匹配：(`matches` 中的索引,
    // 仍需的行数)。条目按 FIFO 顺序完成。
    let mut pending: VecDeque<(usize, usize)> = VecDeque::new();
    let mut line_idx = 0usize;

    loop {
        raw.clear();
        let n = match reader.read_until(b'\n', &mut raw) {
            Ok(n) => n,
            Err(_) => return Ok(None),
        };
        if n == 0 {
            break;
        }
        check_cancelled(cancel_token)?;

        // 镜像 `str::lines`：去掉末尾的 '\n'，以及仅当 '\r' 直接
        // 位于 '\n' 之前时也去掉 '\r'。
        let mut end = raw.len();
        if raw[..end].ends_with(b"\n") {
            end -= 1;
            if raw[..end].ends_with(b"\r") {
                end -= 1;
            }
        }
        let Ok(line) = std::str::from_utf8(&raw[..end]) else {
            return Ok(None);
        };

        for (idx, remaining) in &mut pending {
            matches[*idx].context_after.push(line.to_string());
            *remaining -= 1;
        }
        while pending
            .front()
            .is_some_and(|(_, remaining)| *remaining == 0)
        {
            pending.pop_front();
        }

        if matches.len() < budget && regex.is_match(line) {
            matches.push(GrepMatch {
                file: relative_path.to_string(),
                line_number: line_idx + 1,
                line: line.to_string(),
                context_before: before.iter().cloned().collect(),
                context_after: Vec::new(),
            });
            if context_lines > 0 {
                pending.push_back((matches.len() - 1, context_lines));
            }
        }

        if context_lines > 0 {
            if before.len() == context_lines {
                before.pop_front();
            }
            before.push_back(line.to_string());
        }
        line_idx += 1;
    }

    Ok(Some(matches))
}

/// 流式文件遍历的流程控制。
enum WalkControl {
    Continue,
    Stop,
}

/// 遍历匹配包含/排除模式的文件，按遍历顺序对每个文件调用 `visit`。
/// 当 `visit` 返回 [`WalkControl::Stop`] 时提前停止遍历。
fn visit_files(
    root: &Path,
    include_patterns: &[String],
    exclude_patterns: &[String],
    cancel_token: Option<&CancellationToken>,
    follow_symlinks: bool,
    visit: &mut dyn FnMut(&Path) -> Result<WalkControl, ToolError>,
) -> Result<(), ToolError> {
    let mut visited_dirs: HashSet<PathBuf> = HashSet::new();
    check_cancelled(cancel_token)?;

    if root.is_file() {
        visit(root)?;
        return Ok(());
    }

    if follow_symlinks && let Ok(canonical_root) = root.canonicalize() {
        visited_dirs.insert(canonical_root);
    }

    visit_files_recursive(
        root,
        root,
        include_patterns,
        exclude_patterns,
        cancel_token,
        &mut visited_dirs,
        follow_symlinks,
        visit,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn visit_files_recursive(
    root: &Path,
    current: &Path,
    include_patterns: &[String],
    exclude_patterns: &[String],
    cancel_token: Option<&CancellationToken>,
    visited_dirs: &mut HashSet<PathBuf>,
    follow_symlinks: bool,
    visit: &mut dyn FnMut(&Path) -> Result<WalkControl, ToolError>,
) -> Result<WalkControl, ToolError> {
    check_cancelled(cancel_token)?;

    let entries = fs::read_dir(current).map_err(|e| {
        ToolError::execution_failed(format!(
            "Failed to read directory {}: {}",
            current.display(),
            e
        ))
    })?;

    for entry in entries {
        check_cancelled(cancel_token)?;

        let entry = entry.map_err(|e| ToolError::execution_failed(e.to_string()))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| {
            ToolError::execution_failed(format!(
                "Failed to inspect file type for {}: {}",
                path.display(),
                e
            ))
        })?;
        if file_type.is_symlink() && !follow_symlinks {
            continue;
        }

        // 获取相对路径用于模式匹配
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let relative_str = relative.to_string_lossy();

        // 检查排除规则
        if should_exclude(&relative_str, exclude_patterns) {
            continue;
        }

        // 当追踪符号链接时，解析目录和文件的目标类型，
        // 以便遍历符号链接的目录并包含符号链接的文件。
        let effective_type = if file_type.is_symlink() && follow_symlinks {
            match fs::metadata(&path) {
                Ok(meta) => meta.file_type(),
                Err(_) => continue,
            }
        } else {
            file_type
        };

        if effective_type.is_dir() {
            if follow_symlinks {
                let canonical_dir = match path.canonicalize() {
                    Ok(canonical) => canonical,
                    Err(_) => continue,
                };
                if !visited_dirs.insert(canonical_dir) {
                    continue;
                }
            }
            if let WalkControl::Stop = visit_files_recursive(
                root,
                &path,
                include_patterns,
                exclude_patterns,
                cancel_token,
                visited_dirs,
                follow_symlinks,
                visit,
            )? {
                return Ok(WalkControl::Stop);
            }
        } else if effective_type.is_file() {
            // 检查包含规则（如果有指定）
            if (include_patterns.is_empty() || should_include(&relative_str, include_patterns))
                && let WalkControl::Stop = visit(&path)?
            {
                return Ok(WalkControl::Stop);
            }
        }
    }

    Ok(WalkControl::Continue)
}

fn check_cancelled(cancel_token: Option<&CancellationToken>) -> Result<(), ToolError> {
    if cancel_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(ToolError::execution_failed(
            "search cancelled before completion",
        ));
    }
    Ok(())
}

/// 检查路径是否匹配任何排除模式
fn should_exclude(path: &str, patterns: &[String]) -> bool {
    for pattern in patterns {
        if matches_glob(path, pattern) {
            return true;
        }
    }
    false
}

/// 检查路径是否匹配任何包含模式
fn should_include(path: &str, patterns: &[String]) -> bool {
    for pattern in patterns {
        if matches_glob(path, pattern) {
            return true;
        }
    }
    false
}

/// 简单的 glob 模式匹配
/// 支持：*（任意字符）、**（任意路径）、?（单个字符）
pub(crate) fn matches_glob(path: &str, pattern: &str) -> bool {
    // 处理 ** 表示任意路径
    if pattern.contains("**") {
        let parts: Vec<&str> = pattern.split("**").collect();
        if parts.len() == 2 {
            let prefix = parts[0].trim_end_matches('/');
            let suffix = parts[1].trim_start_matches('/');

            if !prefix.is_empty() && !path.starts_with(prefix) {
                return false;
            }
            if !suffix.is_empty() {
                return path.ends_with(suffix)
                    || path
                        .split('/')
                        .any(|part| matches_simple_glob(part, suffix));
            }
            return path.starts_with(prefix) || prefix.is_empty();
        }
    }

    // 处理类似 "*.rs" 的模式——仅匹配文件名
    if pattern.starts_with('*') && !pattern.contains('/') {
        let filename = path.rsplit('/').next().unwrap_or(path);
        return matches_simple_glob(filename, pattern);
    }

    // 处理包含路径组件的模式
    if pattern.contains('/') {
        return matches_simple_glob(path, pattern);
    }

    // 匹配文件名
    let filename = path.rsplit('/').next().unwrap_or(path);
    matches_simple_glob(filename, pattern)
}

/// 单个路径组件的简单 glob 匹配
fn matches_simple_glob(text: &str, pattern: &str) -> bool {
    let mut text_chars = text.chars().peekable();
    let mut pattern_chars = pattern.chars().peekable();

    while let Some(p) = pattern_chars.next() {
        match p {
            '*' => {
                // 匹配零个或多个字符
                let next_pattern: String = pattern_chars.collect();
                if next_pattern.is_empty() {
                    return true;
                }

                // 在每个位置尝试匹配（使用 char-indices 保持在
                // UTF-8 边界上——字节索引切片会在像 冰糖 这样的多字节
                // 字符上 panic，参见 #249）。
                let remaining: String = text_chars.collect();
                for (i, _) in remaining.char_indices() {
                    if matches_simple_glob(&remaining[i..], &next_pattern) {
                        return true;
                    }
                }
                // 也尝试在字符串末尾匹配空后缀
                if matches_simple_glob("", &next_pattern) {
                    return true;
                }
                return false;
            }
            '?' => {
                // 精确匹配一个字符
                if text_chars.next().is_none() {
                    return false;
                }
            }
            c => {
                // 匹配字面字符
                if text_chars.next() != Some(c) {
                    return false;
                }
            }
        }
    }

    text_chars.next().is_none()
}

// === 单元测试 ===

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{Value, json};
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use crate::tools::spec::{ApprovalRequirement, ToolContext, ToolSpec};

    use super::{GrepFilesTool, matches_glob};

    #[test]
    fn test_matches_glob_star() {
        assert!(matches_glob("test.rs", "*.rs"));
        assert!(matches_glob("foo.rs", "*.rs"));
        assert!(!matches_glob("test.ts", "*.rs"));
        assert!(!matches_glob("test.rs.bak", "*.rs"));
    }

    #[test]
    fn test_matches_glob_question() {
        assert!(matches_glob("test.rs", "test.??"));
        assert!(!matches_glob("test.rs", "test.?"));
    }

    #[test]
    fn test_matches_glob_double_star() {
        assert!(matches_glob("src/main.rs", "src/**"));
        assert!(matches_glob("src/lib/mod.rs", "src/**"));
        assert!(matches_glob("node_modules/pkg/index.js", "node_modules/*"));
    }

    #[test]
    fn test_matches_glob_path() {
        assert!(matches_glob("src/main.rs", "src/*.rs"));
        assert!(!matches_glob("lib/main.rs", "src/*.rs"));
    }

    /// #249 的回归测试：字节索引切片在文件名中的多字节字符
    ///（如 `dialogue_line__冰糖.mp3`）上 panic。
    #[test]
    fn test_matches_glob_unicode_filename() {
        let filename = "dialogue_line__冰糖.mp3";
        // 文件名应匹配 *.mp3 且不 panic。
        assert!(matches_glob(filename, "*.mp3"));
        // 星号匹配多字节字符必须成功。
        assert!(matches_glob(filename, "dialogue_line__*"));
        // 模式中的字面多字节字符必须匹配。
        assert!(matches_glob(filename, "*冰糖*"));
        // 不匹配的模式也不能 panic。
        assert!(!matches_glob(filename, "nonexistent*"));
    }

    #[tokio::test]
    async fn test_grep_files_basic() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // 创建测试文件
        fs::write(
            tmp.path().join("test.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .expect("write");
        fs::write(
            tmp.path().join("lib.rs"),
            "pub fn hello() {}\npub fn world() {}\n",
        )
        .expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "fn"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("main"));
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn test_grep_files_with_context() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(
            tmp.path().join("test.txt"),
            "line1\nline2\nMATCH\nline4\nline5\n",
        )
        .expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "MATCH", "context_lines": 1}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("line2")); // 前向上下文
        assert!(result.content.contains("line4")); // 后向上下文

        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["context_before"], "line2");
        assert_eq!(matches[0]["context_after"], "line4");
        assert!(matches[0]["context_before"].is_string());
        assert!(matches[0]["context_after"].is_string());
    }

    #[tokio::test]
    async fn test_grep_files_multi_line_context_remains_arrays() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(tmp.path().join("test.txt"), "a\nb\nMATCH\nd\ne\n").expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "MATCH", "context_lines": 2}), &ctx)
            .await
            .expect("execute");

        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["context_before"], json!(["a", "b"]));
        assert_eq!(matches[0]["context_after"], json!(["d", "e"]));
    }

    #[tokio::test]
    async fn test_grep_files_case_insensitive() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(
            tmp.path().join("test.txt"),
            "Hello World\nHELLO WORLD\nhello world\n",
        )
        .expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "hello", "case_insensitive": true}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        // 应找到全部 3 行
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 3);
    }

    #[tokio::test]
    async fn test_grep_files_include_filter() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(tmp.path().join("test.rs"), "fn test() {}\n").expect("write");
        fs::write(tmp.path().join("test.js"), "function test() {}\n").expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "test", "include": ["*.rs"]}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        // 应只匹配 .rs 文件
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        let file = matches[0]["file"].as_str().unwrap();
        assert!(
            file.rsplit('.')
                .next()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_grep_files_does_not_follow_symlinked_files() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).expect("mkdir workspace");
        std::fs::create_dir_all(&outside).expect("mkdir outside");
        let outside_file = outside.join("secret.txt");
        fs::write(&outside_file, "NEEDLE\n").expect("write outside");
        std::os::unix::fs::symlink(&outside_file, root.join("secret.txt")).expect("symlink");

        let ctx = ToolContext::new(root);
        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "NEEDLE"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 0);
        assert_eq!(parsed["files_searched"].as_u64().unwrap(), 0);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_grep_files_default_mode_skips_symlinked_directories_but_keeps_real_files() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let real_dir = workspace.join("real");
        std::fs::create_dir_all(&real_dir).expect("mkdir workspace");
        fs::write(real_dir.join("needle.txt"), "NEEDLE\n").expect("write real file");
        std::os::unix::fs::symlink(&workspace, real_dir.join("loop")).expect("symlink loop");

        let ctx = ToolContext::new(workspace);
        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "NEEDLE"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 1);
        assert_eq!(parsed["files_searched"].as_u64().unwrap(), 1);
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert!(
            matches[0]["file"]
                .as_str()
                .unwrap()
                .ends_with("real/needle.txt")
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_grep_files_follow_symlinks_avoids_directory_cycles() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let real_dir = workspace.join("real");
        fs::create_dir_all(&real_dir).expect("mkdir");
        fs::write(real_dir.join("needle.txt"), "NEEDLE\n").expect("write");
        std::os::unix::fs::symlink(&workspace, real_dir.join("loop")).expect("symlink loop");

        let ctx = ToolContext::new(workspace).with_follow_symlinks(true);
        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "NEEDLE"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 1);
        assert_eq!(parsed["files_searched"].as_u64().unwrap(), 1);
        let matches = parsed["matches"].as_array().unwrap();
        assert!(matches[0]["file"].as_str().unwrap().ends_with("needle.txt"));
    }

    #[tokio::test]
    async fn test_grep_files_invalid_regex() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = GrepFilesTool;
        let result = tool.execute(json!({"pattern": "[invalid"}), &ctx).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_grep_files_respects_cancel_token() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("test.txt"), "needle\n").expect("write");
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        let ctx = ToolContext::new(tmp.path().to_path_buf()).with_cancel_token(cancel_token);

        let tool = GrepFilesTool;
        let err = tool
            .execute(json!({"pattern": "needle"}), &ctx)
            .await
            .expect_err("cancelled grep should return an error");

        assert!(
            format!("{err:?}").contains("cancelled"),
            "unexpected error: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_grep_files_streaming_stops_at_max_results() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // 两个文件各有许多匹配；一旦预算耗尽，遍历必须停止，
        // 且不丢失最后一个匹配的上下文。
        for name in ["a.txt", "b.txt"] {
            let body: String = (1..=20).map(|n| format!("needle {n}\n")).collect();
            fs::write(tmp.path().join(name), body).expect("write");
        }

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "needle", "max_results": 5}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 5);
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 5);
        // 所有五个匹配必须来自第一个被遍历的文件，按文件顺序
        //（流式遍历保持遍历顺序）。
        let first_file = matches[0]["file"].as_str().unwrap().to_string();
        for m in matches {
            assert_eq!(m["file"].as_str().unwrap(), first_file);
        }
        // 预算内的最后一个匹配仍然获得完整的后向上下文，
        // 即使匹配预算在它上面已耗尽。
        assert_eq!(
            matches[4]["context_after"],
            json!(["needle 6", "needle 7"]),
            "last match must keep after-context lines"
        );
    }

    #[tokio::test]
    async fn test_grep_files_ring_buffer_context_matches_full_read() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // 文件开头、中间和结尾的匹配分别测试了
        // 部分前向上下文（环形缓冲区未满）和
        // 截断后向上下文（文件结束）的路径。
        fs::write(
            tmp.path().join("ctx.txt"),
            "MATCH first\nb1\nb2\nb3\nMATCH mid\na1\na2\na3\nMATCH last\n",
        )
        .expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "MATCH", "context_lines": 2}), &ctx)
            .await
            .expect("execute");

        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0]["context_before"], json!([]));
        assert_eq!(matches[0]["context_after"], json!(["b1", "b2"]));
        assert_eq!(matches[1]["context_before"], json!(["b2", "b3"]));
        assert_eq!(matches[1]["context_after"], json!(["a1", "a2"]));
        assert_eq!(matches[2]["context_before"], json!(["a2", "a3"]));
        assert_eq!(matches[2]["context_after"], json!([]));
        assert_eq!(matches[2]["line_number"].as_u64().unwrap(), 9);
    }

    #[tokio::test]
    async fn test_grep_files_streaming_skips_invalid_utf8_files() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // 匹配行之后的无效 UTF-8：整个文件必须被跳过，
        // 与历史的 read_to_string 行为一致。
        fs::write(
            tmp.path().join("binary.txt"),
            [b"needle\n".as_slice(), &[0xFF, 0xFE, 0x00]].concat(),
        )
        .expect("write");
        fs::write(tmp.path().join("clean.txt"), "needle\n").expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "needle"}), &ctx)
            .await
            .expect("execute");

        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 1);
        assert_eq!(parsed["files_searched"].as_u64().unwrap(), 1);
        let matches = parsed["matches"].as_array().unwrap();
        assert!(matches[0]["file"].as_str().unwrap().ends_with("clean.txt"));
    }

    #[test]
    fn test_grep_files_tool_properties() {
        let tool = GrepFilesTool;
        assert_eq!(tool.name(), "grep_files");
        assert!(tool.is_read_only());
        assert!(tool.is_sandboxable());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
    }

    #[test]
    fn test_parallel_support_flags() {
        let tool = GrepFilesTool;
        assert!(tool.supports_parallel());
    }
}
