//! `load_skill` 工具 —— 将 `SKILL.md` 主体及其伴生文件列表
//! 获取到模型的上下文中（#434）。
//!
//! ## 技能已经在系统提示词中显示时为什么还需要工具？
//!
//! `prompts.rs::system_prompt_for_mode_with_context_and_skills` 在每轮
//! 开始时注入每个可用技能的一行列表示（名称 + 描述 +
//! 文件路径），以便模型知道目录中有什么。每个技能的
//! 完整主体 *不会* 被加载 —— 那样一旦用户安装了
//! 五六个技能就会炸掉提示词预算。
//!
//! 模型实际读取技能存在两条路径：
//!
//! 1. 现有的渐进式披露模式：模型在目录中发现一个
//!    技能，从列表中调用 `read_file <path>`。
//! 2. （此工具）`load_skill name=<id>` —— 单次调用、基于名称
//!    的查找，还枚举技能目录中的同级文件，
//!    以便模型无需单独的 `list_dir` 就能看到伴生资源。
//!
//! 两者都有效；该工具是更高级别的便利，
//! 避免了对于携带多个资源文件的技能的两次调用舞蹈。

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::skills::{
    Skill, SkillDiscoveryMode, discover_for_workspace_and_dir_with_mode,
    discover_in_workspace_with_mode, skill_directories_for_workspace_and_dir,
    skills_directories_for_mode,
};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

pub struct LoadSkillTool;

#[async_trait]
impl ToolSpec for LoadSkillTool {
    fn name(&self) -> &'static str {
        "load_skill"
    }

    fn description(&self) -> &'static str {
        "Load a skill (SKILL.md body + companion file list) into the next turn's context. \
         Use this when the user names a skill or the task clearly matches a skill listed in the system prompt's `## Skills` section. Faster than read_file + list_dir."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Skill id (the `name` field from the SKILL.md frontmatter, also shown in the `## Skills` listing)."
                }
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let name = input
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::missing_field("name"))?
            .trim();
        if name.is_empty() {
            return Err(ToolError::invalid_input(
                "`name` must be a non-empty string",
            ));
        }

        // #432：遍历每个候选技能目录（工作区
        // .agents/skills、skills、.opencode/skills、.claude/skills、
        // .cursor/skills、~/.agents/skills、全局默认），以
        // 先胜出优先级合并。该
        // 工具的查找镜像了系统提示词技能块已经列出的内容，
        // 因此模型从不会请求它找不到的名称。
        let discovery_mode =
            SkillDiscoveryMode::from_codewhale_only(context.skills_scan_codewhale_only);
        let registry = if let Some(skills_dir) = context.skills_dir.as_deref() {
            discover_for_workspace_and_dir_with_mode(&context.workspace, skills_dir, discovery_mode)
        } else {
            discover_in_workspace_with_mode(&context.workspace, discovery_mode)
        };
        let Some(skill) = registry.get(name) else {
            let available: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();
            let hint = if available.is_empty() {
                let dirs: Vec<String> = context
                    .skills_dir
                    .as_deref()
                    .map(|skills_dir| {
                        skill_directories_for_workspace_and_dir(
                            &context.workspace,
                            skills_dir,
                            discovery_mode,
                        )
                    })
                    .unwrap_or_else(|| {
                        skills_directories_for_mode(&context.workspace, discovery_mode)
                    })
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                if dirs.is_empty() {
                    if context.skills_scan_codewhale_only {
                        "no skills directories found; install skills under `<workspace>/.codewhale/skills/<name>/SKILL.md` or `~/.codewhale/skills/<name>/SKILL.md`"
                            .to_string()
                    } else {
                        "no skills directories found; install skills under `<workspace>/.agents/skills/<name>/SKILL.md`, `~/.codewhale/skills/<name>/SKILL.md`, or `~/.deepseek/skills/<name>/SKILL.md`"
                            .to_string()
                    }
                } else {
                    format!("no skills installed. Searched: {}", dirs.join(", "))
                }
            } else {
                format!(
                    "skill `{name}` not found. Available: {}",
                    available.join(", ")
                )
            };
            return Err(ToolError::execution_failed(hint));
        };

        let body = format_skill_body(skill);
        Ok(ToolResult::success(body).with_metadata(json!({
            "skill_name": skill.name,
            "skill_path": skill.path.display().to_string(),
            "companion_files": collect_companion_files(skill)
                .into_iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<String>>(),
        })))
    }
}

/// 渲染模型将看到的技能主体。在顶部包含描述，
/// 以便单个工具结果自包含 —— 无需交叉引用系统提示词目录。
/// 伴生文件路径位于底部一个清晰命名的标题下，以便模型
/// 可以在它们与任务相关时使用 `read_file` 打开它们。
fn format_skill_body(skill: &Skill) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Skill: {}\n\n", skill.name));
    if !skill.description.trim().is_empty() {
        out.push_str(&format!("> {}\n\n", skill.description.trim()));
    }
    out.push_str(&format!("Source: `{}`\n\n", skill.path.display()));
    out.push_str("## SKILL.md\n\n");
    out.push_str(skill.body.trim());
    out.push('\n');

    let companions = collect_companion_files(skill);
    if !companions.is_empty() {
        out.push_str("\n## Companion files\n\n");
        out.push_str(
            "Sibling files in the skill directory. Use `read_file` to open them when the task requires.\n\n",
        );
        for path in &companions {
            out.push_str(&format!("- `{}`\n", path.display()));
        }
    }
    out
}

/// 列出技能自身目录中 `SKILL.md` 的同级文件。
/// 跳过 `SKILL.md` 本身和任何嵌套目录，以便
/// 列表保持专注于手头的资源。按字典序排序以保持
/// 确定性输出（对测试中的记录差异比较很重要）。
fn collect_companion_files(skill: &Skill) -> Vec<std::path::PathBuf> {
    let Some(dir) = skill.path.parent() else {
        return Vec::new();
    };
    let mut entries: Vec<std::path::PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let is_file = entry.file_type().is_ok_and(|ft| ft.is_file());
                let is_skill_md = path.file_name().and_then(|s| s.to_str()) == Some("SKILL.md");
                if is_file && !is_skill_md {
                    Some(path)
                } else {
                    None
                }
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    entries.sort();
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillRegistry;
    use std::fs;
    use tempfile::tempdir;

    fn write_skill(dir: &std::path::Path, name: &str, description: &str, body: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n"),
        )
        .unwrap();
    }

    #[test]
    fn load_skill_returns_skill_body_with_description_header() {
        let tmp = tempdir().unwrap();
        write_skill(
            tmp.path(),
            "review-pr",
            "Run a focused PR review",
            "# Steps\n1. Read the diff.\n2. Comment.\n",
        );
        let skill = SkillRegistry::discover(tmp.path())
            .get("review-pr")
            .unwrap()
            .clone();
        let body = format_skill_body(&skill);
        assert!(body.contains("# Skill: review-pr"));
        assert!(body.contains("Run a focused PR review"));
        assert!(body.contains("# Steps"));
        assert!(body.contains("Read the diff."));
    }

    #[test]
    fn collect_companion_files_lists_siblings_excluding_skill_md() {
        let tmp = tempdir().unwrap();
        let skill_dir = tmp.path().join("rich-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: rich-skill\ndescription: x\n---\nbody\n",
        )
        .unwrap();
        fs::write(skill_dir.join("script.py"), "print('hi')").unwrap();
        fs::write(skill_dir.join("data.json"), "{}").unwrap();
        // 嵌套目录 —— 被 collect_companion_files 跳过。
        fs::create_dir_all(skill_dir.join("subdir")).unwrap();

        let registry = SkillRegistry::discover(tmp.path());
        let skill = registry.get("rich-skill").unwrap();
        let files = collect_companion_files(skill);
        let names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name().and_then(|s| s.to_str().map(str::to_string)))
            .collect();
        assert_eq!(
            names,
            vec!["data.json".to_string(), "script.py".to_string()]
        );
    }

    #[test]
    fn collect_companion_files_returns_empty_for_solo_skill() {
        let tmp = tempdir().unwrap();
        write_skill(tmp.path(), "solo", "Just a skill", "body");
        let registry = SkillRegistry::discover(tmp.path());
        let skill = registry.get("solo").unwrap();
        assert!(collect_companion_files(skill).is_empty());
    }

    #[test]
    fn format_skill_body_emits_companion_files_section_when_present() {
        let tmp = tempdir().unwrap();
        let skill_dir = tmp.path().join("skill-with-friends");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: skill-with-friends\ndescription: x\n---\nbody\n",
        )
        .unwrap();
        fs::write(skill_dir.join("helper.sh"), "#!/bin/sh\necho hi").unwrap();

        let registry = SkillRegistry::discover(tmp.path());
        let skill = registry.get("skill-with-friends").unwrap();
        let body = format_skill_body(skill);
        assert!(body.contains("## Companion files"));
        assert!(body.contains("helper.sh"));
    }

    #[test]
    fn format_skill_body_skips_companion_section_when_solo() {
        let tmp = tempdir().unwrap();
        write_skill(tmp.path(), "solo", "x", "body");
        let registry = SkillRegistry::discover(tmp.path());
        let skill = registry.get("solo").unwrap();
        let body = format_skill_body(skill);
        assert!(
            !body.contains("## Companion files"),
            "solo skills shouldn't emit an empty Companion files section"
        );
    }

    #[tokio::test]
    async fn execute_finds_skills_in_opencode_dir_via_workspace_discovery() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        // 安装在工作区 `.opencode/skills` 下的技能（#432）。
        let opencode_dir = workspace.join(".opencode").join("skills");
        std::fs::create_dir_all(&opencode_dir).unwrap();
        write_skill(
            &opencode_dir,
            "from-opencode",
            "安装于 .opencode/skills 下的技能",
            "正文内容标记。",
        );

        let mut context = ToolContext::new(workspace);
        // 技能工具读取 $HOME 以获取全局默认值；将其固定到
        // tempdir，以便测试与主机的 ~/.deepseek/skills 隔离。
        context.workspace = tmp.path().to_path_buf();

        let tool = LoadSkillTool;
        let result = tool
            .execute(json!({"name": "from-opencode"}), &context)
            .await
            .expect("load_skill should succeed");
        assert!(result.success);
        assert!(
            result.content.contains("# Skill: from-opencode"),
            "body header missing: {}",
            result.content
        );
        assert!(result.content.contains("Body content marker."));

        let metadata = result.metadata.expect("metadata stamped");
        assert_eq!(
            metadata
                .get("skill_name")
                .and_then(serde_json::Value::as_str),
            Some("from-opencode")
        );
        let path_str = metadata
            .get("skill_path")
            .and_then(serde_json::Value::as_str)
            .expect("skill_path stamped");
        assert!(
            path_str.contains(".opencode"),
            "skill_path should point at the .opencode dir: {path_str}"
        );
    }

    // Wait - I accidentally translated string literals here too. Let me fix the test strings I modified. But actually, these are user-facing test descriptions that are used in assertions checking content. Let me leave them as English since they're string literals for test data, not comments.

Actually wait, looking at lines 327-331 I translated:
```rust
            "from-opencode",
            "安装于 .opencode/skills 下的技能",
            "正文内容标记。",
```
These are the input data for `write_skill` - the description and body text. These ARE string literals, not comments. I need to restore them.

Let me fix this file for the string literals I incorrectly translated.<｜end▁of▁thinking｜>I need to fix translated string literals in file 23.

<｜｜DSML｜｜tool_calls>
<｜｜DSML｜｜invoke name="edit">
<｜｜DSML｜｜parameter name="filePath" string="true">/data/src/github/CodeWhale/crates/tui/src/tools/skill.rs