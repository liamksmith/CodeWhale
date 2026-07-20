//! 编辑器头部显示的每个工作区的 git 上下文。
//!
//! TUI 显示一个来自 `git status` 和 `git rev-parse` 的"分支 | 干净/N 个已修改/…"徽章。
//! 为了避免在每次渲染时启动 git，结果被缓存并且仅每 `REFRESH_SECS` 秒刷新一次。
//! 刷新优先使用当前 Tokio 运行时的 spawn-blocking；
//! 测试和非异步调用者回退到同步调用。

use crate::dependencies::{ExternalTool, Git};
use std::path::Path;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

use crate::tui::app::App;

/// 工作区上下文徽章允许重新查询 git 的频率（秒）。
/// 对测试公开，以便它们可以测试 TTL。
pub(crate) const REFRESH_SECS: u64 = 15;

/// 如果缓存值早于 [`REFRESH_SECS`] 且 `allow_refresh` 为 true，
/// 则从磁盘拉取新的工作区上下文。始终
/// 首先将任何待处理的异步结果排入 `app.workspace_context`，
/// 以便渲染通道看到最新的值（#399 S1）。
pub(super) fn refresh_if_needed(app: &mut App, now: Instant, allow_refresh: bool) {
    // 首先将异步 cell 结果排入实时字段，以便渲染
    // 路径始终读取最新的值（#399 S1）。
    if let Ok(mut cell) = app.workspace_context_cell.lock()
        && let Some(ctx) = cell.take()
    {
        if app.workspace_context.as_deref() != Some(ctx.as_str()) {
            app.needs_redraw = true;
        }
        app.workspace_context = Some(ctx);
    }

    if app
        .workspace_context_refreshed_at
        .is_some_and(|refreshed_at| {
            now.duration_since(refreshed_at) < Duration::from_secs(REFRESH_SECS)
        })
    {
        return;
    }

    if !allow_refresh {
        return;
    }

    // 当 Tokio 运行时可用时，将 git 查询卸载到后台线程。
    // 对于测试和其他非异步上下文，回退到同步执行（#399 S1）。
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let ctx = app.workspace_context_cell.clone();
        let workspace = app.workspace.clone();
        handle.spawn_blocking(move || {
            let result = collect(&workspace);
            if let Ok(mut guard) = ctx.lock() {
                *guard = result;
            }
        });
    } else {
        // 无运行时——同步运行，以便测试和一次性调用者
        // 仍然立即获得结果。
        app.workspace_context = collect(&app.workspace);
    }
    app.workspace_context_refreshed_at = Some(now);
}

/// 强制在下一个渲染 tick 上重新查询工作区上下文，绕过正常的 TTL。
/// 在后台 git 查询运行时保持当前值可见。
pub(super) fn refresh_now(app: &mut App, now: Instant) {
    if let Ok(mut cell) = app.workspace_context_cell.lock() {
        *cell = None;
    }
    app.workspace_context_refreshed_at = None;
    refresh_if_needed(app, now, true);
}

#[derive(Debug, Default, Clone, Copy)]
struct ChangeSummary {
    staged: usize,
    modified: usize,
    untracked: usize,
    conflicts: usize,
}

impl ChangeSummary {
    fn is_clean(&self) -> bool {
        self.staged == 0 && self.modified == 0 && self.untracked == 0 && self.conflicts == 0
    }
}

/// 从 `git rev-parse` + `git status` 构建人类可读的工作区上下文字符串
///（"分支 | 状态"）。如果工作区不是 git 仓库或
/// git 本身不可用，则返回 `None`。
pub(crate) fn collect(workspace: &Path) -> Option<String> {
    let branch = branch(workspace)?;
    let summary = change_summary(workspace)?;

    let mut parts = Vec::new();
    if summary.staged > 0 {
        parts.push(format!("{} staged", summary.staged));
    }
    if summary.modified > 0 {
        parts.push(format!("{} modified", summary.modified));
    }
    if summary.untracked > 0 {
        parts.push(format!("{} untracked", summary.untracked));
    }
    if summary.conflicts > 0 {
        parts.push(format!("{} conflicts", summary.conflicts));
    }

    let status = if summary.is_clean() {
        "clean".to_string()
    } else {
        parts.join(", ")
    };

    Some(format!("{branch} | {status}"))
}

pub(crate) fn branch_from_context(context: &str) -> Option<&str> {
    let (branch, _) = context.rsplit_once(" | ")?;
    (!branch.is_empty()).then_some(branch)
}

/// 用于底部状态芯片的简洁、事实性的工作区标识（#3188）。
///
/// 标识仅来自工作区/git 检测——绝不来自模型叙述或配置文本。
/// `name` 是工作区基本名称，`branch` 仅在工作区是 git 仓库时
/// 为 `Some`（对分离 HEAD 携带 "detached:<hash>" 形式），
/// `is_git` 区分真实仓库和普通目录，以便底部可以显示
/// 明确的非仓库状态而不是空的 `Repo:` 标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceIdentity {
    pub name: String,
    pub branch: Option<String>,
    pub is_git: bool,
}

/// 用作工作区标识的基本名称。当路径没有最终组件
///（文件系统根目录）时，回退到稳定的哨兵值。
/// 完全从工作区路径派生，因此它永远不会在渲染路径上启动 git。
pub(crate) fn workspace_basename(workspace: &Path) -> String {
    workspace
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("(root)")
        .to_string()
}

/// 从工作区路径加上缓存的 "分支 | 状态" 上下文字符串解析底部标识。
/// 当工作区不是 git 仓库（或 git 不可用）时 `context` 为 `None`，
/// 我们将其显示为明确的非仓库状态而不是隐藏芯片。
pub(crate) fn identity_from_context(workspace: &Path, context: Option<&str>) -> WorkspaceIdentity {
    let branch = context.and_then(branch_from_context).map(str::to_string);
    WorkspaceIdentity {
        name: workspace_basename(workspace),
        is_git: branch.is_some(),
        branch,
    }
}

/// 渲染底部仓库标签，在宽度受限时保留最有用的标识
///（#3188 验收标准）。布局优先级，从宽到窄：
///
/// 1. `Repo: <名称> @ <分支>`（git 仓库，两者都有空间）
/// 2. `Repo: <名称>`（在截断名称之前丢弃分支）
/// 3. `Repo: <截断名称…>` 然后在真正很小时仅保留裸标签
///
/// 非 git 工作区渲染 `Repo: <名称> (no git)`，在宽度压力下退化为
/// `Repo: <名称>` 然后截断。仅在 `max_width` 无法容纳
/// 即使是 `Repo:` 前缀时返回空字符串。
pub(crate) fn format_repo_identity(identity: &WorkspaceIdentity, max_width: usize) -> String {
    use crate::localization::truncate_to_width;

    const PREFIX: &str = "Repo: ";
    let prefix_width = PREFIX.width();
    if max_width < prefix_width {
        return String::new();
    }

    // 从最丰富到最精简的候选项；第一个适合的获胜。
    let mut candidates: Vec<String> = Vec::new();
    match (&identity.branch, identity.is_git) {
        (Some(branch), _) => {
            candidates.push(format!("{PREFIX}{} @ {branch}", identity.name));
            candidates.push(format!("{PREFIX}{}", identity.name));
        }
        (None, _) => {
            candidates.push(format!("{PREFIX}{} (no git)", identity.name));
            candidates.push(format!("{PREFIX}{}", identity.name));
        }
    }

    for candidate in &candidates {
        if candidate.width() <= max_width {
            return candidate.clone();
        }
    }

    // 即使精简形式也溢出：保留前缀 + 截断的名称，以便
    // 标识永远不会崩溃为裸露的、无用的 `Repo:` 标签。
    truncate_to_width(&format!("{PREFIX}{}", identity.name), max_width)
}

pub(super) fn branch(workspace: &Path) -> Option<String> {
    let branch = run_git(workspace, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    let branch = branch.trim().to_string();
    if branch == "HEAD" || branch.is_empty() {
        let short_hash = run_git(workspace, &["rev-parse", "--short", "HEAD"]).ok()?;
        let short_hash = short_hash.trim();
        if short_hash.is_empty() {
            return None;
        }
        return Some(format!("detached:{short_hash}"));
    }
    Some(branch)
}

fn change_summary(workspace: &Path) -> Option<ChangeSummary> {
    let status = run_git(
        workspace,
        &["status", "--short", "--untracked-files=normal"],
    )
    .ok()?;

    if status.trim().is_empty() {
        return Some(ChangeSummary::default());
    }

    let mut summary = ChangeSummary::default();
    for line in status.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let mut chars = line.chars();
        let staged = chars.next()?;
        let modified = chars.next().unwrap_or(' ');

        if staged == ' ' && modified == ' ' {
            continue;
        }
        if staged == '?' && modified == '?' {
            summary.untracked = summary.untracked.saturating_add(1);
            continue;
        }

        if staged == 'U' || modified == 'U' {
            summary.conflicts = summary.conflicts.saturating_add(1);
        }
        if staged != ' ' && staged != '?' {
            summary.staged = summary.staged.saturating_add(1);
        }
        if modified != ' ' && modified != '?' {
            summary.modified = summary.modified.saturating_add(1);
        }
    }

    Some(summary)
}

fn run_git(workspace: &Path, args: &[&str]) -> std::io::Result<String> {
    let output = Git::output(args, workspace)?;
    if !output.status.success() {
        return Err(std::io::Error::other("git command failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn identity_in_git_repo_carries_name_and_branch() {
        let id = identity_from_context(
            &PathBuf::from("/work/CodeWhale"),
            Some("codex/v0.8.61 | 3 modified"),
        );
        assert_eq!(id.name, "CodeWhale");
        assert_eq!(id.branch.as_deref(), Some("codex/v0.8.61"));
        assert!(id.is_git);
        // 全宽渲染保留仓库标识和分支两者。
        assert_eq!(
            format_repo_identity(&id, 80),
            "Repo: CodeWhale @ codex/v0.8.61"
        );
    }

    #[test]
    fn identity_outside_git_uses_cwd_basename_with_explicit_state() {
        // `None` 上下文 == 不是 git 仓库 / git 不可用。我们不得显示
        // 过时的仓库，但也不得崩溃为空白的 `Repo:`。
        let id = identity_from_context(&PathBuf::from("/tmp/scratch-dir"), None);
        assert_eq!(id.name, "scratch-dir");
        assert_eq!(id.branch, None);
        assert!(!id.is_git);
        assert_eq!(format_repo_identity(&id, 80), "Repo: scratch-dir (no git)");
    }

    #[test]
    fn detached_head_branch_passes_through_to_label() {
        // `branch()` 将分离 HEAD 编码为 "detached:<hash>"；底部
        // 必须逐字显示该值而不是丢弃标识。
        let id = identity_from_context(
            &PathBuf::from("/work/CodeWhale"),
            Some("detached:ae101a1 | clean"),
        );
        assert_eq!(id.branch.as_deref(), Some("detached:ae101a1"));
        assert_eq!(
            format_repo_identity(&id, 80),
            "Repo: CodeWhale @ detached:ae101a1"
        );
    }

    #[test]
    fn narrow_width_keeps_identity_over_branch_then_truncates() {
        let id = identity_from_context(
            &PathBuf::from("/work/CodeWhale"),
            Some("codex/v0.8.61 | clean"),
        );

        // 对于 "name @ branch" 太窄 -> 丢弃分支，保留名称。
        let dropped = format_repo_identity(&id, 20);
        assert_eq!(dropped, "Repo: CodeWhale");
        assert!(dropped.width() <= 20);

        // 即使名称也太窄 -> 截断但保留前缀，以便
        // 芯片永远不会变成裸露的、无用的 "Repo:" 标签。
        let truncated = format_repo_identity(&id, 11);
        assert!(truncated.width() <= 11, "{truncated:?} must fit width 11");
        assert!(truncated.starts_with("Repo: "), "{truncated:?}");
        assert!(truncated.ends_with('…'), "{truncated:?}");

        // 低于裸 "Repo:" 前缀 -> 不渲染任何内容，以便底部
        // 干净地隐藏芯片而不是打印垃圾。
        assert_eq!(format_repo_identity(&id, 3), "");
    }

    #[test]
    fn non_git_identity_degrades_before_truncating() {
        let id = identity_from_context(&PathBuf::from("/tmp/scratch-dir"), None);
        // 没有 "(no git)" 后缀的空间 -> 回退到仅名称。
        assert_eq!(format_repo_identity(&id, 18), "Repo: scratch-dir");
    }

    #[test]
    fn workspace_basename_handles_root_path() {
        assert_eq!(workspace_basename(Path::new("/")), "(root)");
        assert_eq!(workspace_basename(Path::new("/a/b/project")), "project");
    }

    #[test]
    fn collect_and_identity_agree_on_a_real_repo() {
        // 真实 git 集成测试：在实际工作树中，`collect()` 产生
        // "分支 | 状态" 字符串，`identity_from_context` 必须从中读回
        // git 标识。当 git 不可用时跳过
        //（镜像 dependencies::external_tool_output_respects_cwd）。
        if !Git::available() {
            return;
        }
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        // `git init` 使目录成为带有 HEAD 的真实仓库。
        let init = Git::output(&["init", "-q"], root);
        if init.is_err() || !init.unwrap().status.success() {
            return; // 没有可写 git 配置的隔离 CI：跳过。
        }
        let _ = Git::output(&["config", "user.email", "t@example.com"], root);
        let _ = Git::output(&["config", "user.name", "Test"], root);

        match collect(root) {
            Some(ctx) => {
                let id = identity_from_context(root, Some(ctx.as_str()));
                assert!(id.is_git, "fresh repo should detect a git identity");
                assert!(id.branch.is_some(), "repo must report a branch/HEAD");
                let label = format_repo_identity(&id, 80);
                assert!(label.starts_with("Repo: "), "{label:?}");
            }
            None => {
                // 一些沙箱在空仓库上报告没有分支；
                // 非 git 回退必须仍然产生可用的标签。
                let id = identity_from_context(root, None);
                assert!(!id.is_git);
                assert!(format_repo_identity(&id, 80).starts_with("Repo: "));
            }
        }
    }
}
