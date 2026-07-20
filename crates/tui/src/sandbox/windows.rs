//! Windows 沙箱辅助契约。
//!
//! 当前状态：CodeWhale 未提供进程内 Windows 沙箱。未来的 Windows 支持必须通过专用辅助程序运行命令，
//! 该辅助程序使用 Job Object 和 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 提供进程树包含。
//!
//! 第一个 Windows 辅助切片仅为进程包含。在只读文件系统隔离、工作区写入强制、网络阻止、
//! 注册表隔离或 AppContainer 级隔离各自实现并测试之前，不得声称具备这些能力。

use std::path::Path;

use super::SandboxPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsSandboxKind {
    ProcessContainment,
}

impl std::fmt::Display for WindowsSandboxKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WindowsSandboxKind::ProcessContainment => write!(f, "process-containment"),
        }
    }
}

pub fn is_available() -> bool {
    false
}

pub fn select_best_kind(_policy: &SandboxPolicy, _cwd: &Path) -> WindowsSandboxKind {
    WindowsSandboxKind::ProcessContainment
}

pub fn detect_denial(exit_code: i32, stderr: &str) -> bool {
    if exit_code == 0 {
        return false;
    }

    let patterns = [
        "Access is denied",
        "access denied",
        "STATUS_ACCESS_DENIED",
        "privilege",
        "AppContainer",
        "sandbox",
    ];

    patterns.iter().any(|p| stderr.contains(p))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_sandbox_is_not_advertised_until_helper_exists() {
        assert!(!is_available());
        assert_eq!(
            select_best_kind(&SandboxPolicy::default(), Path::new(".")),
            WindowsSandboxKind::ProcessContainment
        );
    }
}
