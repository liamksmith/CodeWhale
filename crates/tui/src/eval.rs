//! 用于执行代表性工具循环的离线评估框架。
//!
//! 本模块有意保持自包含，以便日后可以接入 CLI 命令，
//! 而无需调用网络或任何 LLM 端点。

use anyhow::{Context, Result, anyhow};
use ignore::WalkBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvalShellPlatform {
    Windows,
    Unix,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct EvalShellInvocation {
    program: &'static str,
    args: Vec<String>,
    raw_payload_on_windows: bool,
}

#[cfg(test)]
fn eval_shell_invocation_for_platform(
    command: &str,
    platform: EvalShellPlatform,
) -> EvalShellInvocation {
    match platform {
        EvalShellPlatform::Windows => EvalShellInvocation {
            program: "cmd",
            args: vec!["/C".to_string(), command.to_string()],
            raw_payload_on_windows: true,
        },
        EvalShellPlatform::Unix => EvalShellInvocation {
            program: "sh",
            args: vec!["-c".to_string(), command.to_string()],
            raw_payload_on_windows: false,
        },
    }
}

/// 评估框架涵盖的代表性工具步骤。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum ScenarioStepKind {
    List,
    Read,
    Search,
    Edit,
    ApplyPatch,
    ExecShell,
}

impl ScenarioStepKind {
    /// 与此步骤关联的工具名称。
    pub fn tool_name(self) -> &'static str {
        match self {
            ScenarioStepKind::List => "list_dir",
            ScenarioStepKind::Read => "read_file",
            ScenarioStepKind::Search => "search",
            ScenarioStepKind::Edit => "edit_file",
            ScenarioStepKind::ApplyPatch => "apply_patch",
            ScenarioStepKind::ExecShell => "exec_shell",
        }
    }

    /// 从 CLI 友好的字符串解析步骤类型。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "list" | "list_dir" => Some(Self::List),
            "read" | "read_file" => Some(Self::Read),
            "search" | "grep" | "grep_files" => Some(Self::Search),
            "edit" | "edit_file" => Some(Self::Edit),
            "patch" | "apply_patch" => Some(Self::ApplyPatch),
            "shell" | "exec_shell" | "exec" => Some(Self::ExecShell),
            _ => None,
        }
    }
}

/// 单个工具类型的聚合统计数据。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ToolStats {
    pub invocations: usize,
    pub errors: usize,
    pub total_duration: Duration,
}

/// 评估运行产生的一级指标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvalMetrics {
    pub success: bool,
    pub tool_errors: usize,
    pub steps: usize,
    pub duration: Duration,
    pub per_tool: BTreeMap<ScenarioStepKind, ToolStats>,
}

/// 框架记录的单个工具调用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvalStep {
    pub kind: ScenarioStepKind,
    pub tool_name: &'static str,
    pub success: bool,
    pub duration: Duration,
    pub error: Option<String>,
    pub output: Option<String>,
}

/// 生成的临时工作区摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceSummary {
    pub root: PathBuf,
    pub file_count: usize,
    pub files: Vec<PathBuf>,
}

/// 离线评估框架的配置。
#[derive(Debug, Clone)]
pub struct EvalHarnessConfig {
    /// 供报告使用的人类可读场景名称。
    pub scenario_name: String,
    /// 如果设置，框架将有意使此步骤失败以测试指标。
    pub fail_step: Option<ScenarioStepKind>,
    /// 在 `exec_shell` 步骤中执行的 shell 命令。
    pub shell_command: String,
    /// 必须出现在 shell 输出中以供验证的 token。
    pub shell_expect_token: String,
    /// 步骤输出摘要存储的最大字符数。
    pub max_output_chars: usize,
    /// 如果设置，每一步都会作为 JSON Lines fixture 追加到此目录下的文件中。
    /// fixture 文件以场景命名（例如 `offline-tool-loop.jsonl`）。
    /// 每行遵循以下模式：`{ "request": <步骤描述符>, "response_events": [<事件>] }`。
    /// mock LLM 客户端（`crate::llm_client::mock`）可以重放这些
    /// fixture 以进行确定性离线测试。有关完整的记录/重放流程，请参见
    /// `crates/tui/tests/README.md`。
    pub record_dir: Option<PathBuf>,
}

impl Default for EvalHarnessConfig {
    fn default() -> Self {
        let shell_command = if cfg!(windows) {
            "echo eval-harness".to_string()
        } else {
            "printf eval-harness".to_string()
        };
        Self {
            scenario_name: "offline-tool-loop".to_string(),
            fail_step: None,
            shell_command,
            shell_expect_token: "eval-harness".to_string(),
            max_output_chars: 240,
            record_dir: None,
        }
    }
}

/// 在临时工作区中执行代表性工具循环的离线框架。
#[derive(Debug, Clone)]
pub struct EvalHarness {
    config: EvalHarnessConfig,
}

impl EvalHarness {
    /// 使用提供的配置创建一个新的框架。
    pub fn new(config: EvalHarnessConfig) -> Self {
        Self { config }
    }

    /// 执行离线评估场景并返回详细结果。
    pub fn run(&self) -> Result<EvalRun> {
        let started_at = Instant::now();
        let workspace = tempfile::Builder::new()
            .prefix("deepseek-eval-")
            .tempdir()
            .context("failed to create evaluation workspace")?;

        let seed = seed_workspace(workspace.path())?;

        let mut steps = Vec::new();
        let mut per_tool: BTreeMap<ScenarioStepKind, ToolStats> = BTreeMap::new();

        let list_output = self.run_step(ScenarioStepKind::List, &mut steps, &mut per_tool, || {
            let entries = list_dir(workspace.path())?;
            Ok(entries.join(", "))
        });

        let _read_output = self.run_step(ScenarioStepKind::Read, &mut steps, &mut per_tool, || {
            let path = if self.config.fail_step == Some(ScenarioStepKind::Read) {
                workspace.path().join("missing.txt")
            } else {
                seed.notes_path.clone()
            };
            read_file(&path)
        });

        let search_output =
            self.run_step(ScenarioStepKind::Search, &mut steps, &mut per_tool, || {
                let root = if self.config.fail_step == Some(ScenarioStepKind::Search) {
                    workspace.path().join("missing-dir")
                } else {
                    workspace.path().to_path_buf()
                };
                let result = search_files(&root, "offline")?;
                Ok(format!("matches={}", result.matches.len()))
            });

        let edit_output = self.run_step(ScenarioStepKind::Edit, &mut steps, &mut per_tool, || {
            let path = if self.config.fail_step == Some(ScenarioStepKind::Edit) {
                workspace.path().join("missing.txt")
            } else {
                seed.notes_path.clone()
            };
            edit_file_append(&path, "edited = true")?;
            Ok("appended line".to_string())
        });

        let patch_output = self.run_step(
            ScenarioStepKind::ApplyPatch,
            &mut steps,
            &mut per_tool,
            || {
                let patch = if self.config.fail_step == Some(ScenarioStepKind::ApplyPatch) {
                    "*** Begin Patch\n*** Update File: notes.txt\n@@\n-THIS LINE DOES NOT EXIST\n+broken\n*** End Patch\n"
                        .to_string()
                } else {
                    "*** Begin Patch\n*** Update File: notes.txt\n@@\n status = \"draft\"\n-todo: offline metrics\n+todo: offline metrics (patched)\n*** End Patch\n"
                        .to_string()
                };
                apply_patch(workspace.path(), &patch)?;
                Ok("patch applied".to_string())
            },
        );

        let shell_output = self.run_step(
            ScenarioStepKind::ExecShell,
            &mut steps,
            &mut per_tool,
            || {
                let command = if self.config.fail_step == Some(ScenarioStepKind::ExecShell) {
                    "command_that_does_not_exist".to_string()
                } else {
                    self.config.shell_command.clone()
                };
                exec_shell(workspace.path(), &command)
            },
        );

        let duration = started_at.elapsed();

        let workspace_summary = summarize_workspace(workspace.path(), list_output.as_deref())?;

        let validation_success = validate_outputs(
            workspace.path(),
            &self.config.shell_expect_token,
            search_output.as_deref(),
            edit_output.as_deref(),
            patch_output.as_deref(),
            shell_output.as_deref(),
        );

        let tool_errors = steps.iter().filter(|s| !s.success).count();
        let success = tool_errors == 0 && validation_success;

        let metrics = EvalMetrics {
            success,
            tool_errors,
            steps: steps.len(),
            duration,
            per_tool,
        };

        Ok(EvalRun {
            scenario_name: self.config.scenario_name.clone(),
            workspace,
            workspace_summary,
            metrics,
            steps,
        })
    }

    fn run_step<T, F>(
        &self,
        kind: ScenarioStepKind,
        steps: &mut Vec<EvalStep>,
        per_tool: &mut BTreeMap<ScenarioStepKind, ToolStats>,
        f: F,
    ) -> Option<T>
    where
        F: FnOnce() -> Result<T>,
        T: ToString,
    {
        let started_at = Instant::now();
        let result = f();
        let duration = started_at.elapsed();

        let stats = per_tool.entry(kind).or_default();
        stats.invocations += 1;
        stats.total_duration += duration;

        match result {
            Ok(value) => {
                let output = truncate_output(&value.to_string(), self.config.max_output_chars);
                steps.push(EvalStep {
                    kind,
                    tool_name: kind.tool_name(),
                    success: true,
                    duration,
                    error: None,
                    output: Some(output.clone()),
                });
                if let Some(dir) = self.config.record_dir.as_deref() {
                    let _ = record_fixture(
                        dir,
                        &self.config.scenario_name,
                        FixtureRecord::ok(kind, &output),
                    );
                }
                Some(value)
            }
            Err(err) => {
                stats.errors += 1;
                let err_str = err.to_string();
                steps.push(EvalStep {
                    kind,
                    tool_name: kind.tool_name(),
                    success: false,
                    duration,
                    error: Some(err_str.clone()),
                    output: None,
                });
                if let Some(dir) = self.config.record_dir.as_deref() {
                    let _ = record_fixture(
                        dir,
                        &self.config.scenario_name,
                        FixtureRecord::err(kind, &err_str),
                    );
                }
                None
            }
        }
    }
}

// === Fixture 记录/重放格式 ===========================================
//
// `--record` 标志每行向 `.jsonl` 文件写入一个 JSON 对象：
//
//     { "request": { "step": "list_dir", "kind": "List" },
//       "response_events": [{ "type": "ok", "output": "…" }] }
//
// mock LLM 客户端通过 `MockLlmClient::push_message_response`（或其流式变体）
// 重放这些 fixture，将每个 `response_events` 数组映射到预制的 `Vec<StreamEvent>`。
//
// 此格式有意保持最小化——可以添加额外字段（时序、模型、用量）
// 而不会破坏旧的 fixture，因为每行都是自包含的 JSON 对象。

/// `--record` JSONL fixture 文件中一行的模式。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureRecord {
    /// 步骤描述符（`{ step, kind }`）。
    pub request: serde_json::Value,
    /// 一个或多个合成的响应事件。
    pub response_events: Vec<serde_json::Value>,
}

impl FixtureRecord {
    fn ok(kind: ScenarioStepKind, output: &str) -> Self {
        Self {
            request: serde_json::json!({
                "step": kind.tool_name(),
                "kind": format!("{kind:?}"),
            }),
            response_events: vec![serde_json::json!({
                "type": "ok",
                "output": output,
            })],
        }
    }

    fn err(kind: ScenarioStepKind, error: &str) -> Self {
        Self {
            request: serde_json::json!({
                "step": kind.tool_name(),
                "kind": format!("{kind:?}"),
            }),
            response_events: vec![serde_json::json!({
                "type": "error",
                "error": error,
            })],
        }
    }
}

/// 将一条 fixture 记录追加到 `<dir>/<scenario>.jsonl`（如果目录或文件不存在则创建）。
/// 最大努力：I/O 错误会返回但通常被框架忽略，以便记录失败不会掩盖运行的主要结果。
pub fn record_fixture(dir: &Path, scenario_name: &str, record: FixtureRecord) -> Result<PathBuf> {
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create fixture dir: {}", dir.display()))?;
    let safe_scenario = scenario_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    let path = dir.join(format!("{safe_scenario}.jsonl"));
    let line = serde_json::to_string(&record).context("failed to serialize fixture record")?;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open fixture file: {}", path.display()))?;
    writeln!(file, "{line}")
        .with_context(|| format!("failed to write fixture line to {}", path.display()))?;
    Ok(path)
}

impl Default for EvalHarness {
    fn default() -> Self {
        Self::new(EvalHarnessConfig::default())
    }
}

/// 运行评估框架的结果。
#[derive(Debug)]
pub struct EvalRun {
    pub scenario_name: String,
    workspace: TempDir,
    pub workspace_summary: WorkspaceSummary,
    pub metrics: EvalMetrics,
    pub steps: Vec<EvalStep>,
}

impl EvalRun {
    /// 获取临时工作区的根目录。
    pub fn workspace_root(&self) -> &Path {
        self.workspace.path()
    }

    /// 将运行结果转换为可序列化报告以用于 CLI 输出。
    pub fn to_report(&self) -> EvalReport {
        EvalReport {
            scenario_name: self.scenario_name.clone(),
            workspace_root: self.workspace_root().to_path_buf(),
            workspace_summary: self.workspace_summary.clone(),
            metrics: self.metrics.clone(),
            steps: self.steps.clone(),
        }
    }
}

/// 从 `EvalRun` 派生的可序列化报告。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EvalReport {
    pub scenario_name: String,
    pub workspace_root: PathBuf,
    pub workspace_summary: WorkspaceSummary,
    pub metrics: EvalMetrics,
    pub steps: Vec<EvalStep>,
}

#[derive(Debug, Clone)]
struct SeedWorkspace {
    notes_path: PathBuf,
}

fn seed_workspace(root: &Path) -> Result<SeedWorkspace> {
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir)
        .with_context(|| format!("failed to create seed directory: {}", src_dir.display()))?;

    let readme_path = root.join("README.md");
    fs::write(
        &readme_path,
        "# Eval Harness Workspace\n\nThis workspace is offline.\n",
    )
    .with_context(|| format!("failed to write {}", readme_path.display()))?;

    let notes_path = root.join("notes.txt");
    fs::write(
        &notes_path,
        "# Eval Harness\nstatus = \"draft\"\ntodo: offline metrics\n",
    )
    .with_context(|| format!("failed to write {}", notes_path.display()))?;

    let lib_path = src_dir.join("lib.rs");
    fs::write(
        &lib_path,
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .with_context(|| format!("failed to write {}", lib_path.display()))?;

    Ok(SeedWorkspace { notes_path })
}

fn summarize_workspace(root: &Path, list_output: Option<&str>) -> Result<WorkspaceSummary> {
    let mut files = Vec::new();

    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .build();

    for entry in walker {
        let entry = entry.with_context(|| format!("failed to walk {}", root.display()))?;
        if entry.file_type().is_some_and(|t| t.is_file()) {
            files.push(entry.into_path());
        }
    }

    if files.is_empty()
        && let Some(output) = list_output
        && !output.trim().is_empty()
    {
        return Err(anyhow!(
            "workspace appears empty after list_dir: {}",
            output.trim()
        ));
    }

    files.sort();

    Ok(WorkspaceSummary {
        root: root.to_path_buf(),
        file_count: files.len(),
        files,
    })
}

fn validate_outputs(
    root: &Path,
    shell_expect_token: &str,
    search_output: Option<&str>,
    edit_output: Option<&str>,
    patch_output: Option<&str>,
    shell_output: Option<&str>,
) -> bool {
    let notes_path = root.join("notes.txt");
    let notes = match fs::read_to_string(&notes_path) {
        Ok(content) => content,
        Err(_) => return false,
    };

    let search_ok = search_output.is_some_and(|s| s.contains("matches="));
    let edit_ok = edit_output.is_some_and(|s| !s.is_empty()) && notes.contains("edited = true");
    let patch_ok = patch_output.is_some_and(|s| !s.is_empty())
        && notes.contains("todo: offline metrics (patched)");
    let shell_ok = shell_output
        .map(str::trim)
        .is_some_and(|s| s.contains(shell_expect_token));

    search_ok && edit_ok && patch_ok && shell_ok
}

fn list_dir(path: &Path) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    let dir = fs::read_dir(path)
        .with_context(|| format!("failed to read directory: {}", path.display()))?;

    for entry in dir {
        let entry = entry.with_context(|| format!("failed to list {}", path.display()))?;
        entries.push(entry.file_name().to_string_lossy().to_string());
    }

    entries.sort();
    Ok(entries)
}

fn read_file(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchMatch {
    path: PathBuf,
    line: usize,
    content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchResult {
    matches: Vec<SearchMatch>,
}

fn search_files(root: &Path, pattern: &str) -> Result<SearchResult> {
    if !root.exists() {
        return Err(anyhow!("search root does not exist: {}", root.display()));
    }

    let regex = Regex::new(pattern).context("failed to compile search regex")?;
    let mut matches = Vec::new();

    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .build();

    for entry in walker {
        let entry = entry.with_context(|| format!("failed to walk {}", root.display()))?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }

        let path = entry.path();
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for (idx, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                matches.push(SearchMatch {
                    path: path.to_path_buf(),
                    line: idx + 1,
                    content: line.to_string(),
                });
            }
            if matches.len() >= 64 {
                break;
            }
        }
        if matches.len() >= 64 {
            break;
        }
    }

    Ok(SearchResult { matches })
}

fn edit_file_append(path: &Path, line: &str) -> Result<()> {
    let mut content = read_file(path)?;
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(line);
    content.push('\n');
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

fn apply_patch(root: &Path, patch: &str) -> Result<()> {
    let mut lines = patch.lines();

    let begin = lines.next().unwrap_or_default();
    if begin != "*** Begin Patch" {
        return Err(anyhow!("patch missing *** Begin Patch header"));
    }

    let header = lines.next().unwrap_or_default();
    let file_rel = header
        .strip_prefix("*** Update File: ")
        .ok_or_else(|| anyhow!("only *** Update File patches are supported"))?;
    if file_rel.contains("..") {
        return Err(anyhow!("patch path must be workspace-relative"));
    }

    let file_path = root.join(file_rel);
    let original = read_file(&file_path)?;
    let had_trailing_newline = original.ends_with('\n');
    let mut file_lines: Vec<String> = original.lines().map(|l| l.to_string()).collect();

    let mut cursor = 0usize;
    for raw_line in lines {
        if raw_line == "*** End Patch" {
            break;
        }
        if raw_line.starts_with("*** ") {
            return Err(anyhow!("unexpected patch directive: {raw_line}"));
        }
        if raw_line.starts_with("@@") {
            continue;
        }

        let (kind, rest) = raw_line.split_at(1);
        let content = rest.to_string();

        match kind {
            " " => {
                let Some(found) = file_lines[cursor..]
                    .iter()
                    .position(|line| line == &content)
                    .map(|offset| cursor + offset)
                else {
                    return Err(anyhow!(
                        "patch context not found in {}: {}",
                        file_path.display(),
                        content
                    ));
                };
                cursor = found + 1;
            }
            "-" => {
                if cursor >= file_lines.len() || file_lines[cursor] != content {
                    return Err(anyhow!(
                        "patch removal mismatch in {}: expected '{}'",
                        file_path.display(),
                        content
                    ));
                }
                file_lines.remove(cursor);
            }
            "+" => {
                file_lines.insert(cursor, content);
                cursor += 1;
            }
            _ => return Err(anyhow!("unsupported patch line: {raw_line}")),
        }
    }

    let mut updated = file_lines.join("\n");
    if had_trailing_newline {
        updated.push('\n');
    }

    fs::write(&file_path, updated)
        .with_context(|| format!("failed to write patched file {}", file_path.display()))
}

fn exec_shell(root: &Path, command: &str) -> Result<String> {
    crate::shell_dispatcher::global_dispatcher().run_foreground(command, root)
}

fn truncate_output(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let truncated: String = value.chars().take(max_chars).collect();
    format!("{truncated}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_shell_invocation_preserves_quoted_payload_as_single_arg() {
        let command = r#"git commit -m "feat: complete sub-pages""#;

        let windows = eval_shell_invocation_for_platform(command, EvalShellPlatform::Windows);
        assert_eq!(windows.program, "cmd");
        assert_eq!(windows.args, vec!["/C".to_string(), command.to_string()]);
        assert!(windows.raw_payload_on_windows);

        let unix = eval_shell_invocation_for_platform(command, EvalShellPlatform::Unix);
        assert_eq!(unix.program, "sh");
        assert_eq!(unix.args, vec!["-c".to_string(), command.to_string()]);
        assert!(!unix.raw_payload_on_windows);
    }
}
