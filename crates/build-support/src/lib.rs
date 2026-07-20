//! `codewhale-cli` 和 `codewhale-tui` 构建脚本的共享构建脚本辅助函数：
//! 重新运行条件声明和嵌入的 `DEEPSEEK_BUILD_VERSION` 元数据。
//! 仅在构建脚本中调用这些函数 — 它们会在 stdout 上发出 `cargo:` 指令。

use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// 声明构建元数据指令的重新运行条件：
/// SHA 覆盖环境变量以及跟踪 `HEAD` 的 git 文件。
///
/// `manifest_dir` 是调用方构建脚本的 `CARGO_MANIFEST_DIR`。
pub fn declare_rerun_conditions(manifest_dir: &Path) {
    println!("cargo:rerun-if-env-changed=DEEPSEEK_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    declare_git_head_rerun(manifest_dir);
}

/// 发出 `cargo:rustc-env=DEEPSEEK_BUILD_VERSION=...` — 包版本，
/// 后附简短的构建 SHA（当可确定时）。
///
/// `manifest_dir` 和 `package_version` 是调用方构建脚本的
/// `CARGO_MANIFEST_DIR` 和 `CARGO_PKG_VERSION`。
pub fn emit_build_version(manifest_dir: &Path, package_version: &str) {
    let build_version = build_sha(manifest_dir)
        .map(|sha| format!("{package_version} ({sha})"))
        .unwrap_or_else(|| package_version.to_string());

    println!("cargo:rustc-env=DEEPSEEK_BUILD_VERSION={build_version}");
}

/// 当 `HEAD` 移动时，告诉 Cargo 使缓存的构建脚本输出失效，
/// 以便嵌入的短 SHA 与检出保持同步。
///
/// `.git/HEAD` 仅在分支切换和分离 HEAD 移动时改变 —
/// 当前分支上的 `git commit` 更新底层引用文件
/// （松散 `refs/heads/<name>`，或 `git pack-refs` 后的 `packed-refs`），
/// 而不会触及 `HEAD` 本身。因此当 `HEAD` 是符号引用时，
/// 我们也监视已解析的目标和 `packed-refs`。不存在的
/// `rerun-if-changed` 路径被 Cargo 视为"始终已更改"，这覆盖了松散→打包的转换。
fn declare_git_head_rerun(manifest_dir: &Path) {
    let workspace_root = manifest_dir.join("..").join("..");
    let git_meta = workspace_root.join(".git");

    let gitdir = if git_meta.is_dir() {
        git_meta
    } else if git_meta.is_file() {
        // 工作树指针文件：直接监视它，然后跟随 `gitdir:`。
        println!("cargo:rerun-if-changed={}", git_meta.display());
        let Ok(contents) = std::fs::read_to_string(&git_meta) else {
            return;
        };
        let Some(rest) = contents.lines().find_map(|l| l.strip_prefix("gitdir:")) else {
            return;
        };
        let trimmed = rest.trim();
        if Path::new(trimmed).is_absolute() {
            PathBuf::from(trimmed)
        } else {
            workspace_root.join(trimmed)
        }
    } else {
        return;
    };

    let head = gitdir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());

    if let Ok(contents) = std::fs::read_to_string(&head)
        && let Some(target) = parse_symbolic_ref(&contents)
    {
        println!("cargo:rerun-if-changed={}", gitdir.join(target).display());
        println!(
            "cargo:rerun-if-changed={}",
            gitdir.join("packed-refs").display()
        );
    }
}

/// 如果 `.git/HEAD` 是符号引用（`ref: refs/heads/...`），则返回目标引用路径。
/// 对于分离 HEAD（原始 SHA）返回 `None`。
fn parse_symbolic_ref(head_contents: &str) -> Option<&str> {
    head_contents
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("ref:"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn build_sha(manifest_dir: &Path) -> Option<String> {
    env_sha("DEEPSEEK_BUILD_SHA")
        .or_else(|| env_sha("GITHUB_SHA"))
        .or_else(|| git_sha(manifest_dir))
}

fn env_sha(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(short_sha)
}

fn git_sha(manifest_dir: &Path) -> Option<String> {
    let top_level_output = Command::new("git")
        .args(["-C"])
        .arg(manifest_dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !top_level_output.status.success() {
        return None;
    }
    let top_level = PathBuf::from(String::from_utf8_lossy(&top_level_output.stdout).trim());
    if !top_level.join("Cargo.toml").is_file() || !top_level.join("crates/tui").is_dir() {
        return None;
    }

    let output = Command::new("git")
        .args(["-C"])
        .arg(top_level)
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    short_sha(String::from_utf8_lossy(&output.stdout).to_string())
}

fn short_sha(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(12).collect())
}

#[cfg(test)]
mod tests {
    use super::parse_symbolic_ref;

    #[test]
    fn symbolic_ref_strips_prefix_and_whitespace() {
        assert_eq!(
            parse_symbolic_ref("ref: refs/heads/main\n"),
            Some("refs/heads/main")
        );
    }

    #[test]
    fn symbolic_ref_handles_no_trailing_newline() {
        assert_eq!(
            parse_symbolic_ref("ref: refs/heads/work/v0.8.26-security"),
            Some("refs/heads/work/v0.8.26-security")
        );
    }

    #[test]
    fn detached_head_is_not_a_symbolic_ref() {
        assert_eq!(
            parse_symbolic_ref("506343f44e48b9c2c8d6b2d3e8e8e8e8e8e8e8e8\n"),
            None
        );
    }

    #[test]
    fn empty_input_returns_none() {
        assert_eq!(parse_symbolic_ref(""), None);
        assert_eq!(parse_symbolic_ref("ref: \n"), None);
    }
}
