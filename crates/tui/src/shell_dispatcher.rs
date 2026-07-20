//! DeepSeek TUI 的 Shell 抽象层。
//!
//! 启动时检测用户的 shell，并为所有命令执行提供单一入口点。
//! DeepSeek TUI 从不直接调用 `Command::new("cmd")`（或
//! `"sh"`、`"pwsh"` 等）——它要求 [`ShellDispatcher`] 构建
//! 一个正确配置的 [`std::process::Command`]。
//!
//! ## 职责
//!
//! 1. **Shell 检测** — 找到用户的实际 shell（PowerShell、pwsh、
//!    通过 WSL / Git Bash 的 bash、Windows 上的 cmd.exe 回退、Unix 上的 /bin/sh）。
//! 2. **引号正确性** — 每个 shell 的参数传递约定都得到尊重，
//!    因此带引号的字符串在 spawn 边界中完整保留。
//! 3. **终端状态** — 前台 shell 执行保存和恢复 crossterm 原始模式，
//!    以便子进程退出后 TUI 输入管道不会中断（问题 #1690）。

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

static LOG_MUTEX: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Shell 种类
// ---------------------------------------------------------------------------

/// 分发器将使用的具体 shell。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellKind {
    /// PowerShell 7+（`pwsh.exe`）。
    Pwsh,
    /// Windows PowerShell 5.1（`powershell.exe`）。
    WindowsPowerShell,
    /// 命令提示符（`cmd.exe`）。
    Cmd,
    /// Unix `/bin/sh`（或 `$SHELL` 检测到的 bash/zsh）。
    Sh,
    /// Bash——在 Unix 上或 Windows 上的 WSL/Git Bash 中通过 `$SHELL` 检测。
    Bash,
    /// 来自 $SHELL 的任何其他 POSIX shell（zsh、fish、dash 等）。
    Custom { binary: String, flag: String },
}

impl ShellKind {
    /// shell 的二进制文件名。在 Windows 上根据需要附加 `.exe`。
    pub fn binary(&self) -> &str {
        match self {
            #[cfg(windows)]
            ShellKind::Pwsh => "pwsh.exe",
            #[cfg(not(windows))]
            ShellKind::Pwsh => "pwsh",

            #[cfg(windows)]
            ShellKind::WindowsPowerShell => "powershell.exe",
            #[cfg(not(windows))]
            ShellKind::WindowsPowerShell => "powershell",

            #[cfg(windows)]
            ShellKind::Cmd => "cmd.exe",
            #[cfg(not(windows))]
            ShellKind::Cmd => "cmd",

            ShellKind::Sh => "sh",
            ShellKind::Bash => "bash",
            ShellKind::Custom { binary, .. } => binary,
        }
    }

    /// 告诉 shell 将以下参数作为命令字符串执行的标志。
    pub fn command_flag(&self) -> &str {
        match self {
            ShellKind::Pwsh | ShellKind::WindowsPowerShell => "-NoProfile",
            ShellKind::Cmd => "/C",
            ShellKind::Sh | ShellKind::Bash => "-c",
            ShellKind::Custom { flag, .. } => flag,
        }
    }

    /// 此 shell 是否需要在配置文件标志后再加一个 `-Command` 标志
    ///（PowerShell 特有）。
    pub fn needs_command_flag(&self) -> bool {
        matches!(self, ShellKind::Pwsh | ShellKind::WindowsPowerShell)
    }

    #[cfg(test)]
    /// 当这是 PowerShell 家族 shell 时返回 true。
    pub fn is_powershell(&self) -> bool {
        matches!(self, ShellKind::Pwsh | ShellKind::WindowsPowerShell)
    }
}

// ---------------------------------------------------------------------------
// 分发器
// ---------------------------------------------------------------------------

/// 中央 shell 抽象。通过 [`ShellDispatcher::detect`] 在启动时创建一次，
/// 然后每当需要生成命令时使用。
#[derive(Debug, Clone)]
pub struct ShellDispatcher {
    kind: ShellKind,
}

#[allow(dead_code)]
impl ShellDispatcher {
    /// 从环境检测用户的 shell。
    ///
    /// ## 检测顺序（Windows）
    ///
    /// 1. `$env:SHELL` — WSL 互操作或 Git Bash 经常设置此变量。
    /// 2. `pwsh.exe` 在 `PATH` 上找到 — PowerShell 7+。
    /// 3. `powershell.exe` 在 `PATH` 上找到 — Windows PowerShell 5.1。
    /// 4. `cmd.exe` — 始终可用，最后手段。
    ///
    /// ## 检测顺序（Unix）
    ///
    /// 1. `$SHELL` — 如果包含 `bash`，使用 `Bash`；否则使用
    ///    通过 `Custom` 的实际二进制路径。
    /// 2. `/bin/sh` 回退。
    pub fn detect() -> Self {
        let kind = Self::detect_shell();
        Self::log_startup(&kind);
        ShellDispatcher { kind }
    }

    /// 当设置了 `SHELL_DISPATCHER_LOG` 时记录 shell 执行行。
    pub fn log_exec(command: &str) {
        if let Ok(path) = std::env::var("SHELL_DISPATCHER_LOG") {
            let _ = Self::append_log_static(&path, command);
        }
    }

    fn log_startup(kind: &ShellKind) {
        let _lock = LOG_MUTEX.lock();
        if let Ok(path) = std::env::var("SHELL_DISPATCHER_LOG") {
            let init_line = format!(
                "--- ShellDispatcher log started pid={} ---\n",
                std::process::id()
            );
            let _ = Self::append_log(&path, &init_line);
            let detect_line = format!("[{}] detect: {kind:?}\n", now_iso());
            let _ = Self::append_log(&path, &detect_line);
        }
    }

    fn append_log(path: &str, line: &str) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(Path::new(path))?;
        file.write_all(line.as_bytes())?;
        file.flush()
    }

    fn append_log_static(path: &str, command: &str) -> std::io::Result<()> {
        // 在锁外部解析种类——`global_dispatcher()` 可能触发
        // `detect()`，它会调用 `log_startup()`，而后者也会获取互斥锁。
        let kind = global_dispatcher().kind();
        let _lock = LOG_MUTEX.lock();
        let line = format!("[{}] exec via {kind:?}: {command}\n", now_iso());
        Self::append_log(path, &line)
    }

    /// 已检测到的 shell 种类。
    pub fn kind(&self) -> &ShellKind {
        &self.kind
    }

    // -- 公共构建器 --------------------------------------------------

    /// 为给定的 shell 命令字符串构建一个 `std::process::Command`。
    pub fn build_command(&self, shell_command: &str) -> Command {
        let mut cmd = Command::new(self.kind.binary());

        if self.kind.needs_command_flag() {
            cmd.arg(self.kind.command_flag());
            cmd.arg("-Command");
            cmd.arg(shell_command);
        } else if matches!(self.kind, ShellKind::Cmd) {
            cmd.arg(self.kind.command_flag());
            #[cfg(windows)]
            {
                cmd.raw_arg(shell_command);
            }
            #[cfg(not(windows))]
            {
                cmd.arg(shell_command);
            }
        } else {
            cmd.arg(self.kind.command_flag());
            cmd.arg(shell_command);
        }

        cmd
    }

    /// 构建程序 + 参数元组。当调用者需要在将参数传递给 `Command` 之前
    /// 检查或修改参数时有用。
    pub fn build_command_parts(&self, shell_command: &str) -> (String, Vec<String>) {
        let program = self.kind.binary().to_string();
        let args = if self.kind.needs_command_flag() {
            vec![
                self.kind.command_flag().to_string(),
                "-Command".to_string(),
                shell_command.to_string(),
            ]
        } else {
            vec![
                self.kind.command_flag().to_string(),
                shell_command.to_string(),
            ]
        };
        (program, args)
    }

    /// 从单独的程序 + 参数构建 `Command`（绕过 shell）。
    /// 当调用者已有解析后的可执行文件和参数向量时使用
    /// ——例如沙箱中的 `ExecEnv`。
    #[cfg(test)]
    pub fn build_direct(&self, program: &str, args: &[String]) -> Command {
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd
    }

    /// 执行前台命令，保存/恢复原始模式。
    ///
    /// 作用域守卫确保即使命令生成失败或提前返回，
    /// 原始模式也会被恢复（审查反馈，问题 #1690）。
    pub fn run_foreground(
        &self,
        shell_command: &str,
        cwd: &std::path::Path,
    ) -> Result<String, anyhow::Error> {
        use anyhow::Context;

        // 记录执行
        {
            let _lock = LOG_MUTEX.lock();
            if let Ok(path) = std::env::var("SHELL_DISPATCHER_LOG") {
                let kind = self.kind();
                let line = format!("[{}] exec via {kind:?}: {shell_command}\n", now_iso());
                let _ = Self::append_log(&path, &line);
            }
        }

        // 禁用原始模式；守卫仅在已启用时才恢复它。
        let raw_mode_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if raw_mode_was_enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }
        struct FgRawModeGuard {
            restore: bool,
        }
        impl Drop for FgRawModeGuard {
            fn drop(&mut self) {
                if self.restore {
                    let _ = crossterm::terminal::enable_raw_mode();
                }
            }
        }
        let _guard = FgRawModeGuard {
            restore: raw_mode_was_enabled,
        };

        let mut cmd = self.build_command(shell_command);
        cmd.current_dir(cwd);

        let output = cmd
            .output()
            .with_context(|| format!("执行 shell 命令失败: {shell_command}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "shell 命令失败 (status={}): {}",
                output.status,
                stderr.trim()
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(stdout)
    }

    // -- 检测 --------------------------------------------------------

    fn detect_shell() -> ShellKind {
        #[cfg(windows)]
        {
            // 1. $env:SHELL — WSL 互操作或 Git Bash 经常设置此变量。
            if let Ok(shell) = std::env::var("SHELL") {
                let lower = shell.to_lowercase();
                if lower.contains("bash") {
                    return ShellKind::Bash;
                }
                if lower.contains("pwsh") {
                    return ShellKind::Pwsh;
                }
                if lower.contains("powershell") {
                    return ShellKind::WindowsPowerShell;
                }
            }

            if Self::find_exe("pwsh.exe") {
                return ShellKind::Pwsh;
            }
            if Self::find_exe("powershell.exe") {
                return ShellKind::WindowsPowerShell;
            }
            ShellKind::Cmd
        }

        #[cfg(not(windows))]
        {
            // 1. $SHELL 环境变量（Unix）
            if let Ok(shell) = std::env::var("SHELL") {
                let lower = shell.to_lowercase();
                if lower.contains("bash") {
                    return ShellKind::Bash;
                }
                if lower.contains("pwsh") {
                    return ShellKind::Pwsh;
                }
                if lower.contains("powershell") {
                    return ShellKind::WindowsPowerShell;
                }
                return ShellKind::Custom {
                    binary: shell,
                    flag: "-c".to_string(),
                };
            }

            ShellKind::Sh
        }
    }

    /// 先检查 PATH，然后回退到已知的安装目录。
    #[cfg(windows)]
    fn find_exe(name: &str) -> bool {
        if Self::binary_on_path(name) {
            return true;
        }
        // 已知安装位置（按偏好排序）。
        let known_dirs: &[&str] = &[
            r"C:\Program Files\PowerShell\7",
            r"C:\Windows\System32\WindowsPowerShell\v1.0",
        ];
        known_dirs
            .iter()
            .any(|dir| std::path::Path::new(dir).join(name).is_file())
    }

    #[cfg(windows)]
    fn binary_on_path(name: &str) -> bool {
        std::env::var_os("PATH")
            .map(|path| {
                std::env::split_paths(&path).any(|dir| {
                    let candidate = dir.join(name);
                    candidate.is_file()
                })
            })
            .unwrap_or(false)
    }
}

// -- 辅助函数 ---------------------------------------------------------------

fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3f")
        .to_string()
}

/// 全局分发器实例，在启动时检测一次。
///
/// 任何需要生成 shell 命令的代码路径都可以使用
/// `global_dispatcher()`，而不是将分发器穿过每个函数签名。
pub fn global_dispatcher() -> &'static ShellDispatcher {
    use std::sync::LazyLock;
    static DISPATCHER: LazyLock<ShellDispatcher> = LazyLock::new(ShellDispatcher::detect);
    &DISPATCHER
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_kind_binary_names() {
        #[cfg(windows)]
        {
            assert_eq!(ShellKind::Pwsh.binary(), "pwsh.exe");
            assert_eq!(ShellKind::WindowsPowerShell.binary(), "powershell.exe");
            assert_eq!(ShellKind::Cmd.binary(), "cmd.exe");
        }
        #[cfg(not(windows))]
        {
            assert_eq!(ShellKind::Pwsh.binary(), "pwsh");
            assert_eq!(ShellKind::WindowsPowerShell.binary(), "powershell");
            assert_eq!(ShellKind::Cmd.binary(), "cmd");
        }
        assert_eq!(ShellKind::Sh.binary(), "sh");
        assert_eq!(ShellKind::Bash.binary(), "bash");
    }

    #[test]
    fn detect_returns_some_shell() {
        let dispatcher = global_dispatcher();
        let _kind = dispatcher.kind();
    }

    #[test]
    fn powershell_build_command_includes_no_profile_and_command_flags() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Pwsh,
        };
        let cmd = dispatcher.build_command("echo hello");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.contains(&"-NoProfile"));
        assert!(args.contains(&"-Command"));
        assert!(args.contains(&"echo hello"));
    }

    #[test]
    fn cmd_build_command_uses_c_flag() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Cmd,
        };
        let cmd = dispatcher.build_command("echo hello");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.contains(&"/C"));
        assert!(args.contains(&"echo hello"));
    }

    #[test]
    fn sh_build_command_uses_dash_c() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Sh,
        };
        let cmd = dispatcher.build_command("echo hello");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.contains(&"-c"));
        assert!(args.contains(&"echo hello"));
    }

    #[cfg(test)]
    #[test]
    fn build_direct_preserves_args() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Cmd,
        };
        let args = vec!["-m".to_string(), "commit message".to_string()];
        let cmd = dispatcher.build_direct("git", &args);
        let cmd_args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(cmd_args, vec!["-m", "commit message"]);
    }

    #[cfg(test)]
    #[test]
    fn powershell_flags_are_correct() {
        assert!(ShellKind::Pwsh.needs_command_flag());
        assert!(ShellKind::WindowsPowerShell.needs_command_flag());
        assert!(!ShellKind::Cmd.needs_command_flag());
        assert!(!ShellKind::Sh.needs_command_flag());
        assert!(!ShellKind::Bash.needs_command_flag());
    }

    #[cfg(test)]
    #[test]
    fn is_powershell_detects_both_variants() {
        assert!(ShellKind::Pwsh.is_powershell());
        assert!(ShellKind::WindowsPowerShell.is_powershell());
        assert!(!ShellKind::Cmd.is_powershell());
        assert!(!ShellKind::Sh.is_powershell());
        assert!(!ShellKind::Bash.is_powershell());
    }

    #[cfg(test)]
    #[test]
    fn build_command_quotes_spaces_for_cmd() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Cmd,
        };
        let cmd = dispatcher.build_command("git commit -m \"msg with spaces\"");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "/C");
        assert!(args[1].contains("msg with spaces"));
        assert!(args[1].starts_with("git "));
    }

    #[cfg(test)]
    #[test]
    fn build_command_quotes_spaces_for_pwsh() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Pwsh,
        };
        let cmd = dispatcher.build_command("git commit -m \"msg with spaces\"");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "-NoProfile");
        assert_eq!(args[1], "-Command");
        assert!(args[2].contains("msg with spaces"));
    }

    #[cfg(test)]
    #[test]
    fn build_direct_handles_empty_args() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Sh,
        };
        let cmd = dispatcher.build_direct("echo", &[]);
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn find_exe_finds_cmd_on_path() {
        // cmd.exe 在 Windows 上始终在 PATH 中。
        assert!(ShellDispatcher::find_exe("cmd.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn find_exe_rejects_nonexistent_binary() {
        assert!(!ShellDispatcher::find_exe("nonexistent_xyz_12345.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn find_exe_falls_back_to_known_dirs() {
        // 验证已知目录回退路径在此系统上实际存在。
        let ps_path = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
        if std::path::Path::new(ps_path).is_file() {
            // 回退目录存在——find_exe 应能找到它。
            assert!(ShellDispatcher::find_exe("powershell.exe"));
        } else {
            eprintln!("跳过: {ps_path} 在此系统上不存在");
        }
    }

    #[test]
    fn custom_shell_uses_provided_binary_and_flag() {
        let kind = ShellKind::Custom {
            binary: "/bin/zsh".to_string(),
            flag: "-c".to_string(),
        };
        assert_eq!(kind.binary(), "/bin/zsh");
        assert_eq!(kind.command_flag(), "-c");
    }
}
