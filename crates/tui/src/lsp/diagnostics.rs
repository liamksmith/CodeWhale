//! LSP 传输返回的诊断形状，以及渲染器，
//! 用于生成文件编辑后注入模型上下文的
//! `<diagnostics file="…">` 块。
//!
//! 格式（与 issue #136 中给出的规范匹配）：
//!
//! ```text
//! <diagnostics file="crates/tui/src/foo.rs">
//!   ERROR [12:8] missing semicolon
//!   ERROR [13:1] expected `,`, found `}`
//! </diagnostics>
//! ```
//!
//! 行号从 1 开始。列号从 1 开始。我们将每条诊断消息
//! 截断为单行，以便块保持紧凑。

use std::path::PathBuf;

/// 渲染块中使用严重级别分类。镜像 LSP 严重级别
/// 代码（1 = Error, 2 = Warning, 3 = Information, 4 = Hint）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    /// 解码 LSP 整数严重级别。当整数缺失或无法识别时返回 `None`——
    /// 调用者默认使用 `Error` 以优先暴露问题。
    #[must_use]
    pub fn from_lsp(code: Option<i64>) -> Option<Self> {
        match code? {
            1 => Some(Severity::Error),
            2 => Some(Severity::Warning),
            3 => Some(Severity::Information),
            4 => Some(Severity::Hint),
            _ => None,
        }
    }

    /// 渲染块中使用的大写标签。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "ERROR",
            Severity::Warning => "WARNING",
            Severity::Information => "INFO",
            Severity::Hint => "HINT",
        }
    }
}

/// 一条 LSP 诊断，归一化为 1-based 行/列，以便直接渲染。
/// 传输层负责 `0-based -> 1-based` 的转换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: u32,
    pub column: u32,
    pub severity: Severity,
    pub message: String,
}

impl Diagnostic {
    /// 将消息修剪为单行以实现紧凑渲染。
    fn render_message(&self) -> String {
        let first_line = self.message.lines().next().unwrap_or("").trim();
        first_line.to_string()
    }
}

/// 一个文件的诊断信息，准备好渲染。渲染器将列表
/// 限制为 `max_per_file` 项。
#[derive(Debug, Clone)]
pub struct DiagnosticBlock {
    /// 在 `file="…"` 属性中使用的路径。应尽可能相对于
    /// 工作区根目录（根据 issue 的硬性规则，如果相对化失败，
    /// 我们使用 `path.file_name()`）。
    pub file: PathBuf,
    pub items: Vec<Diagnostic>,
}

impl DiagnosticBlock {
    /// 以模块文档中描述的格式渲染块。当 `self.items` 为空时
    /// 返回空字符串，以便调用者可以在注入前进行
    /// `if !text.is_empty()` 检查。
    #[must_use]
    pub fn render(&self) -> String {
        if self.items.is_empty() {
            return String::new();
        }
        let file_attr = self.file.display();
        let mut out = format!("<diagnostics file=\"{file_attr}\">\n");
        for item in &self.items {
            out.push_str(&format!(
                "  {} [{}:{}] {}\n",
                item.severity.label(),
                item.line,
                item.column,
                item.render_message(),
            ));
        }
        out.push_str("</diagnostics>");
        out
    }

    /// 截断至最多 `max_per_file` 项，保持顺序。LSP 管理器
    /// 负责在调用此函数前按严重级别排序，以便在截断时
    /// 错误排在警告之前。
    pub fn truncate(&mut self, max_per_file: usize) {
        if self.items.len() > max_per_file {
            self.items.truncate(max_per_file);
        }
    }
}

/// 将一个 [`DiagnosticBlock`] 列表格式化为单个捆绑包。
/// 当引擎在一个轮次中触及多个文件时使用。空的块被跳过。
#[must_use]
pub fn render_blocks(blocks: &[DiagnosticBlock]) -> String {
    let mut chunks = Vec::new();
    for block in blocks {
        let rendered = block.render();
        if !rendered.is_empty() {
            chunks.push(rendered);
        }
    }
    chunks.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_decodes_lsp_codes() {
        assert_eq!(Severity::from_lsp(Some(1)), Some(Severity::Error));
        assert_eq!(Severity::from_lsp(Some(2)), Some(Severity::Warning));
        assert_eq!(Severity::from_lsp(Some(3)), Some(Severity::Information));
        assert_eq!(Severity::from_lsp(Some(4)), Some(Severity::Hint));
        assert_eq!(Severity::from_lsp(Some(99)), None);
        assert_eq!(Severity::from_lsp(None), None);
    }

    #[test]
    fn renders_block_in_required_format() {
        let block = DiagnosticBlock {
            file: PathBuf::from("crates/tui/src/foo.rs"),
            items: vec![
                Diagnostic {
                    line: 12,
                    column: 8,
                    severity: Severity::Error,
                    message: "missing semicolon".to_string(),
                },
                Diagnostic {
                    line: 13,
                    column: 1,
                    severity: Severity::Error,
                    message: "expected `,`, found `}`".to_string(),
                },
            ],
        };
        let rendered = block.render();
        assert!(rendered.contains("<diagnostics file=\"crates/tui/src/foo.rs\">"));
        assert!(rendered.contains("ERROR [12:8] missing semicolon"));
        assert!(rendered.contains("ERROR [13:1] expected `,`, found `}`"));
        assert!(rendered.ends_with("</diagnostics>"));
    }

    #[test]
    fn empty_block_renders_to_empty_string() {
        let block = DiagnosticBlock {
            file: PathBuf::from("foo.rs"),
            items: Vec::new(),
        };
        assert!(block.render().is_empty());
    }

    #[test]
    fn truncate_caps_to_max() {
        let mut block = DiagnosticBlock {
            file: PathBuf::from("foo.rs"),
            items: (0..30)
                .map(|i| Diagnostic {
                    line: i,
                    column: 1,
                    severity: Severity::Error,
                    message: format!("err {i}"),
                })
                .collect(),
        };
        block.truncate(20);
        assert_eq!(block.items.len(), 20);
    }

    #[test]
    fn renders_only_first_line_of_message() {
        let block = DiagnosticBlock {
            file: PathBuf::from("foo.rs"),
            items: vec![Diagnostic {
                line: 1,
                column: 1,
                severity: Severity::Error,
                message: "first line\nsecond line\nthird".to_string(),
            }],
        };
        let rendered = block.render();
        assert!(rendered.contains("first line"));
        assert!(!rendered.contains("second line"));
        assert!(!rendered.contains("third"));
    }
}
