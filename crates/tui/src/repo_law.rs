//! 仓库法律保护的不可变性的机械强制执行。
//!
//! `.codewhale/constitution.json` 的不可变项以前是渲染到提示词中的建议性散文。
//! 现在，带有 `paths` 通配符的条目还会编译为在引擎工具门控中评估的写入保持
//! ——法律变成机制，附带命名不可变项的收据。
//!
//! 契约镜像了项目覆盖规则（"覆盖只能收紧"）：
//!
//! - 法律只能添加保持。模式中没有允许/放宽的形状，因此
//!   精心构造的 constitution 不能授予权限。
//! - `ask` 在所有模式下强制提示，包括 YOLO——如同内置安全基线，
//!   法律不能被模式绕过。`block` 直接拒绝。
//! - 任何失败（文件缺失、解析错误、通配符错误）会降级为更少或
//!   零条规则——绝不会产生被毒化的门控，绝不会对未受保护的路径进行保持。
//! - 只有仓库本地的 constitution 参与。用户全局的
//!   constitution 保持为建议性散文，永远不会到达此模块。

use std::path::Path;

use serde_json::Value;

use crate::project_context::{RepoLawAction, RepoLawRule, load_repo_law_rules};

/// 其输入指定了我们可以保持的文件系统写入目标的工具。任何
/// 具有写入能力的工具都必须在列表中——门控对于它不认识的工具会开放失败，
/// 因此没有条目的新写入工具会静默逃避仓库法律。
/// `fim_edit` 就是这样一个漏洞（它声明 WritesFiles，接受一个 `path`，
/// 并对其执行 `fs::write`），直到它被添加到这里。
const WRITE_TOOLS: &[&str] = &["write_file", "edit_file", "apply_patch", "fim_edit"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepoLawPlanDecision {
    /// 在所有模式下强制提示批准，命名该法律。
    ForcePrompt(String),
    /// 直接拒绝该调用，命名该法律。
    Block(String),
}

/// 针对提议的工具调用评估工作区的仓库法律。对于没有写入目标的工具、
/// 没有可执行法律的工作区以及每个受保护通配符之外的写入返回 `None`。
pub(crate) fn repo_law_plan_decision(
    workspace: &Path,
    tool_name: &str,
    tool_input: &Value,
) -> Option<RepoLawPlanDecision> {
    if !WRITE_TOOLS.contains(&tool_name) {
        return None;
    }
    let targets = write_target_paths(workspace, tool_input);
    if targets.is_empty() {
        return None;
    }
    let rules = load_repo_law_rules(workspace);
    if rules.is_empty() {
        return None;
    }

    // 最强操作在所有（规则，目标）匹配中胜出。
    let mut hold: Option<(&RepoLawRule, &str)> = None;
    for rule in &rules {
        for target in &targets {
            if rule.globs.is_match(target) {
                let stronger = matches!(rule.action, RepoLawAction::Block) || hold.is_none();
                let already_blocking = hold
                    .as_ref()
                    .is_some_and(|(held, _)| matches!(held.action, RepoLawAction::Block));
                if stronger && !already_blocking {
                    hold = Some((rule, target.as_str()));
                }
            }
        }
    }
    let (rule, target) = hold?;
    let protects = rule.patterns.join(", ");
    let reason = format!(
        "Repo law holds this write: \"{}\" protects {protects} (matched {target}, .codewhale/constitution.json)",
        rule.text
    );
    Some(match rule.action {
        RepoLawAction::Ask => RepoLawPlanDecision::ForcePrompt(reason),
        RepoLawAction::Block => RepoLawPlanDecision::Block(reason),
    })
}

/// 从工具输入中提取相对于工作区的写入目标。覆盖
/// `path`/`target`/`destination`/`file_path` 参数、`changes[].path` 以及
/// 补丁工具接受的每个 unified-diff / codex-envelope 头部形状——
/// 旧（`--- `）和新（`+++ `）路径，有或没有 `a/`/`b/` 前缀，
/// 去掉制表符时间戳后缀，`/dev/null`（删除）回退到对应路径。
/// 遗漏任何工具支持的形状都会导致保持绕过，
/// 因此这故意过度收集候选路径。
fn write_target_paths(workspace: &Path, input: &Value) -> Vec<String> {
    let mut targets = Vec::new();
    for key in ["path", "target", "destination", "file_path"] {
        if let Some(path) = input.get(key).and_then(Value::as_str) {
            push_normalized(&mut targets, workspace, path);
        }
    }
    if let Some(changes) = input.get("changes").and_then(Value::as_array) {
        for change in changes {
            if let Some(path) = change.get("path").and_then(Value::as_str) {
                push_normalized(&mut targets, workspace, path);
            }
        }
    }
    if let Some(patch) = input.get("patch").and_then(Value::as_str) {
        let mut pending_old: Option<String> = None;
        for line in patch.lines() {
            if let Some(rest) = line.strip_prefix("*** Update File: ") {
                push_normalized(&mut targets, workspace, rest.trim());
            } else if let Some(rest) = line.strip_prefix("*** Add File: ") {
                push_normalized(&mut targets, workspace, rest.trim());
            } else if let Some(rest) = line.strip_prefix("*** Delete File: ") {
                push_normalized(&mut targets, workspace, rest.trim());
            } else if let Some(rest) = line.strip_prefix("--- ") {
                // 旧路径：记住它，以便 `+++ /dev/null` 删除仍然
                // 保持对被删除文件的保护。
                pending_old = diff_header_path(rest);
                if let Some(ref p) = pending_old {
                    push_normalized(&mut targets, workspace, p);
                }
            } else if let Some(rest) = line.strip_prefix("+++ ") {
                match diff_header_path(rest) {
                    Some(new_path) => push_normalized(&mut targets, workspace, &new_path),
                    // `+++ /dev/null` → 删除；目标是旧路径。
                    None => {
                        if let Some(old) = pending_old.take() {
                            push_normalized(&mut targets, workspace, &old);
                        }
                    }
                }
            }
        }
    }
    targets.sort();
    targets.dedup();
    targets
}

/// 解析 unified-diff 头部路径：去掉可选的 `a/`/`b/` 前缀和
/// 制表符分隔的时间戳后缀。对于 `/dev/null`（不存在）返回 `None`。
fn diff_header_path(rest: &str) -> Option<String> {
    // 头部可能带有 "\t<timestamp>" 后缀；路径是第一个字段。
    let path = rest.split('\t').next().unwrap_or(rest).trim();
    if path.is_empty() || path == "/dev/null" {
        return None;
    }
    let stripped = path
        .strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path);
    Some(stripped.to_string())
}

/// 规范化为正斜杠、相对于工作区的字符串，以便编写为 `crates/x/**` 的通配符
/// 无论工具如何拼写路径都能匹配。关键的是，这以与写入工具的
/// `resolve_path` 相同的方式折叠 `.`/`..` 路径组件，因此内部的
/// `crates/./protocol/x` 或 `x/../crates/protocol/x` 无法绕过通配符
///（在此之前已有确认的绕过）。
fn push_normalized(targets: &mut Vec<String>, workspace: &Path, raw: &str) {
    let trimmed = raw.trim().replace('\\', "/");
    if trimmed.is_empty() {
        return;
    }
    // 当工具给出工作区内的绝对路径时，使其相对于工作区。
    let path = Path::new(&trimmed);
    let relative = path.strip_prefix(workspace).unwrap_or(path);

    // 词法上折叠 CurDir（`.`）和 ParentDir（`..`）组件，并
    // 去掉任何前导的根/空组件。工作区外的绝对路径
    // 保留其尾部（例如 `/etc/passwd` -> `etc/passwd`），以便
    // `**/passwd` 通配符仍然匹配，而工作区锚定的通配符则不匹配。
    let mut parts: Vec<String> = Vec::new();
    for component in relative.to_string_lossy().split('/') {
        match component {
            "" | "." => {}
            ".." => {
                // 弹出到根目录之上的 `..` 逃逸出工作区；保留
                // 一个显式标记，使其永远无法匹配工作区相对的通配符，
                // 而普通的批准/沙箱门控仍然约束它。
                if parts.pop().is_none() {
                    parts.push("..".to_string());
                }
            }
            other => parts.push(other.to_string()),
        }
    }
    let normalized = parts.join("/");
    if !normalized.is_empty() {
        targets.push(normalized);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn write_law(workspace: &Path, body: &str) {
        let dir = workspace.join(".codewhale");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("constitution.json"), body).unwrap();
    }

    const LAW: &str = r#"{
        "authority": ["AGENTS.md"],
        "protected_invariants": [
            "Keep DeepSeek support first-class.",
            { "text": "The wire format is frozen", "paths": ["crates/protocol/**"], "action": "block" },
            { "text": "Release notes need human review", "paths": ["CHANGELOG.md"] }
        ]
    }"#;

    #[test]
    fn advisory_only_law_never_holds() {
        let tmp = TempDir::new().unwrap();
        write_law(
            tmp.path(),
            r#"{"protected_invariants": ["Prose only, no paths."]}"#,
        );
        assert_eq!(
            repo_law_plan_decision(
                tmp.path(),
                "write_file",
                &json!({"path": "src/main.rs", "content": "x"}),
            ),
            None
        );
    }

    #[test]
    fn block_action_denies_protected_write() {
        let tmp = TempDir::new().unwrap();
        write_law(tmp.path(), LAW);
        let decision = repo_law_plan_decision(
            tmp.path(),
            "write_file",
            &json!({"path": "crates/protocol/wire.rs", "content": "x"}),
        );
        let Some(RepoLawPlanDecision::Block(reason)) = decision else {
            panic!("expected block, got {decision:?}");
        };
        assert!(reason.contains("The wire format is frozen"), "{reason}");
        assert!(reason.contains("crates/protocol/wire.rs"), "{reason}");
        assert!(reason.contains(".codewhale/constitution.json"), "{reason}");
    }

    #[test]
    fn ask_action_force_prompts_and_names_the_law() {
        let tmp = TempDir::new().unwrap();
        write_law(tmp.path(), LAW);
        let decision = repo_law_plan_decision(
            tmp.path(),
            "edit_file",
            &json!({"path": "CHANGELOG.md", "old": "a", "new": "b"}),
        );
        let Some(RepoLawPlanDecision::ForcePrompt(reason)) = decision else {
            panic!("expected force prompt, got {decision:?}");
        };
        assert!(
            reason.contains("Release notes need human review"),
            "{reason}"
        );
    }

    #[test]
    fn unprotected_writes_and_non_write_tools_pass() {
        let tmp = TempDir::new().unwrap();
        write_law(tmp.path(), LAW);
        assert_eq!(
            repo_law_plan_decision(
                tmp.path(),
                "write_file",
                &json!({"path": "src/main.rs", "content": "x"}),
            ),
            None
        );
        assert_eq!(
            repo_law_plan_decision(
                tmp.path(),
                "read_file",
                &json!({"path": "crates/protocol/wire.rs"}),
            ),
            None
        );
    }

    #[test]
    fn apply_patch_targets_are_extracted_from_all_shapes() {
        let tmp = TempDir::new().unwrap();
        write_law(tmp.path(), LAW);
        // changes[].path 形状
        let decision = repo_law_plan_decision(
            tmp.path(),
            "apply_patch",
            &json!({"changes": [{"path": "crates/protocol/msg.rs"}]}),
        );
        assert!(matches!(decision, Some(RepoLawPlanDecision::Block(_))));
        // unified diff 形状
        let decision = repo_law_plan_decision(
            tmp.path(),
            "apply_patch",
            &json!({"patch": "--- a/crates/protocol/msg.rs\n+++ b/crates/protocol/msg.rs\n@@\n"}),
        );
        assert!(matches!(decision, Some(RepoLawPlanDecision::Block(_))));
        // codex 信封形状
        let decision = repo_law_plan_decision(
            tmp.path(),
            "apply_patch",
            &json!({"patch": "*** Begin Patch\n*** Update File: crates/protocol/msg.rs\n*** End Patch\n"}),
        );
        assert!(matches!(decision, Some(RepoLawPlanDecision::Block(_))));
    }

    #[test]
    fn block_outranks_ask_when_both_match() {
        let tmp = TempDir::new().unwrap();
        write_law(
            tmp.path(),
            r#"{"protected_invariants": [
                { "text": "ask first", "paths": ["docs/**"] },
                { "text": "never", "paths": ["docs/frozen/**"], "action": "block" }
            ]}"#,
        );
        let decision = repo_law_plan_decision(
            tmp.path(),
            "write_file",
            &json!({"path": "docs/frozen/spec.md", "content": "x"}),
        );
        assert!(matches!(decision, Some(RepoLawPlanDecision::Block(_))));
    }

    #[test]
    fn absolute_and_dot_prefixed_paths_normalize_to_workspace_relative() {
        let tmp = TempDir::new().unwrap();
        write_law(tmp.path(), LAW);
        let absolute = tmp.path().join("crates/protocol/wire.rs");
        let decision = repo_law_plan_decision(
            tmp.path(),
            "write_file",
            &json!({"path": absolute.to_string_lossy(), "content": "x"}),
        );
        assert!(matches!(decision, Some(RepoLawPlanDecision::Block(_))));
        let decision = repo_law_plan_decision(
            tmp.path(),
            "write_file",
            &json!({"path": "./CHANGELOG.md", "content": "x"}),
        );
        assert!(matches!(
            decision,
            Some(RepoLawPlanDecision::ForcePrompt(_))
        ));
    }

    #[test]
    fn malformed_law_and_bad_globs_degrade_to_no_holds() {
        let tmp = TempDir::new().unwrap();
        write_law(tmp.path(), "{ not json");
        assert_eq!(
            repo_law_plan_decision(
                tmp.path(),
                "write_file",
                &json!({"path": "crates/protocol/wire.rs", "content": "x"}),
            ),
            None
        );
        write_law(
            tmp.path(),
            r#"{"protected_invariants": [
                { "text": "broken glob", "paths": ["crates/[invalid"] }
            ]}"#,
        );
        assert_eq!(
            repo_law_plan_decision(
                tmp.path(),
                "write_file",
                &json!({"path": "crates/protocol/wire.rs", "content": "x"}),
            ),
            None
        );
    }

    #[test]
    fn interior_dot_and_parent_segments_cannot_evade_a_block() {
        let tmp = TempDir::new().unwrap();
        write_law(tmp.path(), LAW);
        for path in [
            "crates/./protocol/wire.rs",
            "crates/../crates/protocol/wire.rs",
            "x/../crates/protocol/wire.rs",
            "./crates/protocol/wire.rs",
        ] {
            let decision = repo_law_plan_decision(
                tmp.path(),
                "write_file",
                &json!({ "path": path, "content": "x" }),
            );
            assert!(
                matches!(decision, Some(RepoLawPlanDecision::Block(_))),
                "{path} must be held, got {decision:?}"
            );
        }
    }

    #[test]
    fn fim_edit_is_gated_like_other_write_tools() {
        let tmp = TempDir::new().unwrap();
        write_law(tmp.path(), LAW);
        let decision = repo_law_plan_decision(
            tmp.path(),
            "fim_edit",
            &json!({ "path": "crates/protocol/wire.rs", "prefix": "a", "suffix": "b" }),
        );
        assert!(
            matches!(decision, Some(RepoLawPlanDecision::Block(_))),
            "{decision:?}"
        );
    }

    #[test]
    fn apply_patch_header_variants_are_all_extracted() {
        let tmp = TempDir::new().unwrap();
        write_law(tmp.path(), LAW);
        // 没有 a/ 或 b/ 前缀
        let d = repo_law_plan_decision(
            tmp.path(),
            "apply_patch",
            &json!({ "patch": "--- crates/protocol/wire.rs\n+++ crates/protocol/wire.rs\n@@\n" }),
        );
        assert!(
            matches!(d, Some(RepoLawPlanDecision::Block(_))),
            "no-prefix: {d:?}"
        );
        // 删除：+++ /dev/null，目标是旧路径
        let d = repo_law_plan_decision(
            tmp.path(),
            "apply_patch",
            &json!({ "patch": "--- a/crates/protocol/wire.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n" }),
        );
        assert!(
            matches!(d, Some(RepoLawPlanDecision::Block(_))),
            "deletion: {d:?}"
        );
        // 头部上的制表符时间戳后缀
        let d = repo_law_plan_decision(
            tmp.path(),
            "apply_patch",
            &json!({ "patch": "--- a/x\t2026-01-01\n+++ b/crates/protocol/wire.rs\t2026-01-01 10:00:00\n@@\n" }),
        );
        assert!(
            matches!(d, Some(RepoLawPlanDecision::Block(_))),
            "tab-timestamp: {d:?}"
        );
    }

    #[test]
    fn no_law_file_means_no_holds() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(
            repo_law_plan_decision(
                tmp.path(),
                "write_file",
                &json!({"path": "anything.rs", "content": "x"}),
            ),
            None
        );
    }
}
