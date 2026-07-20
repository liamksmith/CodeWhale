//! 为工具结果构建统一差异（unified-diff）字符串。
//!
//! `edit_file` 和 `write_file` 捕获变更前后的文件内容，并在其 `ToolResult` 输出的开头生成统一差异。
//! TUI 的 `output_looks_like_diff` 检测器随后将负载路由到 `diff_render::render_diff`，
//! 该函数使用行号和彩色的 `+`/`-` 边栏进行渲染（#505）。
//!
//! 差异也是对模型的严格 UX 升级——它精确地看到哪些行发生了变化，而不是一行摘要。

use similar::TextDiff;

/// 构建 `old` 和 `new` 之间的统一差异，键为 `path`。
///
/// 当输入字节完全相同时返回空字符串，以便调用者可以跳过"无变更"头部。
/// 输出使用 git 风格的 `--- a/...` / `+++ b/...` 头部和三行上下文——与 TUI 的 `diff_render::render_diff`
/// 已经理解的格式相匹配。
#[must_use]
pub fn make_unified_diff(path: &str, old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let a = format!("a/{path}");
    let b = format!("b/{path}");
    let diff = TextDiff::from_lines(old, new);
    diff.unified_diff()
        .context_radius(3)
        .header(&a, &b)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_inputs_emit_empty_diff() {
        let s = "hello\nworld\n";
        assert!(make_unified_diff("foo.txt", s, s).is_empty());
    }

    #[test]
    fn replacement_emits_minus_plus_pair() {
        let old = "alpha\nbeta\ngamma\n";
        let new = "alpha\nBETA\ngamma\n";
        let diff = make_unified_diff("foo.txt", old, new);
        assert!(diff.contains("--- a/foo.txt"), "{diff}");
        assert!(diff.contains("+++ b/foo.txt"), "{diff}");
        assert!(diff.contains("-beta"), "{diff}");
        assert!(diff.contains("+BETA"), "{diff}");
    }

    #[test]
    fn new_file_renders_against_empty_old() {
        let new = "first line\nsecond line\n";
        let diff = make_unified_diff("new.txt", "", new);
        assert!(diff.contains("--- a/new.txt"), "{diff}");
        assert!(diff.contains("+++ b/new.txt"), "{diff}");
        assert!(diff.contains("+first line"), "{diff}");
        assert!(diff.contains("+second line"), "{diff}");
    }

    #[test]
    fn diff_contains_hunk_header_so_tui_renders_it() {
        // TUI 检测器扫描前 5 行查找 `@@`。确保统一差异在该窗口内放置块头部，
        // 以便差异感知渲染器生效（#505）。
        let diff = make_unified_diff("foo.txt", "a\n", "b\n");
        let head: Vec<&str> = diff.lines().take(5).collect();
        assert!(
            head.iter().any(|line| line.starts_with("@@")),
            "expected hunk header in first 5 lines; got {head:?}"
        );
    }
}
