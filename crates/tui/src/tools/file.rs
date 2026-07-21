//! 文件系统工具：`read_file`、`write_file`、`edit_file`、`list_dir`
//!
//! 这些工具在工作区内提供安全的文件系统操作，
//! 并通过路径验证防止逃逸工作区边界。

use super::diff_format::make_unified_diff;
use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    lsp_diagnostics_for_paths, optional_bool, optional_str, required_str,
};
use async_trait::async_trait;
use serde_json::{Value, json};
#[cfg(feature = "pdf")]
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// === ReadFileTool ===

/// 用于读取工作区内 UTF-8 文件的工具。
pub struct ReadFileTool;

#[async_trait]
impl ToolSpec for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read a UTF-8 file from the workspace. Use this instead of `cat`, `head`, `tail`, or `sed -n '..p'` in `exec_shell` — it's faster, sandbox-aware, and skips the approval prompt. Plain text is returned as-is and records the file snapshot required before `edit_file` will make a narrow in-place edit. PDFs are auto-extracted via the bundled pure-Rust extractor (no Poppler install required). Image screenshots are OCR-extracted when local OCR is available. Cannot read other non-PDF binaries.\n\nFor large files, use `start_line` and `max_lines` to read in chunks. By default, returns at most 200 lines (~16KB). If `truncated=\"true\"` in the response, use `next_start_line` to continue reading. For PDFs, use `pages` instead — `start_line`/`max_lines` only apply to text files."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file (relative to workspace or absolute)"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Starting line (1-based, default 1)"
                },
                "max_lines": {
                    "type": "integer",
                    "description": "Maximum lines to return (default 200, max 500)"
                },
                "pages": {
                    "type": "string",
                    "description": "PDF only: page range to extract, e.g. \"1-5\" or \"10\". Ignored for non-PDF files."
                }
            },
            "required": ["path"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = required_str(&input, "path")?;
        let file_path = context.resolve_path(path_str)?;
        let pages = optional_str(&input, "pages");

        if is_pdf(&file_path)? {
            return read_pdf(&file_path, pages);
        }
        if is_image_for_ocr(&file_path) {
            return read_image_via_ocr(&file_path, path_str);
        }

        // 在参数解析前打开文件，这样当文件缺失时，
        // 无论其他参数如何，都能保持历史上
        // "Failed to read …" 的错误格式。
        let file = fs::File::open(&file_path).map_err(|e| {
            ToolError::execution_failed(format!("Failed to read {}: {}", file_path.display(), e))
        })?;
        let file_bytes = file.metadata().map(|meta| meta.len()).unwrap_or(u64::MAX);

        let explicit_range = input
            .get("start_line")
            .or_else(|| input.get("max_lines"))
            .is_some();

        // 小文件快速路径。仅在调用者未传递显式范围时适用——
        // 否则，在小文件上显式指定 `start_line = 5`
        // 会静默忽略请求。
        if !explicit_range && file_bytes <= SMALL_FILE_BYTES as u64 {
            drop(file);
            let contents = fs::read_to_string(&file_path).map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to read {}: {}",
                    file_path.display(),
                    e
                ))
            })?;
            context.note_file_read(&file_path);

            let total_lines = contents.lines().count();
            if total_lines <= SMALL_FILE_LINES {
                return Ok(ToolResult::success(contents));
            }

            // 字节数小但行数过多：
            // 直接从内存内容渲染默认窗口。
            let window: Vec<String> = contents
                .lines()
                .take(DEFAULT_READ_LINES)
                .map(str::to_string)
                .collect();
            return Ok(render_line_window(
                path_str,
                &window,
                total_lines,
                1,
                DEFAULT_READ_LINES,
            ));
        }

        let start_line = match input.get("start_line").and_then(Value::as_u64) {
            Some(0) => {
                return Err(ToolError::invalid_input(
                    "start_line must be 1-based and greater than 0".to_string(),
                ));
            }
            Some(v) => usize::try_from(v).map_err(|_| {
                ToolError::invalid_input(
                    "start_line exceeds platform addressable range".to_string(),
                )
            })?,
            None => 1,
        };

        let max_lines = match input.get("max_lines").and_then(Value::as_u64) {
            Some(0) => {
                return Err(ToolError::invalid_input(
                    "max_lines must be greater than 0".to_string(),
                ));
            }
            Some(v) => {
                let converted = usize::try_from(v).map_err(|_| {
                    ToolError::invalid_input(
                        "max_lines exceeds platform addressable range".to_string(),
                    )
                })?;
                std::cmp::min(converted, HARD_MAX_READ_LINES)
            }
            None => DEFAULT_READ_LINES,
        };

        // 针对范围/大文件的有界读取：通过 BufReader 跳过和取行，
        // 而不是将整个文件实例化。流仍然运行到 EOF，
        // 因此总行数和全文件 UTF-8 验证
        // 与历史上的 read_to_string 行为一致。
        let (window, total_lines) =
            read_window_streaming(file, start_line, max_lines).map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to read {}: {}",
                    file_path.display(),
                    e
                ))
            })?;
        context.note_file_read(&file_path);

        // `start_line > total_lines` 不是错误——
        // 它让模型可以翻页到末尾之后而不报错。
        // 返回一个空内容标记，以便后续读取可以停止。
        if start_line > total_lines {
            let output = format!(
                "<file path=\"{path_str}\" total_lines=\"{total_lines}\" shown_lines=\"none\" truncated=\"false\">\n\
                 \n\
                 [NO CONTENT] start_line {start_line} is beyond total_lines {total_lines}.\n\
                 </file>"
            );
            return Ok(ToolResult::success(output));
        }

        Ok(render_line_window(
            path_str,
            &window,
            total_lines,
            start_line,
            max_lines,
        ))
    }
}

// 针对大文件的有界输出。小文件快速路径保持历史上
// "返回未修改内容"的行为，这样现有流程
// （小配置文件、单个源文件等）不会突然开始
// 看到包裹后的输出。一旦文件变大或调用者请求
// 显式范围，我们就切换到一个带编号、行标记的
// 窗口，并附上继续提示，这样模型可以在不每轮
// 重新加载整个文件的情况下翻页。
// 来自 PR #1451 by @Oliver-ZPLiu，关闭 #1450 的一部分。
const DEFAULT_READ_LINES: usize = 200;
const HARD_MAX_READ_LINES: usize = 500;
const MAX_VISIBLE_BYTES: usize = 16 * 1024;
const SMALL_FILE_LINES: usize = 200;
const SMALL_FILE_BYTES: usize = 16 * 1024;

/// 从 `file` 中流式读取行窗口：跳过 `start_line - 1` 行，
/// 收集最多 `max_lines` 行，然后继续计数（并验证 UTF-8）直到 EOF。
/// 返回收集到的窗口及总行数。
/// 只有窗口数据会保留在内存中。
fn read_window_streaming(
    file: fs::File,
    start_line: usize,
    max_lines: usize,
) -> std::io::Result<(Vec<String>, usize)> {
    use std::io::BufRead;

    let mut reader = std::io::BufReader::new(file);
    let mut raw: Vec<u8> = Vec::new();
    let mut window: Vec<String> = Vec::new();
    let mut total_lines = 0usize;
    let start_idx = start_line - 1;

    loop {
        raw.clear();
        let n = reader.read_until(b'\n', &mut raw)?;
        if n == 0 {
            break;
        }
        // 镜像 `str::lines`：去掉末尾的 '\n'，
        // 并且仅当 '\r' 紧随 '\n' 之前时也去掉。
        let mut end = raw.len();
        if raw[..end].ends_with(b"\n") {
            end -= 1;
            if raw[..end].ends_with(b"\r") {
                end -= 1;
            }
        }
        // 验证每一行，这样文件中任何位置的无效 UTF-8
        // 都会像之前全文件 read_to_string 那样失败。
        let line = std::str::from_utf8(&raw[..end]).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            )
        })?;
        if total_lines >= start_idx && window.len() < max_lines {
            window.push(line.to_string());
        }
        total_lines += 1;
    }

    Ok((window, total_lines))
}

/// 将收集到的行窗口渲染成用于范围/大文件读取的
/// `<file …>` 包装器。`window` 必须包含
/// `start_line..start_line + max_lines` 范围内的行（已钳制到 EOF）。
fn render_line_window(
    path_str: &str,
    window: &[String],
    total_lines: usize,
    start_line: usize,
    max_lines: usize,
) -> ToolResult {
    let zero_based_start = start_line - 1;
    let zero_based_end = std::cmp::min(zero_based_start + max_lines, total_lines);
    let shown_first = start_line;
    let shown_last = zero_based_end; // 1-based inclusive line number of the last shown line

    let mut numbered = String::new();
    for (offset, line) in window.iter().enumerate() {
        let line_no = start_line + offset;
        numbered.push_str(&format!("{line_no:>6}│ {line}\n"));
    }

    // 对渲染范围进行 UTF-8 安全的字节截断。
    let truncated_by_bytes = numbered.len() > MAX_VISIBLE_BYTES;
    let shown_content = if truncated_by_bytes {
        let mut end = MAX_VISIBLE_BYTES;
        while end > 0 && !numbered.is_char_boundary(end) {
            end -= 1;
        }
        &numbered[..end]
    } else {
        &numbered
    };

    let truncated_by_lines = zero_based_end < total_lines;
    let truncated = truncated_by_lines || truncated_by_bytes;
    let next_start = zero_based_end + 1;

    let mut attrs = format!(
        "path=\"{path_str}\" total_lines=\"{total_lines}\" shown_lines=\"{shown_first}-{shown_last}\" truncated=\"{truncated}\""
    );
    if truncated_by_lines {
        attrs.push_str(&format!(" next_start_line=\"{next_start}\""));
    }

    let mut output = format!("<file {attrs}>\n{shown_content}");
    if truncated_by_lines {
        output.push_str(&format!(
            "\n[TRUNCATED] Showing lines {shown_first}-{shown_last} of {total_lines}. To continue, call read_file with path=\"{path_str}\" start_line={next_start} max_lines={max_lines}\n"
        ));
    }
    if truncated_by_bytes {
        output.push_str(
            "\n[TRUNCATED] The selected range exceeded 16KB. Continue with a smaller max_lines value.\n",
        );
    }
    output.push_str("</file>");

    ToolResult::success(output)
}

fn read_image_via_ocr(path: &Path, requested_path: &str) -> Result<ToolResult, ToolError> {
    let text = crate::tools::image_ocr::ocr_image_path(path)?;
    Ok(ToolResult::success(format!(
        "<image_ocr path=\"{requested_path}\">\n{text}\n</image_ocr>"
    )))
}

/// 通过扩展名或嗅探 `%PDF-` 魔数来检测 PDF。
/// 没有扩展名的文件在头部匹配时
/// 仍会被识别为 PDF。
fn is_pdf(path: &Path) -> Result<bool, ToolError> {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        return Ok(true);
    }
    // 嗅探前 4 个字节。如果文件不存在则不报错——
    // 让调用者的 `read_to_string` 产生标准的未找到错误。
    let mut buf = [0u8; 4];
    let result = match fs::File::open(path) {
        Ok(mut f) => {
            use std::io::Read;
            f.read_exact(&mut buf).map(|_| buf)
        }
        Err(_) => return Ok(false),
    };
    Ok(matches!(result, Ok(b) if &b == b"%PDF"))
}

fn is_image_for_ocr(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "tif" | "tiff" | "bmp"
            )
        })
}

fn parse_pages_arg(spec: &str) -> Option<(u32, u32)> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((a, b)) = trimmed.split_once('-') {
        let start: u32 = a.trim().parse().ok()?;
        let end: u32 = b.trim().parse().ok()?;
        if start == 0 || end < start {
            return None;
        }
        Some((start, end))
    } else {
        let n: u32 = trimmed.parse().ok()?;
        if n == 0 {
            return None;
        }
        Some((n, n))
    }
}

/// 清理 PDF 提取的文本以供 TUI 显示：合并连续空白行
/// （超过 1 行变为 1 行），将 NUL 字节替换为 U+FFFD，
/// 将不换行空格替换为普通空格，并修剪每行末尾的空白。
/// 生成的输出不会用垂直间隙或不可见控制字符
/// 使会话记录变得杂乱。
fn clean_pdf_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut blank_run = 0usize;
    let mut any_content = false;
    for line in raw.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run = blank_run.saturating_add(1);
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            any_content = true;
            // 直接推送清理后的字符——
            // 避免每行临时分配 String。
            for c in trimmed.chars() {
                match c {
                    '\0' => out.push('\u{FFFD}'),
                    '\u{A0}' => out.push(' '),
                    other => out.push(other),
                }
            }
            out.push('\n');
        }
    }
    // 仅修剪前导空行——不要使用 str::trim()，
    // 因为它也会去掉有意的缩进（例如居中的标题）。
    if any_content {
        let start = out.find(|c: char| c != '\n').unwrap_or(0);
        // 从末尾往回走，找到最后一个非换行字符。
        let end = out.rfind(|c: char| c != '\n').map_or(out.len(), |i| {
            i + out[i..].chars().next().map_or(1, |c| c.len_utf8())
        });
        out[start..end].to_string()
    } else {
        String::new()
    }
}

fn read_pdf(path: &Path, pages: Option<&str>) -> Result<ToolResult, ToolError> {
    // 提前验证一次 `pages` 参数，
    // 这样两个提取路径在输入错误时都会产生相同的错误格式。
    let page_range = match pages {
        Some(spec) => match parse_pages_arg(spec) {
            Some((start, end)) => Some((start, end)),
            None => {
                return Err(ToolError::invalid_input(format!(
                    "invalid `pages` value `{spec}` (expected `N` or `N-M`, e.g. `1-5`)"
                )));
            }
        },
        None => None,
    };

    // 默认使用捆绑的纯 Rust `pdf-extract` 读取器：
    // 它移除了困扰每个新用户的安装 poppler 的前置条件，
    // 且该 crate 已经是工作区依赖（`web_run` 的 URL 获取路径也在使用）。
    // 对于列密集/复杂表格的 PDF（学术论文、财务文件），
    // 用户可以通过在 `~/.codewhale/settings.toml` 中设置
    // `prefer_external_pdftotext = true` 来选择使用历史上的
    // `pdftotext -layout` 路径
    // （旧版：`~/.config/deepseek/settings.toml`）。
    let prefer_external = crate::settings::Settings::load()
        .map(|s| s.prefer_external_pdftotext)
        .unwrap_or(false);

    if prefer_external {
        read_pdf_via_pdftotext(path, page_range)
    } else {
        #[cfg(feature = "pdf")]
        {
            read_pdf_via_pdf_extract(path, page_range)
        }
        #[cfg(not(feature = "pdf"))]
        {
            read_pdf_via_pdftotext(path, page_range)
        }
    }
}

#[cfg(feature = "pdf")]
fn read_pdf_via_pdf_extract(
    path: &Path,
    page_range: Option<(u32, u32)>,
) -> Result<ToolResult, ToolError> {
    let text = if let Some((start, end)) = page_range {
        // 逐页提取，这样我们可以切出请求的窗口，
        // 而不必将每一页都拖入调用者上下文。
        // pdf-extract 按文档顺序返回页面；`start`/`end` 是基于 1 的闭区间
        // （上面已验证过），因此我们转换为
        // 基于 0 的半开区间切片并进行边界钳制。
        let pages = guard_pdf_extract(|| pdf_extract::extract_text_by_pages(path)).map_err(|e| {
            ToolError::execution_failed(format!(
                "pdf-extract failed on {}: {e} (set `prefer_external_pdftotext = true` in settings.toml to retry via pdftotext)",
                path.display()
            ))
        })?;
        let total = pages.len();
        if total == 0 {
            String::new()
        } else {
            let start_idx = (start as usize).saturating_sub(1).min(total);
            let end_idx = (end as usize).min(total);
            if start_idx >= end_idx {
                String::new()
            } else {
                pages[start_idx..end_idx].join("\n")
            }
        }
    } else {
        // 即使调用者想要所有页面，也调用 extract_text_by_pages：
        // extract_text 使用的内部代码路径可能在某些 PDF
        // 交叉引用表或字体编码上挂起（#2641）。
        // 逐页路径避免了该挂起问题，并在合并后产生相同的输出。
        guard_pdf_extract(|| pdf_extract::extract_text_by_pages(path))
            .map(|pages| pages.join("\n"))
            .map_err(|e| {
                ToolError::execution_failed(format!(
                    "pdf-extract failed on {}: {e} (set `prefer_external_pdftotext = true` in settings.toml to retry via pdftotext)",
                    path.display()
                ))
            })?
    };
    Ok(ToolResult::success(clean_pdf_text(&text)))
}

fn guard_pdf_extract<T, E, F>(extract: F) -> Result<T, String>
where
    E: Display,
    F: FnOnce() -> Result<T, E>,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(extract)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(err.to_string()),
        Err(payload) => Err(format!(
            "extractor panicked: {}",
            panic_payload_message(payload.as_ref())
        )),
    }
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn read_pdf_via_pdftotext(
    path: &Path,
    page_range: Option<(u32, u32)>,
) -> Result<ToolResult, ToolError> {
    let mut cmd = Command::new("pdftotext");
    cmd.arg("-layout");

    if let Some((start, end)) = page_range {
        cmd.arg("-f").arg(start.to_string());
        cmd.arg("-l").arg(end.to_string());
    }

    cmd.arg(path).arg("-"); // 输出到 stdout
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // 结构化的"二进制不可用"——
            // 仅在用户明确选择了外部路径时才可达。
            // 同时提示安装命令和树内默认方案。
            return ToolResult::json(&json!({
                "type": "binary_unavailable",
                "path": path.display().to_string(),
                "kind": "pdf",
                "reason": "pdftotext not installed (prefer_external_pdftotext = true in settings)",
                "hint": "install poppler (macOS: `brew install poppler`; Debian/Ubuntu: `apt install poppler-utils`) — or unset `prefer_external_pdftotext` to use the bundled pure-Rust extractor"
            }))
            .map_err(|e| {
                ToolError::execution_failed(format!("failed to serialize response: {e}"))
            });
        }
        Err(e) => {
            return Err(ToolError::execution_failed(format!(
                "failed to launch pdftotext: {e}"
            )));
        }
    };

    let output = child
        .wait_with_output()
        .map_err(|e| ToolError::execution_failed(format!("pdftotext failed to complete: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ToolError::execution_failed(format!(
            "pdftotext failed (exit {:?}): {stderr}",
            output.status.code()
        )));
    }

    let text = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(ToolResult::success(clean_pdf_text(&text)))
}

// === WriteFileTool ===

/// 用于向工作区写入 UTF-8 文件的工具。
pub struct WriteFileTool;

#[async_trait]
impl ToolSpec for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Write content to a UTF-8 file in the workspace. Use this instead of heredocs (`cat <<EOF > file`) or `echo > file` in `exec_shell` — diffs render inline and approval is handled cleanly. Creates or overwrites; parent directories are auto-created."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::WritesFiles,
            ToolCapability::Sandboxable,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Suggest
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = required_str(&input, "path")?;
        let file_content = required_str(&input, "content")?;

        let file_path = context.resolve_path(path_str)?;

        // 在覆盖之前对现有内容（如果有）拍照——
        // 用于在工具结果中渲染内联差异。
        let existed_before = file_path.exists();
        let prior_contents = if existed_before {
            fs::read_to_string(&file_path).unwrap_or_default()
        } else {
            String::new()
        };

        // 如有需要则创建父目录
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to create directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        crate::utils::write_atomic(&file_path, file_content.as_bytes()).map_err(|e| {
            ToolError::execution_failed(format!("Failed to write {}: {}", file_path.display(), e))
        })?;
        context.note_file_read(&file_path);

        let display = file_path.display().to_string();
        let diff = make_unified_diff(&display, &prior_contents, file_content);
        let summary = if existed_before {
            format!("Wrote {} bytes to {}", file_content.len(), display)
        } else {
            format!("Created {} ({} bytes)", display, file_content.len())
        };
        let body = if diff.is_empty() {
            format!("{summary}\n(no changes)")
        } else {
            format!("{diff}\n{summary}")
        };

        // 启用时，为写入的文件附加 LSP 诊断信息（#428）。
        let diag_block = lsp_diagnostics_for_paths(context, &[file_path]).await;
        let full_body = if diag_block.is_empty() {
            body
        } else {
            format!("{body}\n{diag_block}")
        };

        Ok(ToolResult::success(full_body))
    }
}

// === EditFileTool ===

/// 用于对文件进行搜索/替换编辑的工具。
pub struct EditFileTool;

#[async_trait]
impl ToolSpec for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Replace text in a single file via exact search/replace after the file has been read with `read_file` in this session. Use this instead of `sed -i` in `exec_shell` for one unambiguous in-place edit. `search` must match exactly one location by default; when no exact match is found the tool retries with leading-whitespace-tolerant fuzzy matching automatically. The optional `fuzz` parameter is accepted for backward compatibility and is no longer needed. Returns a compact unified diff, not the full file. For structural, multi-block, or cross-file changes, use `apply_patch` or `write_file` instead."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "search": {
                    "type": "string",
                    "description": "Exact text to search for, including whitespace, indentation, and newlines"
                },
                "replace": {
                    "type": "string",
                    "description": "Text to replace with"
                },
                "fuzz": {
                    "type": "boolean",
                    "description": "Deprecated: fuzzy fallback is now automatic. Accepted for backward compatibility but ignored."
                }
            },
            "required": ["path", "search", "replace"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::WritesFiles,
            ToolCapability::Sandboxable,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Suggest
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = required_str(&input, "path")?;
        let search = required_str(&input, "search")?;
        let replace = required_str(&input, "replace")?;
        let _fuzz = optional_bool(&input, "fuzz", false);

        if search == replace {
            return Err(ToolError::invalid_input(
                "search and replace are identical, no change intended",
            ));
        }

        let file_path = context.resolve_path(path_str)?;
        context.require_fresh_file_read(&file_path, path_str)?;

        let contents = fs::read_to_string(&file_path).map_err(|e| {
            ToolError::execution_failed(format!("Failed to read {}: {}", file_path.display(), e))
        })?;

        let count = contents.matches(search).count();
        let (updated, count, fuzz_kind) = if count == 0 {
            // 第一次回退：容忍缩进差异。
            let indent_matches = leading_whitespace_fuzzy_matches(&contents, search);
            match indent_matches.as_slice() {
                [(start, end)] => {
                    let mut updated = contents.clone();
                    updated.replace_range(*start..*end, replace);
                    (updated, 1, Some("indentation"))
                }
                [] => {
                    // 第二次回退：容忍排版标点漂移
                    // （智能引号、长破折号、不换行空格）。
                    // 处理复制粘贴失败场景：浏览器/聊天客户端
                    // 静默地将文件中实际包含的 ASCII 标点
                    // 替换成了 Unicode 标点。
                    let punct_matches = punctuation_normalized_matches(&contents, search);
                    match punct_matches.as_slice() {
                        [] => {
                            return Err(ToolError::execution_failed(format!(
                                "Search string not found in {}. Recovery: call read_file with path=\"{path_str}\" to inspect the current contents, then retry with a search string copied from the file.",
                                file_path.display(),
                            )));
                        }
                        [(start, end)] => {
                            let mut updated = contents.clone();
                            updated.replace_range(*start..*end, replace);
                            (updated, 1, Some("punctuation"))
                        }
                        _ => {
                            return Err(ToolError::execution_failed(format!(
                                "edit_file search is non-unique after punctuation normalization: matched {} locations in {}. Recovery: call read_file with path=\"{path_str}\" and retry with surrounding lines that make the search unique.",
                                punct_matches.len(),
                                file_path.display()
                            )));
                        }
                    }
                }
                _ => {
                    return Err(ToolError::execution_failed(format!(
                        "edit_file search is non-unique after indentation normalization: matched {} locations in {}. Recovery: call read_file with path=\"{path_str}\" and retry with surrounding lines that make the search unique.",
                        indent_matches.len(),
                        file_path.display()
                    )));
                }
            }
        } else if count > 1 {
            return Err(ToolError::execution_failed(format!(
                "edit_file search is non-unique: matched {count} locations in {}. \
                 Recovery: call read_file with path=\"{path_str}\" and retry with surrounding lines that make the search unique.",
                file_path.display()
            )));
        } else {
            (contents.replace(search, replace), count, None)
        };

        crate::utils::write_atomic(&file_path, updated.as_bytes()).map_err(|e| {
            ToolError::execution_failed(format!("Failed to write {}: {}", file_path.display(), e))
        })?;
        context.note_file_read(&file_path);

        let display = file_path.display().to_string();
        let diff = make_unified_diff(&display, &contents, &updated);
        let fuzz_note = match fuzz_kind {
            Some("indentation") => " (fuzzy indentation match)",
            Some("punctuation") => {
                " (fuzzy punctuation match — typographic quotes/dashes normalized)"
            }
            Some(other) => other,
            None => "",
        };
        let summary = format!("Replaced {count} occurrence in {display}{fuzz_note}");
        let body = if diff.is_empty() {
            format!("{summary}\n(no textual changes)")
        } else {
            format!("{diff}\n{summary}")
        };

        // 启用时，为编辑的文件附加 LSP 诊断信息（#428）。
        let diag_block = lsp_diagnostics_for_paths(context, &[file_path]).await;
        let full_body = if diag_block.is_empty() {
            body
        } else {
            format!("{body}\n{diag_block}")
        };

        Ok(ToolResult::success(full_body))
    }
}

fn strip_line_leading_whitespace_with_map(input: &str) -> (String, Vec<usize>) {
    let mut normalized = String::with_capacity(input.len());
    let mut byte_map = Vec::with_capacity(input.len());
    let mut at_line_start = true;
    for (idx, ch) in input.char_indices() {
        if at_line_start && matches!(ch, ' ' | '\t') {
            continue;
        }
        normalized.push(ch);
        for _ in 0..ch.len_utf8() {
            byte_map.push(idx);
        }
        at_line_start = ch == '\n';
    }
    (normalized, byte_map)
}

fn line_start_before(input: &str, idx: usize) -> usize {
    input[..idx]
        .rfind('\n')
        .map_or(0, |newline| newline.saturating_add(1))
}

fn next_char_boundary(input: &str, idx: usize) -> usize {
    if idx >= input.len() {
        return input.len();
    }

    let mut next = idx.saturating_add(1);
    while next < input.len() && !input.is_char_boundary(next) {
        next = next.saturating_add(1);
    }
    next
}

fn leading_whitespace_fuzzy_matches(contents: &str, search: &str) -> Vec<(usize, usize)> {
    let (normalized_contents, byte_map) = strip_line_leading_whitespace_with_map(contents);
    let (normalized_search, _) = strip_line_leading_whitespace_with_map(search);
    if normalized_search.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let mut cursor = 0;
    while let Some(rel_idx) = normalized_contents[cursor..].find(&normalized_search) {
        let norm_start = cursor + rel_idx;
        let norm_end = norm_start + normalized_search.len();
        let Some(&mapped_start) = byte_map.get(norm_start) else {
            break;
        };
        // 使用实际的匹配起始位置，仅在规范化文本中
        // 匹配始于行边界时才扩展到行首。
        // 这可以防止在去掉空白后匹配从行中间开始时，
        // 破坏同一行前面的文本。
        let original_start =
            if norm_start == 0 || normalized_contents.as_bytes()[norm_start - 1] == b'\n' {
                // 匹配始于行边界——使用行首进行整行替换。
                line_start_before(contents, mapped_start)
            } else {
                // 匹配始于行中间——使用精确映射位置。
                mapped_start
            };
        let original_end = byte_map.get(norm_end).copied().unwrap_or(contents.len());
        matches.push((original_start, original_end));
        cursor = next_char_boundary(&normalized_contents, norm_start);
    }
    matches
}

/// 将排版标点规范化为其 ASCII 对应字符：
///
/// * `"` `"` / U+201C U+201D → `"`
/// * `'` `'` / U+2018 U+2019 → `'`
/// * `–` `—` / U+2013 U+2014 → `-`
/// * U+00A0（不换行空格）→ ASCII 空格
///
/// 返回规范化后的字符串和一个大小为 `normalized.len()` 的字节映射，
/// 其中第 i 个条目是产生规范化字节 i 的字符的原始字节偏移量。
/// 用于在规范化空间中找到匹配后恢复原始字节范围。
fn punctuation_normalized_with_map(input: &str) -> (String, Vec<usize>) {
    let mut normalized = String::with_capacity(input.len());
    let mut byte_map = Vec::with_capacity(input.len());
    for (idx, ch) in input.char_indices() {
        let replacement: Option<char> = match ch {
            '\u{201C}' | '\u{201D}' => Some('"'),
            '\u{2018}' | '\u{2019}' => Some('\''),
            '\u{2013}' | '\u{2014}' => Some('-'),
            '\u{00A0}' => Some(' '),
            _ => None,
        };
        let written = replacement.unwrap_or(ch);
        normalized.push(written);
        for _ in 0..written.len_utf8() {
            byte_map.push(idx);
        }
    }
    (normalized, byte_map)
}

/// 在对两者都进行排版标点规范化后，
/// 尝试在 `contents` 中找到 `search`。
/// 捕获复制粘贴失败场景：浏览器、文字处理器或聊天客户端
/// 静默地将 ASCII 引号/破折号转换为其 Unicode"美观"形式。
fn punctuation_normalized_matches(contents: &str, search: &str) -> Vec<(usize, usize)> {
    let (norm_contents, byte_map) = punctuation_normalized_with_map(contents);
    let (norm_search, _) = punctuation_normalized_with_map(search);
    if norm_search.is_empty() {
        return Vec::new();
    }
    // 如果规范化没有改变任何内容，
    // 精确匹配阶段已经考虑过这种情况——跳过以避免重复报告。
    if norm_contents == contents && norm_search == search {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let mut cursor = 0;
    while let Some(rel_idx) = norm_contents[cursor..].find(&norm_search) {
        let norm_start = cursor + rel_idx;
        let norm_end = norm_start + norm_search.len();
        let Some(&original_start) = byte_map.get(norm_start) else {
            break;
        };
        let original_end = byte_map.get(norm_end).copied().unwrap_or(contents.len());
        matches.push((original_start, original_end));
        cursor = next_char_boundary(&norm_contents, norm_start);
    }
    matches
}

// === ListDirTool ===

/// 用于列出目录内容的工具。
pub struct ListDirTool;

const LIST_DIR_TIMEOUT: Duration = Duration::from_secs(30);

/// 单次 `list_dir` 调用返回的条目上限，
/// 防止巨大目录（node_modules、构建输出、照片转储）使工具结果膨胀。
/// 镜像了 `read_file` 的 `HARD_MAX_READ_LINES` 的有界输出惯用做法。
/// 不超过上限的目录保持历史上的纯数组响应；
/// 更大的目录返回包含截断元数据的对象。
const LIST_DIR_MAX_ENTRIES: usize = 500;

#[async_trait]
impl ToolSpec for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn description(&self) -> &'static str {
        "List entries in a directory relative to the workspace. Use this instead of `ls`, `ls -la`, or `find . -maxdepth 1` in `exec_shell` for directory listings."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path (default: .)"
                }
            },
            "required": []
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = optional_str(&input, "path").unwrap_or(".");
        let dir_path = context.resolve_path(path_str)?;

        let entries =
            list_dir_entries_async(dir_path, context.cancel_token.clone(), LIST_DIR_TIMEOUT)
                .await?;

        ToolResult::json(&entries).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

async fn list_dir_entries_async(
    dir_path: PathBuf,
    cancel_token: Option<CancellationToken>,
    timeout: Duration,
) -> Result<Value, ToolError> {
    let worker_cancel_token = cancel_token.clone();
    run_blocking_list_dir(timeout, cancel_token, move || {
        list_dir_entries(&dir_path, worker_cancel_token.as_ref())
    })
    .await
}

async fn run_blocking_list_dir<F>(
    timeout: Duration,
    cancel_token: Option<CancellationToken>,
    list_dir: F,
) -> Result<Value, ToolError>
where
    F: FnOnce() -> Result<Value, ToolError> + Send + 'static,
{
    if cancel_token
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Err(list_dir_cancelled());
    }

    let task = tokio::task::spawn_blocking(list_dir);
    let result = match cancel_token {
        Some(token) => {
            tokio::select! {
                biased;
                () = token.cancelled() => return Err(list_dir_cancelled()),
                result = tokio::time::timeout(timeout, task) => result,
            }
        }
        None => tokio::time::timeout(timeout, task).await,
    };

    let joined = result.map_err(|_| list_dir_timeout(timeout))?;
    joined.map_err(|err| {
        ToolError::execution_failed(format!("list_dir worker failed before completion: {err}"))
    })?
}

fn list_dir_entries(
    dir_path: &Path,
    cancel_token: Option<&CancellationToken>,
) -> Result<Value, ToolError> {
    check_list_dir_cancelled(cancel_token)?;

    let mut entries = Vec::new();
    let mut total_entries = 0usize;

    for entry in fs::read_dir(dir_path).map_err(|e| {
        ToolError::execution_failed(format!(
            "Failed to read directory {}: {}",
            dir_path.display(),
            e
        ))
    })? {
        check_list_dir_cancelled(cancel_token)?;

        let entry = entry.map_err(|e| ToolError::execution_failed(e.to_string()))?;
        total_entries += 1;
        // 超过上限后，继续计数以获取截断元数据，
        // 但停止构建条目。
        if entries.len() >= LIST_DIR_MAX_ENTRIES {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;

        entries.push(json!({
            "name": entry.file_name().to_string_lossy().to_string(),
            "is_dir": file_type.is_dir(),
        }));
    }

    if total_entries > entries.len() {
        Ok(json!({
            "entries": entries,
            "listed_entries": LIST_DIR_MAX_ENTRIES,
            "total_entries": total_entries,
            "truncated": true,
        }))
    } else {
        Ok(Value::Array(entries))
    }
}

fn check_list_dir_cancelled(cancel_token: Option<&CancellationToken>) -> Result<(), ToolError> {
    if cancel_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(list_dir_cancelled());
    }
    Ok(())
}

fn list_dir_cancelled() -> ToolError {
    ToolError::execution_failed("list_dir cancelled before completion")
}

fn list_dir_timeout(timeout: Duration) -> ToolError {
    ToolError::Timeout {
        seconds: timeout.as_secs().max(1),
    }
}

// === 单元测试 ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn read_before_edit(ctx: &ToolContext, path: &str) {
        ReadFileTool
            .execute(json!({"path": path}), ctx)
            .await
            .expect("read before edit");
    }

    #[tokio::test]
    async fn test_read_file_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // 创建一个测试文件
        let test_file = tmp.path().join("test.txt");
        fs::write(&test_file, "hello world").expect("write");

        let tool = ReadFileTool;
        let result = tool
            .execute(json!({"path": "test.txt"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert_eq!(result.content, "hello world");
    }

    #[tokio::test]
    async fn read_file_ocr_extracts_text_from_image_when_backend_exists() {
        if !crate::tools::image_ocr::ocr_available() {
            return;
        }
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ocr_hello.png");
        if !fixture.exists() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        fs::copy(&fixture, tmp.path().join("ocr_hello.png")).expect("copy fixture");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let result = ReadFileTool
            .execute(json!({"path": "ocr_hello.png"}), &ctx)
            .await
            .expect("read image through OCR");

        assert!(result.success);
        assert!(result.content.contains("<image_ocr"));
        let normalized = result.content.to_uppercase();
        assert!(
            normalized.contains("HELLO") && normalized.contains("OCR"),
            "expected OCR text in read_file result, got {:?}",
            result.content
        );
    }

    #[test]
    fn parse_pages_arg_accepts_single_page() {
        assert_eq!(parse_pages_arg("3"), Some((3, 3)));
        assert_eq!(parse_pages_arg("  7  "), Some((7, 7)));
    }

    #[test]
    fn parse_pages_arg_accepts_range() {
        assert_eq!(parse_pages_arg("1-5"), Some((1, 5)));
        assert_eq!(parse_pages_arg("10-20"), Some((10, 20)));
        // 破折号两侧的空白是被容忍的，
        // 因此手写的 `pages: "1 - 5"` 仍然有效。
        assert_eq!(parse_pages_arg(" 1 - 5 "), Some((1, 5)));
    }

    #[test]
    fn parse_pages_arg_rejects_invalid_ranges() {
        // 否则调用者会传入 `pdftotext -f 5 -l 1`，
        // 它不会输出任何内容——大声失败以便模型可以重新发起。
        assert!(parse_pages_arg("5-1").is_none(), "end < start must reject");
        // pdftotext 中没有基于 0 的页码概念；
        // 拒绝它以避免调用者得到令人困惑的"无输出"静默失败。
        assert!(
            parse_pages_arg("0").is_none(),
            "zero single-page must reject"
        );
        assert!(parse_pages_arg("0-3").is_none(), "zero start must reject");
        // 空/仅空白/非数字输入必须拒绝。
        assert!(parse_pages_arg("").is_none());
        assert!(parse_pages_arg("   ").is_none());
        assert!(parse_pages_arg("abc").is_none());
        assert!(parse_pages_arg("3.5").is_none(), "floats must reject");
    }

    #[test]
    fn parse_pages_arg_rejects_half_open_ranges() {
        // 像 `1-` 或 `-5` 这样的半开范围几乎肯定是
        // `1-N`/`N` 的打字错误，而非有意输入。
        // 拒绝它们，而不是静默地扩展到 u32::MAX 或 0。
        assert!(parse_pages_arg("1-").is_none());
        assert!(parse_pages_arg("-5").is_none());
        assert!(parse_pages_arg("-").is_none());
    }

    #[test]
    fn parse_pages_arg_rejects_negative_numbers() {
        // u32::parse 对负数字面量返回 Err，
        // 因此函数返回 `None` 而不是包装成一个巨大的正数——
        // 防御性但值得固定测试。
        assert!(parse_pages_arg("-3-5").is_none());
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = ReadFileTool;
        let result = tool.execute(json!({"path": "nonexistent.txt"}), &ctx).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_file_small_file_returns_unwrapped_contents() {
        // 小文件（≤ 200 行且 ≤ 16KB，无显式范围）保持
        // 历史上"返回未修改内容"的行为，
        // 这样现有提示不会突然看到 <file> 标签出现。
        // 来自 #1451——固定快速路径契约。
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("small.txt");
        fs::write(&file, "line 1\nline 2\nline 3\n").expect("write");
        let tool = ReadFileTool;
        let result = tool
            .execute(json!({ "path": "small.txt" }), &ctx)
            .await
            .expect("execute");
        assert!(result.success);
        assert_eq!(result.content, "line 1\nline 2\nline 3\n");
        assert!(
            !result.content.contains("<file"),
            "small-file fast path must not wrap output"
        );
    }

    #[tokio::test]
    async fn read_file_explicit_range_wraps_in_file_tag_with_one_based_lines() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("ranged.txt");
        let body: String = (1..=10).map(|n| format!("line {n}\n")).collect();
        fs::write(&file, &body).expect("write");
        let tool = ReadFileTool;
        let result = tool
            .execute(
                json!({ "path": "ranged.txt", "start_line": 3, "max_lines": 4 }),
                &ctx,
            )
            .await
            .expect("execute");
        assert!(result.success);
        assert!(
            result.content.contains("shown_lines=\"3-6\""),
            "1-based inclusive range must be reflected in shown_lines: {}",
            result.content
        );
        assert!(
            result.content.contains("next_start_line=\"7\""),
            "next_start_line must point one past the last shown line: {}",
            result.content
        );
        assert!(
            result.content.contains("     3│ line 3"),
            "rendered lines must start at the requested line number"
        );
        assert!(
            result.content.contains("     6│ line 6"),
            "rendered lines must end at the last in-range line"
        );
        assert!(
            !result.content.contains("     7│ line 7"),
            "lines past max_lines must be excluded"
        );
        assert!(result.content.contains("truncated=\"true\""));
    }

    #[tokio::test]
    async fn read_file_range_beyond_total_returns_no_content_sentinel() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("short.txt");
        fs::write(&file, "only\nthree\nlines\n").expect("write");
        let tool = ReadFileTool;
        let result = tool
            .execute(json!({ "path": "short.txt", "start_line": 99 }), &ctx)
            .await
            .expect("execute");
        assert!(
            result.success,
            "out-of-range must not raise — it's a sentinel"
        );
        assert!(result.content.contains("[NO CONTENT]"));
        assert!(result.content.contains("shown_lines=\"none\""));
        assert!(result.content.contains("truncated=\"false\""));
    }

    #[tokio::test]
    async fn read_file_rejects_zero_start_line_and_zero_max_lines() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        fs::write(tmp.path().join("any.txt"), "x\n").expect("write");
        let tool = ReadFileTool;
        let zero_start = tool
            .execute(json!({ "path": "any.txt", "start_line": 0 }), &ctx)
            .await;
        assert!(zero_start.is_err(), "start_line=0 must error (1-based)");
        let zero_max = tool
            .execute(json!({ "path": "any.txt", "max_lines": 0 }), &ctx)
            .await;
        assert!(zero_max.is_err(), "max_lines=0 must error");
    }

    #[tokio::test]
    async fn read_file_clamps_max_lines_to_hard_cap() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("bigish.txt");
        let body: String = (1..=600).map(|n| format!("L{n}\n")).collect();
        fs::write(&file, &body).expect("write");
        let tool = ReadFileTool;
        let result = tool
            .execute(json!({ "path": "bigish.txt", "max_lines": 5000 }), &ctx)
            .await
            .expect("execute");
        // 硬上限是 500 行；第 500 行必须出现，第 501 行必须不出现。
        assert!(
            result.content.contains("   500│ L500"),
            "line 500 should be in the window (max_lines clamped to 500)"
        );
        assert!(
            !result.content.contains("   501│ L501"),
            "line 501 must be outside the clamped window"
        );
        assert!(result.content.contains("next_start_line=\"501\""));
        assert!(result.content.contains("truncated=\"true\""));
    }

    #[tokio::test]
    async fn read_file_large_file_without_range_uses_default_window() {
        // 超过 200 行/16KB 且没有显式范围的文件
        // 仍然获取默认窗口，而不是无限制的原始内容——
        // 这就是该补丁的全部意义（token 预算控制）。
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("big.txt");
        let body: String = (1..=250).map(|n| format!("row {n}\n")).collect();
        fs::write(&file, &body).expect("write");
        let tool = ReadFileTool;
        let result = tool
            .execute(json!({ "path": "big.txt" }), &ctx)
            .await
            .expect("execute");
        assert!(result.content.contains("<file "));
        assert!(result.content.contains("shown_lines=\"1-200\""));
        assert!(result.content.contains("next_start_line=\"201\""));
        assert!(result.content.contains("     1│ row 1"));
        assert!(result.content.contains("   200│ row 200"));
        assert!(
            !result.content.contains("   201│ row 201"),
            "default max_lines=200 must hold"
        );
    }

    #[tokio::test]
    async fn read_file_streamed_range_on_large_file_matches_windowed_contract() {
        // 超过 16KB 即使没有显式范围
        // 也会强制走流式 BufRead 路径；
        // 断言范围输出与历史上的全量读取实现保持字节兼容。
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("large.txt");
        let body: String = (1..=2000)
            .map(|n| format!("line {n} {}\n", "x".repeat(20)))
            .collect();
        assert!(body.len() > 16 * 1024, "fixture must exceed 16KB");
        fs::write(&file, &body).expect("write");

        let tool = ReadFileTool;
        let result = tool
            .execute(
                json!({ "path": "large.txt", "start_line": 1500, "max_lines": 10 }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("total_lines=\"2000\""));
        assert!(result.content.contains("shown_lines=\"1500-1509\""));
        assert!(result.content.contains("next_start_line=\"1510\""));
        assert!(result.content.contains("  1500│ line 1500"));
        assert!(result.content.contains("  1509│ line 1509"));
        assert!(!result.content.contains("  1510│"));
        assert!(result.content.contains(
            "[TRUNCATED] Showing lines 1500-1509 of 2000. To continue, call read_file with path=\"large.txt\" start_line=1510 max_lines=10"
        ));

        // 同一大文件上的默认窗口（无范围）从第 1 行开始。
        let default_window = tool
            .execute(json!({ "path": "large.txt" }), &ctx)
            .await
            .expect("execute");
        assert!(default_window.content.contains("shown_lines=\"1-200\""));
        assert!(default_window.content.contains("next_start_line=\"201\""));
        assert!(default_window.content.contains("     1│ line 1"));

        // 翻页到 EOF 之后返回无内容标记，而不是错误。
        let past_end = tool
            .execute(json!({ "path": "large.txt", "start_line": 5000 }), &ctx)
            .await
            .expect("execute");
        assert!(past_end.content.contains("[NO CONTENT]"));
        assert!(past_end.content.contains("shown_lines=\"none\""));
    }

    #[tokio::test]
    async fn read_file_streamed_range_rejects_invalid_utf8_like_full_read() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("mixed.bin");
        // 有效的前几行，后面是无效字节：
        // 流式路径仍必须像 read_to_string 那样使整个读取失败。
        let mut bytes = b"good line\n".repeat(5);
        bytes.extend_from_slice(&[0xFF, 0xFE, b'\n']);
        fs::write(&file, &bytes).expect("write");

        let err = ReadFileTool
            .execute(
                json!({ "path": "mixed.bin", "start_line": 1, "max_lines": 2 }),
                &ctx,
            )
            .await
            .expect_err("invalid UTF-8 must error");
        let message = err.to_string();
        assert!(message.contains("Failed to read"), "{message}");
        assert!(message.contains("valid UTF-8"), "{message}");
    }

    #[tokio::test]
    async fn test_read_file_missing_path() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = ReadFileTool;
        let result = tool.execute(json!({}), &ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("Failed to validate input: missing required field 'path'")
        );
    }

    #[test]
    fn pdf_detected_by_extension() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("paper.PDF");
        fs::write(&path, b"not really a pdf, but extension says yes").unwrap();
        assert!(is_pdf(&path).unwrap());
    }

    #[test]
    fn pdf_detected_by_magic_bytes_without_extension() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("blob");
        fs::write(&path, b"%PDF-1.7\nrest of bytes").unwrap();
        assert!(is_pdf(&path).unwrap());
    }

    #[test]
    fn non_pdf_not_detected() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("notes.txt");
        fs::write(&path, "hello").unwrap();
        assert!(!is_pdf(&path).unwrap());
    }

    #[test]
    fn pages_arg_parses_single_and_range() {
        assert_eq!(parse_pages_arg("5"), Some((5, 5)));
        assert_eq!(parse_pages_arg("1-10"), Some((1, 10)));
        assert_eq!(parse_pages_arg(" 3 - 7 "), Some((3, 7)));
        assert_eq!(parse_pages_arg("0"), None);
        assert_eq!(parse_pages_arg("10-3"), None);
        assert_eq!(parse_pages_arg(""), None);
        assert_eq!(parse_pages_arg("abc"), None);
    }

    /// 仓库附带的示例 PDF，用于与纯 Rust 提取器进行一致性测试。
    /// 38 页，数字原生 LaTeX（arXiv 2512.24601）。
    /// 路径相对于工作区根目录，
    /// 因为测试夹具位于 tui crate 外部。
    const SAMPLE_PDF_PATH: &str = "../../docs/2512.24601v2.pdf";

    fn sample_pdf_present() -> bool {
        std::path::Path::new(SAMPLE_PDF_PATH).exists()
    }

    #[test]
    fn clean_pdf_text_collapses_consecutive_blank_lines() {
        let raw = "line1\n\n\n\n\nline2\n\n\nline3";
        let cleaned = super::clean_pdf_text(raw);
        assert_eq!(cleaned, "line1\n\nline2\n\nline3");
    }

    #[test]
    fn clean_pdf_text_replaces_nul_bytes_with_replacement_char() {
        let raw = "hello\0world";
        let cleaned = super::clean_pdf_text(raw);
        assert!(!cleaned.contains('\0'));
        assert!(cleaned.contains('\u{FFFD}'));
    }

    #[test]
    fn clean_pdf_text_replaces_non_breaking_spaces() {
        let raw = "hello\u{A0}world";
        let cleaned = super::clean_pdf_text(raw);
        assert!(!cleaned.contains('\u{A0}'));
        assert_eq!(cleaned, "hello world");
    }

    #[test]
    fn clean_pdf_text_trims_trailing_whitespace() {
        let raw = "hello   ";
        let cleaned = super::clean_pdf_text(raw);
        assert_eq!(cleaned, "hello");
    }

    #[test]
    fn clean_pdf_text_preserves_leading_indentation() {
        let raw = "   indented line\nregular line";
        let cleaned = super::clean_pdf_text(raw);
        assert_eq!(cleaned, "   indented line\nregular line");
    }

    #[test]
    fn read_pdf_via_pdf_extract_finds_known_title() {
        // 当测试夹具未被检出时跳过
        // （稀疏克隆、浅工作树）。本地开发和 CI 都有它。
        if !sample_pdf_present() {
            // 测试夹具不存在（稀疏/浅检出）。
            // 静默跳过——`cargo test` 无论如何都报告同样的 `ok`。
            return;
        }
        let path = std::path::PathBuf::from(SAMPLE_PDF_PATH);
        let result = read_pdf_via_pdf_extract(&path, None).expect("extract whole PDF");
        assert!(result.success);
        assert!(
            result.content.contains("Recursive Language Models"),
            "pdf-extract should recover the document title; got prefix {:?}",
            result.content.chars().take(200).collect::<String>()
        );
    }

    #[test]
    fn read_pdf_via_pdf_extract_respects_pages_window() {
        if !sample_pdf_present() {
            // 测试夹具不存在（稀疏/浅检出）。
            // 静默跳过——`cargo test` 无论如何都报告同样的 `ok`。
            return;
        }
        let path = std::path::PathBuf::from(SAMPLE_PDF_PATH);
        let single = read_pdf_via_pdf_extract(&path, Some((1, 1))).expect("single page");
        let two = read_pdf_via_pdf_extract(&path, Some((1, 2))).expect("two pages");
        assert!(single.success);
        assert!(two.success);
        // 两页切片必须至少与一页切片一样长
        // （大多数文档在第 1 页之后都有非平凡的正文）。
        assert!(
            two.content.len() >= single.content.len(),
            "expected pages 1-2 ({} bytes) >= page 1 ({} bytes)",
            two.content.len(),
            single.content.len()
        );
        // 标题文本在第 1 页——必须在窗口裁剪后仍然存在。
        assert!(single.content.contains("Recursive Language Models"));
    }

    #[test]
    fn pdf_extract_panic_is_returned_as_tool_error_text() {
        let err = guard_pdf_extract(|| -> Result<String, &'static str> {
            panic!("assertion failed: name == \"Identity-H\"");
        })
        .expect_err("panic should become an error");

        assert!(err.contains("extractor panicked"));
        assert!(err.contains("Identity-H"));
    }

    #[tokio::test]
    async fn read_file_pdf_path_uses_pdf_extract_by_default() {
        if !sample_pdf_present() {
            // 测试夹具不存在（稀疏/浅检出）。
            // 静默跳过——`cargo test` 无论如何都报告同样的 `ok`。
            return;
        }
        // 测试夹具位于 tui crate 外部，因此我们将 ToolContext
        // 指向工作区根目录并通过相对路径读取。
        // 这会在捆绑提取器上执行完整的
        // ReadFileTool::execute → is_pdf → read_pdf 调度
        //（测试主机上无需 pdftotext）。
        let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../");
        let ctx = ToolContext::new(workspace);
        let result = ReadFileTool
            .execute(json!({"path": "docs/2512.24601v2.pdf", "pages": "1"}), &ctx)
            .await
            .expect("execute");
        assert!(result.success);
        assert!(
            result.content.contains("Recursive Language Models"),
            "page-1 extraction must surface the title"
        );
    }

    struct ConfigPathEnvGuard {
        prior: Option<std::ffi::OsString>,
    }
    impl ConfigPathEnvGuard {
        fn capture() -> Self {
            Self {
                prior: std::env::var_os("DEEPSEEK_CONFIG_PATH"),
            }
        }
    }
    impl Drop for ConfigPathEnvGuard {
        fn drop(&mut self) {
            // SAFETY: 限定在测试进程内；恢复为捕获的值。
            match &self.prior {
                Some(v) => unsafe { std::env::set_var("DEEPSEEK_CONFIG_PATH", v) },
                None => unsafe { std::env::remove_var("DEEPSEEK_CONFIG_PATH") },
            }
        }
    }

    #[test]
    fn read_pdf_routes_to_pdftotext_when_setting_opted_in() {
        // 一个测试中的两个关注点：
        // 当 `prefer_external_pdftotext = true` 时，
        // 调度必须 (a) 在 pdftotext 存在时调用它，
        // 以及 (b) 在 pdftotext 缺失时返回结构化的 `binary_unavailable` 响应。
        // 同步测试（直接调用 `read_pdf`，而非异步的 ReadFileTool 包装器），
        // 这样 env-var 锁永远不会跨 `.await` 持有。
        // 必须持有进程级环境锁，而非模块级锁：
        // 其他测试模块在 `lock_test_env` 下重定向
        // `DEEPSEEK_CONFIG_PATH`/`HOME`，
        // 模块级互斥锁会使此测试的重定向与它们的交错。
        let _lock = crate::test_support::lock_test_env();
        let _guard = ConfigPathEnvGuard::capture();

        let tmp = tempdir().expect("tempdir");
        let config_dir = tmp.path().join("cfg");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.toml");
        fs::write(&config_path, "").unwrap();
        // 同级的 settings.toml 是 Settings::load() 读取的文件。
        fs::write(
            config_dir.join("settings.toml"),
            "prefer_external_pdftotext = true\n",
        )
        .unwrap();
        // SAFETY: 由进程级测试环境锁序列化；由 guard 恢复。
        unsafe {
            std::env::set_var("DEEPSEEK_CONFIG_PATH", &config_path);
        }

        let pdf_path = tmp.path().join("doc.pdf");
        fs::write(&pdf_path, b"%PDF-1.7\n%%EOF").unwrap();
        let outcome = read_pdf(&pdf_path, None);

        let pdftotext_present = Command::new("pdftotext")
            .arg("-v")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok();

        if pdftotext_present {
            // pdftotext 在桩 `%PDF-1.7\n%%EOF` 上找不到真正的
            // trailer/xref 表并以 `exit 1` 失败。
            // 该失败文本显式提到 pdftotext——
            // 证明我们路由经过 Poppler 而不是回退到捆绑提取器。
            // 通过检查错误消息来验证。
            let err = outcome.expect_err("malformed PDF must surface the pdftotext error");
            let msg = err.to_string();
            assert!(
                msg.contains("pdftotext"),
                "error message must reference pdftotext; got {msg}"
            );
        } else {
            let result = outcome.expect("binary_unavailable is a structured success, not an Err");
            assert!(result.success);
            assert!(result.content.contains("binary_unavailable"));
            assert!(result.content.contains("pdftotext"));
            assert!(
                result.content.contains("prefer_external_pdftotext"),
                "hint must reference the opt-in flag the user set"
            );
        }
    }

    #[tokio::test]
    async fn test_write_file_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = WriteFileTool;
        let result = tool
            .execute(
                json!({"path": "output.txt", "content": "test content"}),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        // 新文件 → "Created …" 摘要；
        // 摘要上方的统一差异为 TUI 的差异感知渲染器做准备（#505）。
        assert!(result.content.contains("Created"), "{}", result.content);
        assert!(result.content.contains("--- a/"), "{}", result.content);
        assert!(
            result.content.contains("+test content"),
            "{}",
            result.content
        );

        // 验证文件已写入
        let written = fs::read_to_string(tmp.path().join("output.txt")).expect("read");
        assert_eq!(written, "test content");
    }

    #[tokio::test]
    async fn test_write_file_creates_dirs() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = WriteFileTool;
        let result = tool
            .execute(
                json!({"path": "subdir/nested/file.txt", "content": "nested content"}),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);

        // 验证嵌套文件已创建
        let written = fs::read_to_string(tmp.path().join("subdir/nested/file.txt")).expect("read");
        assert_eq!(written, "nested content");
    }

    #[tokio::test]
    async fn test_edit_file_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // 创建一个要编辑的文件
        let test_file = tmp.path().join("edit_me.txt");
        fs::write(&test_file, "hello world").expect("write");
        read_before_edit(&ctx, "edit_me.txt").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({"path": "edit_me.txt", "search": "hello", "replace": "hi"}),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("Replaced 1 occurrence"));
        // 内联差异（#505）——统一差异位于摘要行上方，
        // 以便 TUI 的差异感知渲染器生效。
        assert!(result.content.contains("--- a/"), "{}", result.content);
        assert!(
            result.content.contains("-hello world"),
            "{}",
            result.content
        );
        assert!(result.content.contains("+hi world"), "{}", result.content);

        // 验证编辑已生效
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(edited, "hi world");
    }

    #[tokio::test]
    async fn edit_file_requires_prior_read() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("blind.txt");
        fs::write(&test_file, "hello world").expect("write");

        let err = EditFileTool
            .execute(
                json!({"path": "blind.txt", "search": "hello", "replace": "hi"}),
                &ctx,
            )
            .await
            .expect_err("edit without read should fail");
        let message = err.to_string();
        assert!(message.contains("not been read"), "{message}");
        assert!(message.contains("read_file"), "{message}");

        let unchanged = fs::read_to_string(&test_file).expect("read");
        assert_eq!(unchanged, "hello world");
    }

    #[tokio::test]
    async fn edit_file_rejects_stale_prior_read() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("stale.txt");
        fs::write(&test_file, "alpha beta").expect("write");
        read_before_edit(&ctx, "stale.txt").await;
        fs::write(&test_file, "alpha beta gamma").expect("external write");

        let err = EditFileTool
            .execute(
                json!({"path": "stale.txt", "search": "alpha", "replace": "omega"}),
                &ctx,
            )
            .await
            .expect_err("stale read should fail");
        let message = err.to_string();
        assert!(message.contains("changed since"), "{message}");
        assert!(message.contains("read_file"), "{message}");

        let unchanged = fs::read_to_string(&test_file).expect("read");
        assert_eq!(unchanged, "alpha beta gamma");
    }

    #[tokio::test]
    async fn edit_file_rejects_non_unique_exact_match() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("multi.txt");
        fs::write(&test_file, "hello world hello").expect("write");
        read_before_edit(&ctx, "multi.txt").await;

        let err = EditFileTool
            .execute(
                json!({"path": "multi.txt", "search": "hello", "replace": "hi"}),
                &ctx,
            )
            .await
            .expect_err("non-unique exact match should fail");
        let message = err.to_string();
        assert!(message.contains("non-unique"), "{message}");
        assert!(message.contains("matched 2"), "{message}");
        assert!(message.contains("read_file"), "{message}");

        let unchanged = fs::read_to_string(&test_file).expect("read");
        assert_eq!(unchanged, "hello world hello");
    }

    #[tokio::test]
    async fn test_edit_file_accepts_omitted_and_explicit_fuzz() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = EditFileTool;

        for (file_name, fuzz) in [
            ("fuzz_omitted.txt", None),
            ("fuzz_false.txt", Some(false)),
            ("fuzz_true.txt", Some(true)),
        ] {
            let test_file = tmp.path().join(file_name);
            fs::write(&test_file, "hello world").expect("write");
            read_before_edit(&ctx, file_name).await;

            let mut input = serde_json::Map::from_iter([
                ("path".to_string(), json!(file_name)),
                ("search".to_string(), json!("hello")),
                ("replace".to_string(), json!("hi")),
            ]);
            if let Some(fuzz) = fuzz {
                input.insert("fuzz".to_string(), json!(fuzz));
            }

            let result = tool
                .execute(Value::Object(input), &ctx)
                .await
                .expect("execute");

            assert!(result.success, "{file_name}: {}", result.content);
            assert!(result.content.contains("Replaced 1 occurrence"));
            let edited = fs::read_to_string(&test_file).expect("read");
            assert_eq!(edited, "hi world");
        }
    }

    #[tokio::test]
    async fn test_edit_file_single_match_has_no_multi_match_warning() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("single.txt");
        fs::write(&test_file, "hello world").expect("write");
        read_before_edit(&ctx, "single.txt").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({"path": "single.txt", "search": "hello", "replace": "hi"}),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("Replaced 1 occurrence"));
        assert!(!result.content.contains("multiple matches were replaced"));
    }

    #[tokio::test]
    async fn test_edit_file_fuzz_tolerates_leading_whitespace() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("fuzzy.txt");
        fs::write(
            &test_file,
            "fn main() {\n    if true {\n        let value = 1;\n    }\n}\n",
        )
        .expect("write");
        read_before_edit(&ctx, "fuzzy.txt").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "fuzzy.txt",
                    "search": "if true {\n    let value = 1;\n}",
                    "replace": "    if true {\n        let value = 2;\n    }",
                    "fuzz": true
                }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("fuzzy indentation match"));
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(
            edited,
            "fn main() {\n    if true {\n        let value = 2;\n    }\n}\n"
        );
    }

    #[tokio::test]
    async fn test_edit_file_fuzz_tolerates_leading_whitespace_after_multibyte_start() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("fuzzy_cjk.txt");
        fs::write(&test_file, "数据\n").expect("write");
        read_before_edit(&ctx, "fuzzy_cjk.txt").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "fuzzy_cjk.txt",
                    "search": "    数据",
                    "replace": "记录",
                    "fuzz": true
                }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success, "{}", result.content);
        assert!(result.content.contains("fuzzy indentation match"));
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(edited, "记录\n");
    }

    #[tokio::test]
    async fn test_edit_file_fuzz_tolerates_smart_quote_substitution() {
        // 磁盘上的文件有 ASCII 引号。
        // 搜索内容来自浏览器的粘贴，带有花引号。
        // 精确匹配失败；标点规范化的回退应该仍然能够完成编辑。
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("smart.rs");
        fs::write(&test_file, "let s = \"hello world\";\n").expect("write");
        read_before_edit(&ctx, "smart.rs").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "smart.rs",
                    // \u{201C} \u{201D} 是花双引号对。
                    "search": "let s = \u{201C}hello world\u{201D};",
                    "replace": "let s = \"hello universe\";",
                    "fuzz": true
                }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success, "fuzzy punctuation edit should succeed");
        assert!(
            result.content.contains("fuzzy punctuation match"),
            "expected punctuation-fuzz note, got: {}",
            result.content
        );
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(edited, "let s = \"hello universe\";\n");
    }

    #[tokio::test]
    async fn test_edit_file_fuzz_tolerates_smart_quote_after_multibyte_start() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("smart_cjk.md");
        fs::write(&test_file, "数据 \"x\"\n").expect("write");
        read_before_edit(&ctx, "smart_cjk.md").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "smart_cjk.md",
                    "search": "数据 \u{201C}x\u{201D}",
                    "replace": "数据 y",
                    "fuzz": true
                }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success, "{}", result.content);
        assert!(result.content.contains("fuzzy punctuation match"));
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(edited, "数据 y\n");
    }

    #[tokio::test]
    async fn test_edit_file_fuzz_tolerates_em_dash_and_nbsp() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("dash.md");
        // 文件有一个 ASCII 连字符和 ASCII 空格。
        fs::write(&test_file, "alpha - beta\n").expect("write");
        read_before_edit(&ctx, "dash.md").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "dash.md",
                    // 搜索使用长破折号 + NBSP，
                    // 这在从样式文档复制粘贴后很常见。
                    "search": "alpha\u{00A0}\u{2014}\u{00A0}beta",
                    "replace": "alpha - gamma",
                    "fuzz": true
                }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(edited, "alpha - gamma\n");
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // 创建一个不包含搜索字符串的文件
        let test_file = tmp.path().join("no_match.txt");
        fs::write(&test_file, "foo bar baz").expect("write");
        read_before_edit(&ctx, "no_match.txt").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({"path": "no_match.txt", "search": "hello", "replace": "hi"}),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
        assert!(err.to_string().contains("read_file"));
    }

    #[tokio::test]
    async fn test_edit_file_rejects_identical_search_and_replace() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("same.txt");
        fs::write(&test_file, "a := \"foo\"").expect("write");

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "same.txt",
                    "search": "a := \"foo\"",
                    "replace": "a := \"foo\""
                }),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("search and replace are identical"),
            "error must explain the no-op input: {err}"
        );
        let unchanged = fs::read_to_string(&test_file).expect("read");
        assert_eq!(unchanged, "a := \"foo\"");
    }

/// #157 — 当模型使用 `replacement` 而不是 `replace` 时，
/// 错误应指出提供的字段名，
/// 以便模型无需第二次往返就能自我修正。
    #[tokio::test]
    async fn test_edit_file_wrong_param_name_shows_provided_fields() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("test.txt");
        fs::write(&test_file, "hello world").expect("write");

        let tool = EditFileTool;
        // 模型使用 `replacement` 而不是 `replace`。
        let result = tool
            .execute(
                json!({"path": "test.txt", "search": "hello", "replacement": "hi"}),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        // 错误必须同时指出缺失的字段和已提供的字段。
        assert!(
            err.contains("missing required field 'replace'"),
            "error must name the missing field: {err}"
        );
        assert!(
            err.contains("Input provided:") || err.contains("provided:"),
            "error must list the fields the model did supply: {err}"
        );
    }

    #[tokio::test]
    async fn test_list_dir_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // 创建一些文件和目录
        fs::write(tmp.path().join("file1.txt"), "").expect("write");
        fs::write(tmp.path().join("file2.txt"), "").expect("write");
        fs::create_dir(tmp.path().join("subdir")).expect("mkdir");

        let tool = ListDirTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");

        assert!(result.success);
        assert!(result.content.contains("file1.txt"));
        assert!(result.content.contains("file2.txt"));
        assert!(result.content.contains("subdir"));
        let entries: Value = serde_json::from_str(&result.content).expect("list_dir json");
        assert!(entries.as_array().expect("entries").iter().any(|entry| {
            entry.get("name").and_then(Value::as_str) == Some("subdir")
                && entry.get("is_dir").and_then(Value::as_bool) == Some(true)
        }));
    }

    #[tokio::test]
    async fn test_list_dir_with_path() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // 创建一个包含文件的子目录
        let subdir = tmp.path().join("mydir");
        fs::create_dir(&subdir).expect("mkdir");
        fs::write(subdir.join("nested.txt"), "").expect("write");

        let tool = ListDirTool;
        let result = tool
            .execute(json!({"path": "mydir"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("nested.txt"));
    }

    #[tokio::test]
    async fn test_list_dir_small_dir_keeps_plain_array_response() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        fs::write(tmp.path().join("only.txt"), "").expect("write");

        let tool = ListDirTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");

        let parsed: Value = serde_json::from_str(&result.content).expect("json");
        assert!(
            parsed.is_array(),
            "small dirs must keep the historical array shape: {parsed}"
        );
        assert_eq!(parsed.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_list_dir_caps_entries_with_truncation_metadata() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let extra = 7;
        for i in 0..LIST_DIR_MAX_ENTRIES + extra {
            fs::write(tmp.path().join(format!("f{i:04}.txt")), "").expect("write");
        }

        let tool = ListDirTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");

        let parsed: Value = serde_json::from_str(&result.content).expect("json");
        assert!(parsed.is_object(), "oversized dirs return an object");
        assert_eq!(parsed["truncated"], json!(true));
        assert_eq!(
            parsed["listed_entries"].as_u64().unwrap() as usize,
            LIST_DIR_MAX_ENTRIES
        );
        assert_eq!(
            parsed["total_entries"].as_u64().unwrap() as usize,
            LIST_DIR_MAX_ENTRIES + extra
        );
        assert_eq!(
            parsed["entries"].as_array().unwrap().len(),
            LIST_DIR_MAX_ENTRIES
        );
    }

    #[tokio::test]
    async fn test_list_dir_respects_cancel_token() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("file.txt"), "").expect("write");
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        let ctx = ToolContext::new(tmp.path().to_path_buf()).with_cancel_token(cancel_token);

        let tool = ListDirTool;
        let err = tool
            .execute(json!({}), &ctx)
            .await
            .expect_err("cancelled list_dir should return an error");

        assert!(
            format!("{err:?}").contains("cancelled"),
            "unexpected error: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_list_dir_blocking_wrapper_reports_timeout() {
        let err = run_blocking_list_dir(Duration::from_millis(1), None, || {
            std::thread::sleep(Duration::from_millis(50));
            Ok(Value::Array(Vec::new()))
        })
        .await
        .expect_err("slow list_dir worker should time out");

        assert!(
            matches!(err, ToolError::Timeout { seconds: 1 }),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn test_read_file_tool_properties() {
        let tool = ReadFileTool;
        assert_eq!(tool.name(), "read_file");
        assert!(tool.is_read_only());
        assert!(tool.is_sandboxable());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
    }

    #[test]
    fn test_write_file_tool_properties() {
        let tool = WriteFileTool;
        assert_eq!(tool.name(), "write_file");
        assert!(!tool.is_read_only());
        assert!(tool.is_sandboxable());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Suggest);
    }

    #[test]
    fn test_edit_file_tool_properties() {
        let tool = EditFileTool;
        assert_eq!(tool.name(), "edit_file");
        assert!(!tool.is_read_only());
        assert!(tool.is_sandboxable());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Suggest);
        assert!(tool.description().contains("exact search/replace"));
        assert!(tool.description().contains("structural"));
    }

    #[test]
    fn test_list_dir_tool_properties() {
        let tool = ListDirTool;
        assert_eq!(tool.name(), "list_dir");
        assert!(tool.is_read_only());
        assert!(tool.is_sandboxable());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
    }

    #[test]
    fn test_parallel_support_flags() {
        let read_tool = ReadFileTool;
        let list_tool = ListDirTool;
        let write_tool = WriteFileTool;

        assert!(read_tool.supports_parallel());
        assert!(list_tool.supports_parallel());
        assert!(!write_tool.supports_parallel());
    }

    #[test]
    fn test_input_schemas() {
        // 验证所有工具都有有效的 JSON schema
        let read_schema = ReadFileTool.input_schema();
        assert!(read_schema.get("type").is_some());
        assert!(read_schema.get("properties").is_some());

        let write_schema = WriteFileTool.input_schema();
        let required = write_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("write schema should include required array");
        assert!(required.iter().any(|v| v.as_str() == Some("path")));
        assert!(required.iter().any(|v| v.as_str() == Some("content")));

        let edit_schema = EditFileTool.input_schema();
        let required = edit_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("edit schema should include required array");
        let required_fields: Vec<_> = required.iter().filter_map(|value| value.as_str()).collect();
        assert_eq!(required_fields, vec!["path", "search", "replace"]);
        assert!(!required_fields.contains(&"fuzz"));
        assert_eq!(
            edit_schema["properties"]["fuzz"]["type"].as_str(),
            Some("boolean")
        );
        let search_desc = edit_schema["properties"]["search"]["description"]
            .as_str()
            .expect("search description");
        assert!(search_desc.contains("Exact text"));
        assert!(search_desc.contains("whitespace"));

        let list_schema = ListDirTool.input_schema();
        let required = list_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("list schema should include required array");
        assert!(required.is_empty()); // path 是可选的
    }
}
