//! REPL 围栏提取工具函数。
//!
//! agent 的主循环扫描助手的文本中的 ` ```repl ` 围栏块，并将其提供给 [`crate::repl::runtime::PythonRuntime`]。
//! 捕获 `FINAL(...)` 和路由子 LLM RPC 都是在运行时内部通过 stdin/stdout 协议处理的——此处无需抓取。

/// 检查字符串是否包含 `` ```repl `` 围栏代码块。
pub fn has_repl_block(text: &str) -> bool {
    text.contains("```repl")
}

/// 从 `text` 中提取每个 `` ```repl `` 块及其字节偏移量。
pub fn extract_repl_blocks(text: &str) -> Vec<ReplBlock> {
    let mut blocks = Vec::new();
    let mut rest = text;

    while let Some(start_idx) = rest.find("```repl") {
        let after_fence = &rest[start_idx..];
        let code_start = after_fence.find('\n').unwrap_or(after_fence.len());
        let code_region = &after_fence[code_start..];
        let Some(end_offset) = code_region.find("\n```") else {
            break;
        };
        let code = code_region[..end_offset].to_string();
        let global_start = text.len() - rest.len() + start_idx;
        let global_end = global_start + code_start + end_offset + 3;
        blocks.push(ReplBlock {
            code,
            start_offset: global_start,
            end_offset: global_end,
        });
        rest = &after_fence[code_start + end_offset + 4..];
    }

    blocks
}

/// 一个带有字节偏移位置信息的 `` ```repl `` 代码块。
#[derive(Debug, Clone)]
pub struct ReplBlock {
    pub code: String,
    pub start_offset: usize,
    pub end_offset: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_repl_block_detects_fence() {
        assert!(has_repl_block("some text ```repl\ncode\n``` more"));
        assert!(!has_repl_block("no repl here ```python\ncode\n```"));
        assert!(!has_repl_block("just text"));
    }

    #[test]
    fn extract_repl_blocks_single() {
        let text = "before\n```repl\nprint('hello')\n```\nafter";
        let blocks = extract_repl_blocks(text);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].code.trim(), "print('hello')");
    }

    #[test]
    fn extract_repl_blocks_multiple() {
        let text = "```repl\ncode1\n```\nmid\n```repl\ncode2\n```\nend";
        let blocks = extract_repl_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].code.trim(), "code1");
        assert_eq!(blocks[1].code.trim(), "code2");
    }

    #[test]
    fn extract_repl_blocks_empty_when_none() {
        let blocks = extract_repl_blocks("no blocks here");
        assert!(blocks.is_empty());
    }
}
