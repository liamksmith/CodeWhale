//! 审批指纹键（§5.A）。
//!
//! 与其仅按工具名称缓存（这会让已批准的 `exec_shell "cat foo"` 静默通过
//! `exec_shell "rm -rf /"`），审批流程使用 **调用指纹**——
//! 工具名称及其语义相关参数部分的摘要。
//!
//! ## 两种指纹形状
//!
//! 有两种键类型，用于决策的对立面：
//!
//! * [`build_approval_key`] —— 完整参数的 **精确** 摘要。
//!   用于划分 *拒绝* 的作用域，以便拒绝一个调用（例如 `rm -rf /tmp/x`）
//!   不会同时抑制对同一工具的后续不同调用（#1617）。
//!
//!   | 工具         | 精确键                                   |
//!   |-------------|------------------------------------------|
//!   | 文件写入     | `file:<tool_name>:<参数哈希>`               |
//!   | shell 工具   | `shell:<tool_name>:<参数哈希>`              |
//!   | `fetch_url`  | `net:<主机名>`                             |
//!   | 其他所有     | `tool:<tool_name>:<输入哈希>`               |
//!
//! * [`build_approval_grouping_key`] —— **松散/arity 感知的** 摘要。
//!   用于划分 *批准* 的作用域，以便批准会话中的 `cargo build`
//!   也覆盖 `cargo build --release`（v0.8.37 行为）。
//!
//!   | 工具           | 分组键                                   |
//!   |---------------|------------------------------------------|
//!   | `apply_patch`  | `patch:<文件路径哈希>`                     |
//!   | shell 工具    | `shell:<命令前缀>`                        |
//!   | `fetch_url`    | `net:<主机名>`                             |
//!   | 其他所有      | `tool:<tool_name>:<输入哈希>`              |
//!
use std::fmt::Write as _;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::command_safety::classify_command;

/// 工具调用的指纹——足够稳定以匹配重复调用，
/// 但足够具体以避免权限混淆。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApprovalKey(pub String);

/// 构建工具调用的审批缓存键。
///
/// 键包含工具名称和参数的标准摘要，
/// 以便拒绝一个调用会抑制完全相同的重试，而不是
/// 同一工具不同参数的后续调用。
#[must_use]
pub fn build_approval_key(tool_name: &str, input: &serde_json::Value) -> ApprovalKey {
    let fingerprint = match tool_name {
        "apply_patch" | "write_file" | "edit_file" | "fim_edit" => {
            format!("file:{tool_name}:{}", hash_json_value(input))
        }
        "exec_shell"
        | "task_shell_start"
        | "exec_shell_wait"
        | "exec_shell_interact"
        | "exec_wait"
        | "exec_interact" => {
            format!("shell:{tool_name}:{}", hash_json_value(input))
        }
        "fetch_url" | "web.fetch" | "web_fetch" => {
            let host = parse_host(input);
            format!("net:{host}")
        }
        _ => format!("tool:{tool_name}:{}", hash_json_value(input)),
    };
    ApprovalKey(fingerprint)
}

/// 构建工具调用的 **分组** 审批键。
///
/// 与 [`build_approval_key`] 不同，这将同一命令族的参数变体
/// 折叠到一个键上（v0.8.37 行为），以便"批准本次会话"的决策
/// 覆盖后续仅标志不同的调用。拒绝必须继续使用
/// 精确的 [`build_approval_key`]。
#[must_use]
pub fn build_approval_grouping_key(tool_name: &str, input: &serde_json::Value) -> ApprovalKey {
    let fingerprint = match tool_name {
        "apply_patch" => {
            let paths_hash = hash_patch_paths(input);
            format!("patch:{paths_hash}")
        }
        "exec_shell"
        | "task_shell_start"
        | "exec_shell_wait"
        | "exec_shell_interact"
        | "exec_wait"
        | "exec_interact" => {
            let prefix = command_prefix(input);
            format!("shell:{prefix}")
        }
        "fetch_url" | "web.fetch" | "web_fetch" => {
            let host = parse_host(input);
            format!("net:{host}")
        }
        _ => format!("tool:{tool_name}:{}", hash_json_value(input)),
    };
    ApprovalKey(fingerprint)
}

/// 返回 `input` 中 shell 命令的标准命令前缀。
///
/// 使用来自 arity 字典的 [`classify_command`]，以便批准
/// `git status` 也覆盖 `git status -s` / `git status --porcelain`，
/// 而不会同时覆盖 `git push`。
fn command_prefix(input: &serde_json::Value) -> String {
    let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    if tokens.is_empty() {
        return "<empty>".to_string();
    }
    classify_command(&tokens)
}

/// 对补丁输入引用的文件路径的排序集进行哈希。
fn hash_patch_paths(input: &serde_json::Value) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut paths: Vec<&str> = Vec::new();

    if let Some(changes) = input.get("changes").and_then(|v| v.as_array()) {
        for change in changes {
            if let Some(path) = change.get("path").and_then(|v| v.as_str()) {
                paths.push(path);
            }
        }
    } else if let Some(patch_text) = input.get("patch").and_then(|v| v.as_str()) {
        for line in patch_text.lines() {
            if let Some(rest) = line.strip_prefix("+++ b/") {
                paths.push(rest.trim());
            }
        }
    }

    paths.sort();
    paths.dedup();

    if paths.is_empty() {
        return "no_files".to_string();
    }

    let mut hasher = DefaultHasher::new();
    for path in &paths {
        path.hash(&mut hasher);
    }
    format!("{:x}", hasher.finish())
}

/// 从 URL 输入中解析主机部分。
fn parse_host(input: &serde_json::Value) -> String {
    let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("");

    if let Ok(parsed) = reqwest::Url::parse(url) {
        parsed.host_str().unwrap_or(url).to_string()
    } else {
        url.to_string()
    }
}

fn hash_json_value(value: &Value) -> String {
    let mut canonical = String::new();
    push_canonical_json(value, &mut canonical);

    let digest = Sha256::digest(canonical.as_bytes());
    let mut short = String::with_capacity(16);
    for byte in &digest[..8] {
        write!(&mut short, "{byte:02x}").expect("writing to String cannot fail");
    }
    short
}

fn push_canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(value) => {
            out.push_str("bool:");
            out.push_str(if *value { "true" } else { "false" });
        }
        Value::Number(value) => {
            out.push_str("number:");
            // 避免通过 value.to_string() 分配。
            if let Some(n) = value.as_f64() {
                let _ = write!(out, "{n}");
            } else if let Some(n) = value.as_i64() {
                let _ = write!(out, "{n}");
            } else if let Some(n) = value.as_u64() {
                let _ = write!(out, "{n}");
            } else {
                out.push_str(&value.to_string());
            }
        }
        Value::String(value) => {
            out.push_str("string:");
            // 无中间分配地发出 JSON 编码的字符串。
            out.push('"');
            for ch in value.chars() {
                match ch {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c.is_control() => {
                        let _ = write!(out, "\\u{:04x}", c as u32);
                    }
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                push_canonical_json(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);

            out.push('{');
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                let encoded_key =
                    serde_json::to_string(key).expect("serializing an object key cannot fail");
                out.push_str(&encoded_key);
                out.push(':');
                push_canonical_json(value, out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn different_commands_different_keys() {
        let key_a = build_approval_key("exec_shell", &json!({"command": "ls"}));
        let key_b = build_approval_key("exec_shell", &json!({"command": "rm -rf /tmp"}));
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn same_command_same_key() {
        let key_a = build_approval_key("exec_shell", &json!({"command": "cargo build --release"}));
        let key_b = build_approval_key("exec_shell", &json!({"command": "cargo build --release"}));
        assert_eq!(key_a, key_b);
    }

    #[test]
    fn shell_keys_include_full_command_arguments() {
        let key_a = build_approval_key("exec_shell", &json!({"command": "cargo build"}));
        let key_b = build_approval_key("exec_shell", &json!({"command": "cargo build --release"}));
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn grouping_key_collapses_shell_flag_variants() {
        let key_a = build_approval_grouping_key("exec_shell", &json!({"command": "cargo build"}));
        let key_b =
            build_approval_grouping_key("exec_shell", &json!({"command": "cargo build --release"}));
        assert_eq!(
            key_a, key_b,
            "approving a command family must cover later flag variants"
        );
    }

    #[test]
    fn grouping_key_still_separates_distinct_commands() {
        let key_a = build_approval_grouping_key("exec_shell", &json!({"command": "git status"}));
        let key_b = build_approval_grouping_key("exec_shell", &json!({"command": "git push"}));
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn grouping_key_collapses_patch_body_for_same_path() {
        let key_a = build_approval_grouping_key(
            "apply_patch",
            &json!({"changes": [{"path": "a.rs", "content": "x"}]}),
        );
        let key_b = build_approval_grouping_key(
            "apply_patch",
            &json!({"changes": [{"path": "a.rs", "content": "y"}]}),
        );
        assert_eq!(
            key_a, key_b,
            "approving a patch family must cover later edits to the same path"
        );
    }

    #[test]
    fn denial_key_stays_exact_while_grouping_key_collapses() {
        let exact_a = build_approval_key("exec_shell", &json!({"command": "cargo build"}));
        let exact_b =
            build_approval_key("exec_shell", &json!({"command": "cargo build --release"}));
        assert_ne!(exact_a, exact_b, "denials must remain exact-call scoped");

        let group_a = build_approval_grouping_key("exec_shell", &json!({"command": "cargo build"}));
        let group_b =
            build_approval_grouping_key("exec_shell", &json!({"command": "cargo build --release"}));
        assert_eq!(group_a, group_b, "approvals must group by command family");
    }

    #[test]
    fn patch_keys_differ_by_path() {
        let key_a = build_approval_key(
            "apply_patch",
            &json!({"changes": [{"path": "a.rs", "content": "x"}]}),
        );
        let key_b = build_approval_key(
            "apply_patch",
            &json!({"changes": [{"path": "b.rs", "content": "x"}]}),
        );
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn patch_keys_differ_by_body_for_same_path() {
        let key_a = build_approval_key(
            "apply_patch",
            &json!({"changes": [{"path": "a.rs", "content": "x"}]}),
        );
        let key_b = build_approval_key(
            "apply_patch",
            &json!({"changes": [{"path": "a.rs", "content": "y"}]}),
        );
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn net_keys_differ_by_host() {
        let key_a = build_approval_key("fetch_url", &json!({"url": "https://example.com"}));
        let key_b = build_approval_key("fetch_url", &json!({"url": "https://other.org"}));
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn generic_tool_keys_include_arguments() {
        let key_a = build_approval_key("read_file", &json!({"path": "a.txt"}));
        let key_b = build_approval_key("read_file", &json!({"path": "b.txt"}));
        assert_ne!(key_a, key_b);
        assert!(key_a.0.starts_with("tool:read_file:"));
    }

    #[test]
    fn generic_tool_same_arguments_reuse_key() {
        let input = json!({"path": "a.txt"});
        let key_a = build_approval_key("edit_file", &input);
        let key_b = build_approval_key("edit_file", &input);
        assert_eq!(key_a, key_b);
    }

    #[test]
    fn input_hash_is_stable_across_object_key_order() {
        let key_a = build_approval_key("write_file", &json!({"path": "a.txt", "content": "x"}));
        let key_b = build_approval_key("write_file", &json!({"content": "x", "path": "a.txt"}));
        assert_eq!(key_a, key_b);
    }

    #[test]
    fn canonical_json_omits_trailing_commas() {
        let mut canonical = String::new();
        push_canonical_json(&json!({"b": [true, false], "a": {"x": 1}}), &mut canonical);

        assert_eq!(
            canonical,
            r#"{"a":{"x":number:1},"b":[bool:true,bool:false]}"#
        );
        assert!(!canonical.contains(",]"));
        assert!(!canonical.contains(",}"));
    }
}
