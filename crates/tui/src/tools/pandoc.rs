//! `pandoc_convert` 工具 —— 通过 `pandoc` 二进制程序的通用文档转换
//! （<https://pandoc.org>）。
//!
//! Pandoc 是将散文在作者和工程师实际使用的格式之间转换的
//! 事实标准瑞士军刀：Markdown 到 HTML、HTML 到 Markdown、
//! 任何格式到 LaTeX 或 DOCX、RST 到 Markdown、
//! ReST 导入等。将其作为模型可调用的工具暴露出来，解锁了
//! 一大类"将此报告重写为……"/"将此变更日志发布为……"
//! 的工作流，这些工作流以前需要用户在回合之间
//! 进入终端。
//!
//! 注册由 [`crate::dependencies::resolve_pandoc`] 控制
//!（参见 [`crate::tools::registry::ToolRegistryBuilder::with_pandoc_tools`]）。
//! 当 pandoc 未安装时，工具根本不会出现在
//! 目录中，因此模型永远不会看到它实际上无法使用的二进制文件。
//!
//! ## 格式白名单
//!
//! Pandoc 支持约 30 种输入格式和约 50 种输出格式，将每一种
//! 都作为自由文本字符串暴露出来会让模型
//! 请求 `pdf`（需要安装 LaTeX）、`epub3`（任何地方都能工作，
//! 但与 `epub` 存在歧义）或 `markown` 等拼写错误。
//! 下面的白名单是精选子集，a) 覆盖了约 95%
//! 的真实文档处理需求，b) 不要求除 pandoc
//! 本身之外额外的系统依赖（LaTeX 引擎、ImageMagick）。
//!
//! 添加格式：追加到 [`SUPPORTED_TARGET_FORMATS`] 和
//! schema 描述中；调度逻辑是白名单驱动的，因此
//! 列表中的任何内容都原样通过。

use std::path::PathBuf;
use std::process::{Command, Stdio};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_str, required_str,
};

/// 精选的 pandoc 目标格式白名单。每个条目对应
/// pandoc 原生接受的 `--to=<format>` 值，无需
/// 额外的系统工具。保持此列表简短且有针对性 ——
/// 下面的 schema 描述直接引用它。
pub(crate) const SUPPORTED_TARGET_FORMATS: &[&str] = &[
    "markdown",   // Pandoc 风格的 Markdown（安全往返的默认值）
    "gfm",        // GitHub 风格的 Markdown
    "commonmark", // 严格 CommonMark
    "html",       // HTML5
    "rst",        // reStructuredText
    "latex",      // LaTeX 源码（*生成* 不需要安装 TeX）
    "docx",       // Microsoft Word .docx
    "odt",        // OpenDocument 文本
    "epub",       // EPUB 2/3
    "plain",      // 纯文本（格式化已剥离）
    "asciidoc",   // AsciiDoc
];

/// 实现 `pandoc_convert` 的工具。将源文件转换为
/// 目标格式，并将输出写入磁盘或内联返回
/// 转换后的文本。
pub struct PandocConvertTool;

#[async_trait]
impl ToolSpec for PandocConvertTool {
    fn name(&self) -> &'static str {
        "pandoc_convert"
    }

    fn description(&self) -> &'static str {
        "Convert a document between formats via pandoc. Reads `source_path` (any pandoc-supported input format — pandoc autodetects from extension), converts to `target_format`, and either writes the result to `output_path` (when provided) or returns the converted text inline. Supported targets: markdown, gfm, commonmark, html, rst, latex, docx, odt, epub, plain, asciidoc. Use this instead of shelling out to pandoc via `exec_shell` — no approval prompt for output_path-less reads, structured errors, and a curated format whitelist."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source_path": {
                    "type": "string",
                    "description": "Path to the source document (relative to workspace or absolute). Pandoc autodetects the input format from the file extension."
                },
                "target_format": {
                    "type": "string",
                    "description": "One of: markdown, gfm, commonmark, html, rst, latex, docx, odt, epub, plain, asciidoc.",
                    "enum": SUPPORTED_TARGET_FORMATS,
                },
                "output_path": {
                    "type": "string",
                    "description": "Optional path to write the converted document to. When omitted, the converted text is returned inline (text formats only — binary formats like docx/odt/epub require output_path)."
                }
            },
            "required": ["source_path", "target_format"]
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
        let source_path_str = required_str(&input, "source_path")?;
        let target_format = required_str(&input, "target_format")?.trim().to_lowercase();
        let output_path_str = optional_str(&input, "output_path");

        if !SUPPORTED_TARGET_FORMATS.contains(&target_format.as_str()) {
            return Err(ToolError::invalid_input(format!(
                "unsupported target_format `{target_format}`. Pick one of: {}",
                SUPPORTED_TARGET_FORMATS.join(", ")
            )));
        }

        let source_path = context.resolve_path(source_path_str)?;
        if !source_path.exists() {
            return Err(ToolError::execution_failed(format!(
                "source_path does not exist: {}",
                source_path.display()
            )));
        }

        let resolved_output_path: Option<PathBuf> = match output_path_str {
            Some(p) => Some(context.resolve_path(p)?),
            None => None,
        };

        // 二进制格式无法可靠地通过 stdout 往返——
        // 需要 output_path 以便字节能够完整传输。
        if resolved_output_path.is_none() && format_is_binary(&target_format) {
            return Err(ToolError::invalid_input(format!(
                "target_format `{target_format}` is binary; provide an `output_path` to write the converted file."
            )));
        }

        // 在执行时也解析 pandoc 二进制文件——注册
        // 依赖于 resolve_pandoc()，但在目录构建和模型调用之间的
        // 并发卸载应该产生清晰的错误，而不是
        // 从原始 Command::spawn 返回晦涩的"程序未找到"。
        let pandoc = crate::dependencies::resolve_pandoc().ok_or_else(|| {
            ToolError::execution_failed(
                "pandoc_convert: pandoc binary not found on PATH. \
                 Install pandoc (macOS: `brew install pandoc`; \
                 Debian/Ubuntu: `apt install pandoc`; \
                 Windows: `winget install JohnMacFarlane.Pandoc`) and restart codewhale.",
            )
        })?;

        let mut cmd = Command::new(&pandoc);
        cmd.arg(&source_path);
        cmd.arg("--to").arg(&target_format);
        if let Some(out) = resolved_output_path.as_ref() {
            cmd.arg("--output").arg(out);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd
            .output()
            .map_err(|e| ToolError::execution_failed(format!("failed to launch pandoc: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(ToolError::execution_failed(format!(
                "pandoc failed (exit {:?}): {stderr}",
                output.status.code()
            )));
        }

        let summary = if let Some(out) = resolved_output_path {
            format!(
                "Converted {} → {} via pandoc; wrote {}",
                source_path.display(),
                target_format,
                out.display()
            )
        } else {
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            return Ok(ToolResult::success(text));
        };
        Ok(ToolResult::success(summary))
    }
}

/// 输出为二进制的目标格式白名单（因此
/// 不能作为内联文本返回）。`docx`、`odt` 和 `epub` 是
/// ZIP 归档；[`SUPPORTED_TARGET_FORMATS`] 中的其他所有格式
/// 渲染为 UTF-8 文本。
pub(crate) fn format_is_binary(target_format: &str) -> bool {
    matches!(target_format, "docx" | "odt" | "epub")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn pandoc_present() -> bool {
        crate::dependencies::resolve_pandoc().is_some()
    }

    fn pandoc_environment_unavailable(err: &ToolError) -> bool {
        let msg = err.to_string();
        msg.contains("getXdgDirectory") || msg.contains("sHGetFolderPath")
    }

    // 仅测试用的跳过诊断；模块级的 print_stderr deny 针对的是生产代码。
    #[allow(clippy::print_stderr)]
    async fn execute_pandoc_or_skip(input: Value, ctx: &ToolContext) -> Option<ToolResult> {
        match PandocConvertTool.execute(input, ctx).await {
            Ok(result) => Some(result),
            Err(err) if pandoc_environment_unavailable(&err) => {
                eprintln!("skipping pandoc integration assertion: {err}");
                None
            }
            Err(err) => panic!("execute: {err:?}"),
        }
    }

    #[test]
    fn supported_target_formats_match_schema_enum() {
        let tool = PandocConvertTool;
        let schema = tool.input_schema();
        let enum_vals = schema
            .get("properties")
            .and_then(|p| p.get("target_format"))
            .and_then(|t| t.get("enum"))
            .and_then(|e| e.as_array())
            .expect("target_format enum must be present in schema");
        let from_schema: Vec<&str> = enum_vals.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(
            from_schema, SUPPORTED_TARGET_FORMATS,
            "schema enum must mirror the SUPPORTED_TARGET_FORMATS constant exactly",
        );
    }

    #[test]
    fn binary_formats_require_output_path() {
        for fmt in ["docx", "odt", "epub"] {
            assert!(format_is_binary(fmt));
        }
        for fmt in [
            "markdown",
            "html",
            "rst",
            "latex",
            "plain",
            "gfm",
            "commonmark",
        ] {
            assert!(!format_is_binary(fmt));
        }
    }

    #[tokio::test]
    async fn pandoc_convert_rejects_unsupported_target_format() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("in.md");
        fs::write(&src, "# hi").unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let err = PandocConvertTool
            .execute(
                json!({"source_path": "in.md", "target_format": "definitely-not-real"}),
                &ctx,
            )
            .await
            .expect_err("unsupported target format must reject before pandoc spawn");
        assert!(
            err.to_string().contains("unsupported target_format"),
            "error must call out the unsupported format; got {err}"
        );
    }

    #[tokio::test]
    async fn pandoc_convert_rejects_inline_request_for_binary_format() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("in.md");
        fs::write(&src, "# hi").unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let err = PandocConvertTool
            .execute(
                json!({"source_path": "in.md", "target_format": "docx"}),
                &ctx,
            )
            .await
            .expect_err("missing output_path for docx must reject");
        assert!(
            err.to_string().contains("binary") && err.to_string().contains("output_path"),
            "error must explain why output_path is required; got {err}"
        );
    }

    #[tokio::test]
    async fn pandoc_convert_roundtrips_markdown_to_html_inline() {
        if !pandoc_present() {
            // 没有 pandoc 工具就不会注册；镜像
            // 目录构建行为。
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("note.md");
        fs::write(&src, "# Title\n\nA paragraph with `inline code`.\n").unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let Some(result) = execute_pandoc_or_skip(
            json!({"source_path": "note.md", "target_format": "html"}),
            &ctx,
        )
        .await
        else {
            return;
        };
        assert!(result.success);
        assert!(
            result.content.contains("<h1") && result.content.contains("Title"),
            "html output must contain the heading; got {}",
            result.content
        );
        assert!(
            result.content.contains("<code") || result.content.contains("inline code"),
            "html output must preserve inline code; got {}",
            result.content
        );
    }

    #[tokio::test]
    async fn pandoc_convert_writes_output_path_and_reports_summary() {
        if !pandoc_present() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("note.md");
        fs::write(&src, "# Title\n").unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let Some(result) = execute_pandoc_or_skip(
            json!({
                "source_path": "note.md",
                "target_format": "html",
                "output_path": "out.html",
            }),
            &ctx,
        )
        .await
        else {
            return;
        };
        assert!(result.success);
        assert!(result.content.contains("wrote"));
        let written = fs::read_to_string(tmp.path().join("out.html")).expect("read");
        assert!(
            written.contains("Title"),
            "written file must contain converted body; got {written}"
        );
    }

    #[tokio::test]
    async fn pandoc_convert_surfaces_missing_source_path_clearly() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let err = PandocConvertTool
            .execute(
                json!({"source_path": "missing.md", "target_format": "html"}),
                &ctx,
            )
            .await
            .expect_err("nonexistent source must reject");
        assert!(
            err.to_string().contains("source_path") && err.to_string().contains("does not exist"),
            "error must call out missing source; got {err}"
        );
    }
}
