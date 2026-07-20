//! 执行策略规则的命令匹配辅助函数。

use regex::Regex;

/// 通过 shlex 解析并重新拼接 Token 来归一化命令字符串。
///
/// 首先剥离 heredoc 正文（#419），这样像
/// `cat <<EOF > file.txt\nbody\nEOF` 这样的命令会在模式匹配
/// 之前折叠为 `cat > file.txt`。如果不这样做，`cat > file.txt`
/// 的 `auto_allow` 模式将无法匹配，因为 shlex 会将
/// 正文行也作为命令的一部分进行分词。
pub fn normalize_command(command: &str) -> String {
    let stripped = strip_heredoc_bodies(command);
    if let Some(tokens) = shlex::split(&stripped) {
        tokens.join(" ")
    } else {
        stripped
            .split_whitespace()
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// 从多行命令字符串中剥离 heredoc 正文。
///
/// 识别常见的格式：
///
/// * `<<DELIM` — 正文直到等于 `DELIM` 的行。
/// * `<<-DELIM` — 正文直到等于 `DELIM` 的行（实际 shell 中会去除制表符；
///   我们保持分隔符匹配相同）。
/// * `<<'DELIM'` / `<<"DELIM"` — 引号包裹的分隔符；关闭匹配时
///   会剥离引号。
///
/// Here-string 操作符 `<<<` 故意不被剥离——
/// 它的正文在同一个行的下一个 Token 上，而非单独的行，
/// shlex 可以正确分词。
fn strip_heredoc_bodies(command: &str) -> String {
    if !command.contains("<<") {
        return command.to_string();
    }
    // 通过将 here-string 操作符（`<<<`）替换为占位符来避开它，
    // 然后在运行 heredoc 正则表达式后恢复。Rust 的 `regex` crate
    // 不支持后顾断言，因此我们无法直接编写"仅当 `<<` 前面没有
    // `<` 时才匹配"；这种预处理实现了相同的效果。
    const HERESTRING_PLACEHOLDER: &str = "\u{0001}HERESTRING\u{0001}";
    let command_owned: String = command.replace("<<<", HERESTRING_PLACEHOLDER);
    let command: &str = &command_owned;

    // 惰性初始化 heredoc 起始正则表达式。允许 `<<` 和分隔符之间
    // 有空格 / `-`，接受分隔符名称周围的可选 `'` / `"`。
    // 分隔符是典型的 shell 标识符（字母数字 + 下划线）。
    static HEREDOC_RE_INIT: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = HEREDOC_RE_INIT.get_or_init(|| {
        Regex::new(r#"<<-?\s*(?:['"]?)([A-Za-z_][A-Za-z0-9_]*)(?:['"]?)"#)
            .expect("heredoc 正则表达式编译成功")
    });

    let mut out = String::with_capacity(command.len());
    let mut lines = command.lines();
    while let Some(line) = lines.next() {
        // 检测此行上的 heredoc，捕获分隔符，并从行中剥离 `<<DELIM`
        // 操作符，这样下游分词器就不会在模式中看到它。一行可以
        // 有多个 heredoc（罕见但合法：`cmd <<A <<B`）；
        // 我们会剥离该行上的每个匹配，并消耗直到
        // *最后* 一个分隔符（匹配的 shell 行为是堆叠它们，
        // 但为了模式匹配的目的，它们都会被折叠）。
        let mut delim: Option<String> = None;
        let mut redacted = line.to_string();
        for cap in re.captures_iter(line) {
            // 从行中剥离整个 `<<DELIM` 文本。
            let whole = cap.get(0).map_or("", |m| m.as_str());
            redacted = redacted.replace(whole, "");
            // 跟踪最后看到的分隔符，用于消耗正文。
            delim = cap.get(1).map(|m| m.as_str().to_string());
        }
        // 去除剥离后留下的多余空格。
        let cleaned = redacted
            .split_whitespace()
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&cleaned);
        out.push('\n');
        if let Some(d) = delim {
            // 跳过正文行，直到遇到匹配的分隔符。
            for body_line in lines.by_ref() {
                if body_line.trim() == d {
                    break;
                }
            }
        }
    }
    // 恢复我们在正则表达式匹配之前隐藏的 here-string 操作符。
    out.replace(HERESTRING_PLACEHOLDER, "<<<")
}

/// 如果模式匹配命令则返回 true。
///
/// 模式支持匹配任意子串的 `*` 通配符。
pub fn pattern_matches(pattern: &str, command: &str) -> bool {
    let pattern = normalize_command(pattern);
    let command = normalize_command(command);

    if pattern == "*" {
        return true;
    }

    let escaped = regex::escape(&pattern).replace("\\*", ".*");
    let Ok(re) = Regex::new(&format!("^{escaped}$")) else {
        return false;
    };
    re.is_match(&command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_command() {
        assert_eq!(normalize_command("git   status"), "git status");
        assert_eq!(
            normalize_command("git \"log --oneline\""),
            "git log --oneline"
        );
    }

    #[test]
    fn test_pattern_matches() {
        assert!(pattern_matches("git status", "git status"));
        assert!(pattern_matches("git log *", "git log --oneline"));
        assert!(pattern_matches("cargo *", "cargo test --all"));
        assert!(!pattern_matches("git push --force", "git push origin main"));
    }

    #[test]
    fn strip_heredoc_strips_simple_body() {
        let cmd = "cat <<EOF > file.txt\nhello\nworld\nEOF";
        let stripped = super::strip_heredoc_bodies(cmd);
        // 正文行 `hello` 和 `world` 消失了；分隔符
        // `EOF` 行也被消耗。
        assert!(!stripped.contains("hello"));
        assert!(!stripped.contains("world"));
        // 重定向目标保留。
        assert!(stripped.contains("> file.txt"));
    }

    #[test]
    fn strip_heredoc_handles_dash_form() {
        // 在实际 shell 中，`<<-EOF` 会去除前导制表符；
        // 出于匹配目的，我们仍然希望分隔符被消耗。
        let cmd = "cat <<-EOF > file.txt\n\tbody\nEOF";
        let stripped = super::strip_heredoc_bodies(cmd);
        assert!(!stripped.contains("body"));
        assert!(stripped.contains("> file.txt"));
    }

    #[test]
    fn strip_heredoc_handles_quoted_delimiter() {
        let cmd = "cat <<'END_OF_FILE' > out\nliteral $vars\nEND_OF_FILE";
        let stripped = super::strip_heredoc_bodies(cmd);
        assert!(!stripped.contains("literal $vars"));
        assert!(stripped.contains("> out"));
    }

    #[test]
    fn strip_heredoc_leaves_non_heredoc_commands_intact() {
        let cmd = "echo hello && ls";
        // 提前返回路径：输入中没有 `<<`，因此原始
        // 字符串原样通过（不添加尾随换行符）。
        assert_eq!(super::strip_heredoc_bodies(cmd), "echo hello && ls");
    }

    #[test]
    fn strip_heredoc_does_not_touch_here_string_operator() {
        // `<<<` 是 here-string；正文在同一个行上。
        // shlex 能够正确处理它——我们不应该尝试剥离
        // 任何内容，因为后续行中没有正文。
        let cmd = "grep foo <<< \"some text\"";
        let stripped = super::strip_heredoc_bodies(cmd);
        // 输出保留 `<<<` —— 内容未被剥离。
        assert!(stripped.contains("<<<"));
        assert!(stripped.contains("some text"));
    }

    #[test]
    fn normalize_command_strips_heredoc_for_pattern_matching() {
        // 端到端目标：用户的 `auto_allow = ["cat > file.txt"]`
        // 模式也能匹配 heredoc 形式。
        let normalized = normalize_command("cat <<EOF > file.txt\nbody\nEOF");
        assert!(pattern_matches("cat > file.txt", &normalized));
    }
}
