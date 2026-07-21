//! 增强型 shell 执行模块，支持后台进程管理和沙箱隔离。
//!
//! 提供以下功能：
//! - 带超时的同步命令执行
//! - 后台进程执行
//! - 进程输出获取
//! - 进程终止
//! - 沙箱支持（macOS Seatbelt）
//! - 流式输出（计划中）

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;
use wait_timeout::ChildExt;

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
#[cfg(windows)]
use windows::core::PCWSTR;

#[cfg(not(target_env = "ohos"))]
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

mod output;

use super::shell_output::{summarize_output, truncate_with_meta};
use crate::child_env;
use crate::sandbox::{
    CommandSpec,
    ExecEnv,
    SandboxManager,
    SandboxPolicy as ExecutionSandboxPolicy, // 重命名以避免与 spec::SandboxPolicy 冲突
    SandboxType,
};
use crate::worker_profile::ShellPolicy;
use output::{tail_from_buffer, take_delta_from_buffer};

/// Shell 进程的状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShellStatus {
    Running,
    Completed,
    Failed,
    Killed,
    TimedOut,
}

/// Shell 命令执行的结果
#[derive(Debug, Clone, Serialize, Deserialize)]

pub struct ShellResult {
    pub task_id: Option<String>,
    pub status: ShellStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    /// 原始标准输出的字节长度
    #[serde(default)]
    pub stdout_len: usize,
    /// 原始标准错误的字节长度
    #[serde(default)]
    pub stderr_len: usize,
    /// 标准输出因截断而省略的字节数
    #[serde(default)]
    pub stdout_omitted: usize,
    /// 标准错误因截断而省略的字节数
    #[serde(default)]
    pub stderr_omitted: usize,
    /// 标准输出是否被截断
    #[serde(default)]
    pub stdout_truncated: bool,
    /// 标准错误是否被截断
    #[serde(default)]
    pub stderr_truncated: bool,
    /// 命令是否在沙箱中执行
    #[serde(default)]
    pub sandboxed: bool,
    /// 使用的沙箱类型（如有）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_type: Option<String>,
    /// 命令是否被沙箱限制阻止
    #[serde(default)]
    pub sandbox_denied: bool,
}

/// 紧凑的、面向 UI 的后台 shell 作业快照视图
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellJobSnapshot {
    pub id: String,
    pub job_id: String,
    pub command: String,
    pub cwd: PathBuf,
    pub status: ShellStatus,
    pub exit_code: Option<i32>,
    pub elapsed_ms: u64,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub stdout_len: usize,
    pub stderr_len: usize,
    pub stdin_available: bool,
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_since_output_ms: Option<u64>,
    pub linked_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_name: Option<String>,
}

/// 追踪的后台 shell 作业的一次性完成事件
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellCompletionEvent {
    pub task_id: String,
    pub command: String,
    pub status: ShellStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub linked_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_name: Option<String>,
}

/// 后台 shell 作业的可选所属者归属
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellJobOwner {
    pub agent_id: String,
    pub agent_name: String,
}

/// `/jobs show <id>` 使用的完整输出视图
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellJobDetail {
    pub snapshot: ShellJobSnapshot,
    pub stdout: String,
    pub stderr: String,
}

pub struct ShellDeltaResult {
    pub command: String,
    pub result: ShellResult,
    pub stdout_total_len: usize,
    pub stderr_total_len: usize,
}

enum ShellChild {
    Process(Child),
    #[cfg(not(target_env = "ohos"))]
    Pty(Box<dyn portable_pty::Child + Send>),
}

#[cfg(unix)]
fn kill_child_process_group(child: &mut Child) -> std::io::Result<()> {
    let pgid = child.id() as libc::pid_t;
    if pgid <= 0 {
        return child.kill();
    }

    let result = unsafe { libc::kill(-pgid, libc::SIGKILL) };
    if result == 0 {
        Ok(())
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            child.kill()
        }
    }
}

/// 配置父进程死亡信号，以便在 TUI 异常退出时回收 shell 启动的子进程（#421）。
/// 在 Linux 上，通过 `pre_exec` 安装 `PR_SET_PDEATHSIG(SIGTERM)` —— 内核会在父进程
/// 退出时立即向子进程发送 SIGTERM，即使 TUI 被 SIGKILL 也是如此。取消路径已经对整个
/// 进程组发起了 SIGKILL，因此此机制仅在父进程未运行 drop/cleanup 代码就死亡时触发
/// （如关闭时 panic、OOM、硬件崩溃等）。
///
/// 在 macOS/Windows 上没有等价的 kernel 机制。现有的优雅关闭路径（取消令牌中的
/// `kill_child_process_group`）仍能处理正常关闭；异常退出可能导致子进程泄漏 ——
/// 根据原始 issue 的验收标准，作为后续 watchdog 项跟踪。
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
fn install_parent_death_signal(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `pre_exec` 在子进程的 fork 和 exec 之间运行。该闭包仅使用栈分配的
    // 常量参数调用 `libc::prctl`，不涉及堆内存或父进程的锁。两个要求
    //（异步信号安全 + 无 post-fork 分配）都满足。
    unsafe {
        cmd.pre_exec(|| {
            let result = libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
            if result == -1 {
                // 透传 errno 但不中止 spawn —— 子进程只会失去父进程死亡清理的安全网
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

/// 将 `args` 附加到 `std::process::Command`，在 Windows 上保留 shell 引号语义。
///
/// Issue #1691：在 Windows 上，shell 命令被调用为
/// `cmd /C "chcp 65001 >NUL & <command>"`。Rust 的 `Command::arg` 使用
/// MSVCRT（`CommandLineToArgvW`）转义，会将带引号参数（如 `git commit -m "feat: complete sub-pages"`）中的内嵌 `"` 转义为
/// `\"`。但 `cmd.exe` 不使用 MSVCRT 解析 —— 它将 `\` 视为字面量，
/// 将 `"` 视为原始引号切换 —— 因此转义后的载荷被错误分词，
/// `git` 收到 `feat:`、`complete`、`sub-pages"` 作为独立 pathspec
///（表现为报告的 `pathspec 'sub-pages"' did not match` 症状）。通过
/// `CommandExt::raw_arg` 传递 `cmd /C` 载荷可抑制 std 的转义，
/// 使字符串原样到达 `cmd.exe`，与终端行为完全一致。
#[cfg(windows)]
fn push_shell_args(cmd: &mut Command, program: &str, args: &[String]) {
    use std::os::windows::process::CommandExt;
    // `cmd /C <payload>` 是 std 按参数转义会破坏引号命令的唯一场景。
    // 以原始方式传递 `/C` 和载荷以保留引号；其他程序保持正常（正确）转义。通过
    // 文件主名匹配 `cmd`，使得完整路径（`C:\Windows\System32\cmd.exe`）或 `.exe`
    // 后缀仍能触发原始参数路径。
    let is_cmd = std::path::Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("cmd"))
        .unwrap_or(false);
    if is_cmd && args.len() == 2 && args[0].eq_ignore_ascii_case("/C") {
        cmd.raw_arg(&args[0]);
        cmd.raw_arg(&args[1]);
    } else {
        cmd.args(args);
    }
}

#[cfg(not(windows))]
fn push_shell_args(cmd: &mut Command, _program: &str, args: &[String]) {
    // Unix 将分词完全委托给 `sh -c <command>`；命令字符串
    // 作为单个 argv 条目传递，不会由我们拆分。
    cmd.args(args);
}

#[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
fn install_parent_death_signal(_cmd: &mut Command) {
    // macOS / Windows 上没有内核级别等效机制。协作式取消 + 进程组 SIGKILL 路径
    // 覆盖正常关闭；异常退出（无 unwind 的 panic、TUI 被 SIGKILL）在这些平台上
    // 仍可能导致子进程泄漏 —— 作为后续项跟踪。
}

#[cfg(windows)]
#[derive(Debug)]
struct WindowsJob {
    handle: HANDLE,
}

#[cfg(windows)]
// SAFETY: Windows 作业句柄是进程范围内的内核句柄。在线程间移动
// 包装器不会使句柄失效，访问由 ShellManager 的互斥锁外部同步。
unsafe impl Send for WindowsJob {}
#[cfg(windows)]
// SAFETY: 该包装器仅暴露关于内核句柄的 terminate/drop 操作；
// 并发使用由 ShellManager 保护。
unsafe impl Sync for WindowsJob {}

#[cfg(windows)]
impl WindowsJob {
    fn attach_to_child(child: &Child) -> std::io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()).map_err(windows_io_error)? };
        let job = Self { handle };

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .map_err(windows_io_error)?;

            let process_handle = HANDLE(child.as_raw_handle());
            AssignProcessToJobObject(job.handle, process_handle).map_err(windows_io_error)?;
        }

        Ok(job)
    }

    fn terminate(&self) -> std::io::Result<()> {
        unsafe { TerminateJobObject(self.handle, 1).map_err(windows_io_error) }
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
fn windows_io_error(error: windows::core::Error) -> std::io::Error {
    std::io::Error::other(error)
}

#[cfg(windows)]
fn terminate_windows_job(job: Option<&WindowsJob>, child: &mut Child) -> std::io::Result<()> {
    if let Some(job) = job {
        match job.terminate() {
            Ok(()) => return Ok(()),
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "failed to terminate Windows job object; falling back to immediate child kill"
                );
            }
        }
    }
    child.kill()
}

#[cfg(windows)]
fn terminate_and_close_windows_job(windows_job: Option<WindowsJob>) {
    if let Some(job) = windows_job.as_ref()
        && let Err(err) = job.terminate()
    {
        tracing::warn!(
            ?err,
            "failed to terminate Windows shell job before closing job handle"
        );
    }
    drop(windows_job);
}

#[cfg(windows)]
fn terminate_child_and_close_windows_job(
    windows_job: Option<WindowsJob>,
    child: &mut Child,
) -> std::io::Result<()> {
    let result = terminate_windows_job(windows_job.as_ref(), child);
    drop(windows_job);
    result
}

#[cfg(windows)]
fn attach_windows_job(child: &Child, command: &str) -> Option<WindowsJob> {
    match WindowsJob::attach_to_child(child) {
        Ok(job) => Some(job),
        Err(error) => {
            tracing::warn!(
                ?error,
                command,
                "failed to attach Windows shell process to job object; descendant cleanup degraded"
            );
            None
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ShellExitStatus {
    code: Option<i32>,
    success: bool,
}

impl ShellExitStatus {
    fn from_std(status: std::process::ExitStatus) -> Self {
        Self {
            code: status.code(),
            success: status.success(),
        }
    }

    #[cfg(not(target_env = "ohos"))]
    fn from_pty(status: portable_pty::ExitStatus) -> Self {
        let code = i32::try_from(status.exit_code()).unwrap_or(i32::MAX);
        Self {
            code: Some(code),
            success: status.success(),
        }
    }
}

impl ShellChild {
    fn try_wait(&mut self) -> std::io::Result<Option<ShellExitStatus>> {
        match self {
            ShellChild::Process(child) => child
                .try_wait()
                .map(|status| status.map(ShellExitStatus::from_std)),
            #[cfg(not(target_env = "ohos"))]
            ShellChild::Pty(child) => child
                .try_wait()
                .map(|status| status.map(ShellExitStatus::from_pty)),
        }
    }

    fn wait(&mut self) -> std::io::Result<ShellExitStatus> {
        match self {
            ShellChild::Process(child) => child.wait().map(ShellExitStatus::from_std),
            #[cfg(not(target_env = "ohos"))]
            ShellChild::Pty(child) => child.wait().map(ShellExitStatus::from_pty),
        }
    }

    #[cfg(not(windows))]
    fn kill(&mut self) -> std::io::Result<()> {
        match self {
            #[cfg(unix)]
            ShellChild::Process(child) => kill_child_process_group(child),
            #[cfg(not(unix))]
            ShellChild::Process(child) => child.kill(),
            #[cfg(not(target_env = "ohos"))]
            ShellChild::Pty(child) => child.kill(),
        }
    }
}

enum StdinWriter {
    Pipe(ChildStdin),
    #[cfg(not(target_env = "ohos"))]
    Pty(Box<dyn Write + Send>),
}

impl StdinWriter {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self {
            StdinWriter::Pipe(stdin) => stdin.write_all(data),
            #[cfg(not(target_env = "ohos"))]
            StdinWriter::Pty(writer) => writer.write_all(data),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            StdinWriter::Pipe(stdin) => stdin.flush(),
            #[cfg(not(target_env = "ohos"))]
            StdinWriter::Pty(writer) => writer.flush(),
        }
    }
}

fn spawn_reader_thread<R: Read + Send + 'static>(
    mut reader: R,
    buffer: Arc<Mutex<Vec<u8>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut guard) = buffer.lock() {
                        guard.extend_from_slice(&chunk[..n]);
                    }
                }
                Err(_) => break,
            }
        }
    })
}

const SYNC_READER_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const STALE_NO_OUTPUT_AFTER: Duration = Duration::from_secs(60);

fn spawn_sync_reader_thread<R: Read + Send + 'static>(
    mut reader: R,
) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        tx.send(buf).ok();
    });
    rx
}

fn recv_sync_reader_output(rx: &std::sync::mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    rx.recv_timeout(SYNC_READER_DRAIN_TIMEOUT)
        .unwrap_or_default()
}

/// 正在追踪的后台 shell 进程
pub struct BackgroundShell {
    pub id: String,
    pub command: String,
    pub working_dir: PathBuf,
    pub status: ShellStatus,
    pub exit_code: Option<i32>,
    pub started_at: Instant,
    last_output_at: Instant,
    last_observed_output_len: usize,
    pub sandbox_type: SandboxType,
    pub linked_task_id: Option<String>,
    pub owner_agent: Option<ShellJobOwner>,
    stdout_buffer: Arc<Mutex<Vec<u8>>>,
    stderr_buffer: Option<Arc<Mutex<Vec<u8>>>>,
    stdout_cursor: usize,
    stderr_cursor: usize,
    completion_reported: bool,
    stdin: Option<StdinWriter>,
    child: Option<ShellChild>,
    #[cfg(windows)]
    windows_job: Option<WindowsJob>,
    stdout_thread: Option<std::thread::JoinHandle<()>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

impl BackgroundShell {
    /// 检查进程是否已完成并更新状态
    fn poll(&mut self) -> bool {
        self.refresh_output_activity();
        if self.status != ShellStatus::Running {
            return true;
        }

        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.exit_code = status.code;
                    self.status = if status.success {
                        ShellStatus::Completed
                    } else {
                        ShellStatus::Failed
                    };
                    self.collect_output();
                    true
                }
                Ok(None) => false, // 仍在运行
                Err(_) => {
                    self.status = ShellStatus::Failed;
                    self.collect_output();
                    true
                }
            }
        } else {
            true
        }
    }

    fn refresh_output_activity(&mut self) {
        let observed_len = self.observed_output_len();
        if observed_len != self.last_observed_output_len {
            self.last_observed_output_len = observed_len;
            self.last_output_at = Instant::now();
        }
    }

    fn observed_output_len(&self) -> usize {
        let stdout_len = self
            .stdout_buffer
            .lock()
            .map(|data| data.len())
            .unwrap_or(0);
        let stderr_len = self
            .stderr_buffer
            .as_ref()
            .and_then(|buffer| buffer.lock().ok().map(|data| data.len()))
            .unwrap_or(0);
        stdout_len.saturating_add(stderr_len)
    }

    /// 从后台线程收集输出
    fn collect_output(&mut self) {
        // 在合并读取线程之前杀死整个进程组。
        // 当 shell 产生了持久化后台作业（如 `nohup curl`）时，
        // 这些子进程会在 shell 退出后保持管道写端打开。
        // 不进行此杀死操作，handle.join() 将无限阻塞，冻结调用
        // list_jobs() → poll() → collect_output() 的 UI 事件循环。
        #[cfg(unix)]
        if let Some(child) = self.child.as_mut() {
            match child {
                ShellChild::Process(proc) => {
                    let _ = kill_child_process_group(proc);
                }
                #[cfg(not(target_env = "ohos"))]
                ShellChild::Pty(_) => {}
            }
        }
        #[cfg(windows)]
        terminate_and_close_windows_job(self.windows_job.take());
        if let Some(handle) = self.stdout_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
        self.stdin = None;
        self.child = None;
    }

    fn write_stdin(&mut self, input: &str, close: bool) -> Result<()> {
        if let Some(stdin) = self.stdin.as_mut() {
            if !input.is_empty() {
                stdin
                    .write_all(input.as_bytes())
                    .context("Failed to write to stdin")?;
                stdin.flush().ok();
            }
            if close {
                self.stdin = None;
            }
            return Ok(());
        }

        if input.is_empty() && close {
            return Ok(());
        }

        Err(anyhow!("stdin is not available for task {}", self.id))
    }

    fn full_output(&self) -> (String, String, usize, usize) {
        let stdout_bytes = self
            .stdout_buffer
            .lock()
            .map(|data| data.clone())
            .unwrap_or_default();
        let stderr_bytes = self
            .stderr_buffer
            .as_ref()
            .and_then(|buffer| buffer.lock().ok().map(|data| data.clone()))
            .unwrap_or_default();

        let stdout_len = stdout_bytes.len();
        let stderr_len = stderr_bytes.len();

        (
            String::from_utf8_lossy(&stdout_bytes).to_string(),
            String::from_utf8_lossy(&stderr_bytes).to_string(),
            stdout_len,
            stderr_len,
        )
    }

    fn take_delta(&mut self) -> (String, String, usize, usize, usize, usize) {
        let (stdout_delta, stdout_total) =
            take_delta_from_buffer(&self.stdout_buffer, &mut self.stdout_cursor);
        let (stderr_delta, stderr_total) = if let Some(buffer) = self.stderr_buffer.as_ref() {
            take_delta_from_buffer(buffer, &mut self.stderr_cursor)
        } else {
            (Vec::new(), 0)
        };

        let stdout_delta_len = stdout_delta.len();
        let stderr_delta_len = stderr_delta.len();

        if stdout_delta_len > 0 || stderr_delta_len > 0 {
            self.last_output_at = Instant::now();
            self.last_observed_output_len = stdout_total.saturating_add(stderr_total);
        }

        (
            String::from_utf8_lossy(&stdout_delta).to_string(),
            String::from_utf8_lossy(&stderr_delta).to_string(),
            stdout_delta_len,
            stderr_delta_len,
            stdout_total,
            stderr_total,
        )
    }

    fn sandbox_denied(&self) -> bool {
        if matches!(self.status, ShellStatus::Running) {
            return false;
        }
        let (_, stderr_full, _, _) = self.full_output();
        SandboxManager::was_denied(
            self.sandbox_type,
            self.exit_code.unwrap_or(-1),
            &stderr_full,
        )
    }

    /// 杀死进程
    fn kill(&mut self) -> Result<()> {
        if let Some(ref mut child) = self.child {
            match child {
                ShellChild::Process(proc) => {
                    #[cfg(windows)]
                    {
                        terminate_windows_job(self.windows_job.as_ref(), proc)
                            .context("Failed to kill process tree")?;
                        let _ = proc.wait();
                    }
                    #[cfg(not(windows))]
                    {
                        proc.kill().context("Failed to kill process")?;
                        let _ = proc.wait();
                    }
                }
                #[cfg(not(target_env = "ohos"))]
                ShellChild::Pty(child) => {
                    child.kill().context("Failed to kill process")?;
                    let _ = child.wait();
                }
            }
        }
        self.status = ShellStatus::Killed;
        self.collect_output();
        Ok(())
    }

    /// 获取当前状态的快照
    #[allow(dead_code)]
    pub fn snapshot(&self) -> ShellResult {
        let sandboxed = !matches!(self.sandbox_type, SandboxType::None);
        let (stdout_full, stderr_full, _, _) = self.full_output();
        let (stdout, stdout_meta) = truncate_with_meta(&stdout_full);
        let (stderr, stderr_meta) = truncate_with_meta(&stderr_full);
        ShellResult {
            task_id: Some(self.id.clone()),
            status: self.status.clone(),
            exit_code: self.exit_code,
            stdout,
            stderr,
            duration_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout_len: stdout_meta.original_len,
            stderr_len: stderr_meta.original_len,
            stdout_omitted: stdout_meta.omitted,
            stderr_omitted: stderr_meta.omitted,
            stdout_truncated: stdout_meta.truncated,
            stderr_truncated: stderr_meta.truncated,
            sandboxed,
            sandbox_type: if sandboxed {
                Some(self.sandbox_type.to_string())
            } else {
                None
            },
            sandbox_denied: self.sandbox_denied(),
        }
    }

    fn job_snapshot(&self) -> ShellJobSnapshot {
        // 使用 tail_from_buffer 而非 full_output，这样我们永远不会为显示目的
        // 克隆整个累积的 stdout/stderr。full_output 的时间复杂度是 O(总写入字节数)，
        // 这会导致在 TUI 事件循环中调用 list_jobs() 时，ShellManager 互斥锁被
        // 持有任意长时间 —— 在长时间自动化运行时冻结输入处理。
        let (stdout_len, stdout_tail) = tail_from_buffer(&self.stdout_buffer, 1200);
        let (stderr_len, stderr_tail) = self
            .stderr_buffer
            .as_ref()
            .map(|buf| tail_from_buffer(buf, 1200))
            .unwrap_or((0, String::new()));
        let elapsed_since_output_ms = (self.status == ShellStatus::Running)
            .then(|| u64::try_from(self.last_output_at.elapsed().as_millis()).unwrap_or(u64::MAX));
        let stale = elapsed_since_output_ms.is_some_and(|elapsed| {
            elapsed >= u64::try_from(STALE_NO_OUTPUT_AFTER.as_millis()).unwrap_or(u64::MAX)
        });
        ShellJobSnapshot {
            id: self.id.clone(),
            job_id: self.id.clone(),
            command: self.command.clone(),
            cwd: self.working_dir.clone(),
            status: self.status.clone(),
            exit_code: self.exit_code,
            elapsed_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout_tail,
            stderr_tail,
            stdout_len,
            stderr_len,
            stdin_available: self.stdin.is_some() && self.status == ShellStatus::Running,
            stale,
            elapsed_since_output_ms,
            linked_task_id: self.linked_task_id.clone(),
            owner_agent_id: self
                .owner_agent
                .as_ref()
                .map(|owner| owner.agent_id.clone()),
            owner_agent_name: self
                .owner_agent
                .as_ref()
                .map(|owner| owner.agent_name.clone()),
        }
    }

    fn completion_event(&self) -> ShellCompletionEvent {
        let snapshot = self.job_snapshot();
        ShellCompletionEvent {
            task_id: snapshot.id,
            command: snapshot.command,
            status: snapshot.status,
            exit_code: snapshot.exit_code,
            duration_ms: snapshot.elapsed_ms,
            stdout_tail: snapshot.stdout_tail,
            stderr_tail: snapshot.stderr_tail,
            linked_task_id: snapshot.linked_task_id,
            owner_agent_id: snapshot.owner_agent_id,
            owner_agent_name: snapshot.owner_agent_name,
        }
    }

    fn job_detail(&self) -> ShellJobDetail {
        let (stdout, stderr, _, _) = self.full_output();
        ShellJobDetail {
            snapshot: self.job_snapshot(),
            stdout,
            stderr,
        }
    }
}

impl Drop for BackgroundShell {
    fn drop(&mut self) {
        if self.status == ShellStatus::Running
            && let Some(ref mut child) = self.child
        {
            #[cfg(windows)]
            match child {
                ShellChild::Process(proc) => {
                    let _ = terminate_windows_job(self.windows_job.as_ref(), proc);
                }
                #[cfg(not(target_env = "ohos"))]
                ShellChild::Pty(child) => {
                    let _ = child.kill();
                }
            }
            #[cfg(not(windows))]
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// 管理后台 shell 进程，支持可选的沙箱功能。
pub struct ShellManager {
    processes: HashMap<String, BackgroundShell>,
    stale_jobs: HashMap<String, ShellJobSnapshot>,
    default_workspace: PathBuf,
    sandbox_manager: SandboxManager,
    sandbox_policy: ExecutionSandboxPolicy,
    foreground_background_requested: bool,
}

impl std::fmt::Debug for ShellManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellManager")
            .field("processes", &self.processes.len())
            .field("stale_jobs", &self.stale_jobs.len())
            .field("default_workspace", &self.default_workspace)
            .field("sandbox_policy", &self.sandbox_policy)
            .field(
                "foreground_background_requested",
                &self.foreground_background_requested,
            )
            .finish()
    }
}

impl ShellManager {
    /// 创建新的 `ShellManager`，使用默认（无沙箱）策略。
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            processes: HashMap::new(),
            stale_jobs: HashMap::new(),
            default_workspace: workspace,
            sandbox_manager: SandboxManager::new(),
            sandbox_policy: ExecutionSandboxPolicy::default(),
            foreground_background_requested: false,
        }
    }

    /// 创建新的 `ShellManager`，使用指定的沙箱策略。
    #[allow(dead_code)]
    pub fn with_sandbox(workspace: PathBuf, policy: ExecutionSandboxPolicy) -> Self {
        Self {
            processes: HashMap::new(),
            stale_jobs: HashMap::new(),
            default_workspace: workspace,
            sandbox_manager: SandboxManager::new(),
            sandbox_policy: policy,
            foreground_background_requested: false,
        }
    }

    /// 设置后续命令使用的沙箱策略。
    #[allow(dead_code)]
    pub fn set_sandbox_policy(&mut self, policy: ExecutionSandboxPolicy) {
        self.sandbox_policy = policy;
    }

    /// 获取当前的沙箱策略。
    #[allow(dead_code)]
    pub fn sandbox_policy(&self) -> &ExecutionSandboxPolicy {
        &self.sandbox_policy
    }

    /// 启用或禁用 bubblewrap 直通模式（#2184）。
    ///
    /// 启用后，如果 Linux 上存在 `/usr/bin/bwrap`，exec_shell
    /// 命令将通过 bubblewrap 进行文件系统隔离。
    #[allow(dead_code)] // 在后续 PR 中从 EngineConfig 接入
    pub fn set_prefer_bwrap(&mut self, prefer: bool) {
        self.sandbox_manager.set_prefer_bwrap(prefer);
    }

    /// 请求将活动的前台 shell wait 分离，并使其进程
    /// 在后台作业表中继续运行。
    pub fn request_foreground_background(&mut self) {
        self.foreground_background_requested = true;
    }

    fn clear_foreground_background_request(&mut self) {
        self.foreground_background_requested = false;
    }

    fn take_foreground_background_request(&mut self) -> bool {
        let requested = self.foreground_background_requested;
        self.foreground_background_requested = false;
        requested
    }

    /// 检查当前平台是否支持沙箱功能。
    #[allow(dead_code)]
    pub fn is_sandbox_available(&mut self) -> bool {
        self.sandbox_manager.is_available()
    }

    #[allow(dead_code)]
    pub fn default_workspace(&self) -> &Path {
        &self.default_workspace
    }

    /// 使用已配置的沙箱策略执行 shell 命令。
    #[allow(dead_code)]
    pub fn execute(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        background: bool,
    ) -> Result<ShellResult> {
        self.execute_with_policy(command, working_dir, timeout_ms, background, None)
    }

    /// 使用指定的沙箱策略执行 shell 命令（覆盖默认策略）。
    #[allow(dead_code)]
    pub fn execute_with_policy(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        background: bool,
        policy_override: Option<ExecutionSandboxPolicy>,
    ) -> Result<ShellResult> {
        self.execute_with_options(
            command,
            working_dir,
            timeout_ms,
            background,
            None,
            false,
            policy_override,
        )
    }

    /// 使用 stdin/TTY 选项执行 shell 命令。
    #[allow(clippy::too_many_arguments)]
    pub fn execute_with_options(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        background: bool,
        stdin_data: Option<&str>,
        tty: bool,
        policy_override: Option<ExecutionSandboxPolicy>,
    ) -> Result<ShellResult> {
        self.execute_with_options_env(
            command,
            working_dir,
            timeout_ms,
            background,
            stdin_data,
            tty,
            policy_override,
            HashMap::new(),
        )
    }

    /// 与 `execute_with_options` 相同，额外增加一个环境变量映射，
    /// 合并到生成的进程环境中。由 `shell_env` 钩子注入路径使用（#456）；
    /// 其他调用者应使用上面更简单的包装器。
    #[allow(clippy::too_many_arguments)]
    pub fn execute_with_options_env(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        background: bool,
        stdin_data: Option<&str>,
        tty: bool,
        policy_override: Option<ExecutionSandboxPolicy>,
        extra_env: HashMap<String, String>,
    ) -> Result<ShellResult> {
        self.execute_with_options_env_for_owner(
            command,
            working_dir,
            timeout_ms,
            background,
            stdin_data,
            tty,
            policy_override,
            extra_env,
            None,
        )
    }

    /// 与 `execute_with_options_env` 相同，增加了可选的子代理启动作业的
    /// 后台作业所有者归属。
    #[allow(clippy::too_many_arguments)]
    pub fn execute_with_options_env_for_owner(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        background: bool,
        stdin_data: Option<&str>,
        tty: bool,
        policy_override: Option<ExecutionSandboxPolicy>,
        extra_env: HashMap<String, String>,
        owner_agent: Option<ShellJobOwner>,
    ) -> Result<ShellResult> {
        // 当设置了 SHELL_DISPATCHER_LOG 时通过 ShellDispatcher 记录执行日志。
        crate::shell_dispatcher::ShellDispatcher::log_exec(command);

        let work_dir = working_dir.map_or_else(|| self.default_workspace.clone(), PathBuf::from);

        // 将超时限制为最大 10 分钟（600000ms）
        let timeout_ms = timeout_ms.clamp(1000, 600_000);

        // 如果提供了覆盖策略则使用之，否则使用管理器的策略
        let policy = policy_override.unwrap_or_else(|| self.sandbox_policy.clone());

        // 创建命令规格并准备沙箱环境
        let spec = CommandSpec::shell(command, work_dir.clone(), Duration::from_millis(timeout_ms))
            .with_policy(policy)
            .with_env(extra_env);
        let exec_env = self.sandbox_manager.prepare(&spec);

        if background {
            self.spawn_background_sandboxed(
                command,
                &work_dir,
                &exec_env,
                stdin_data,
                tty,
                owner_agent,
            )
        } else {
            if tty {
                return Err(anyhow!(
                    "TTY mode requires background execution (set background: true)."
                ));
            }
            Self::execute_sync_sandboxed(command, &work_dir, timeout_ms, stdin_data, &exec_env)
        }
    }

    /// 以交互方式执行 shell 命令（stdin/stdout/stderr 继承自终端）。
    #[allow(dead_code)]
    pub fn execute_interactive(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
    ) -> Result<ShellResult> {
        self.execute_interactive_with_policy(command, working_dir, timeout_ms, None)
    }

    /// 以交互方式执行 shell 命令，使用指定的沙箱策略覆盖。
    pub fn execute_interactive_with_policy(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        policy_override: Option<ExecutionSandboxPolicy>,
    ) -> Result<ShellResult> {
        self.execute_interactive_with_policy_env(
            command,
            working_dir,
            timeout_ms,
            policy_override,
            HashMap::new(),
        )
    }

    /// 接受额外环境变量的交互式变体（#456 shell_env 钩子）。
    pub fn execute_interactive_with_policy_env(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        policy_override: Option<ExecutionSandboxPolicy>,
        extra_env: HashMap<String, String>,
    ) -> Result<ShellResult> {
        crate::shell_dispatcher::ShellDispatcher::log_exec(command);

        let work_dir = working_dir.map_or_else(|| self.default_workspace.clone(), PathBuf::from);

        let timeout_ms = timeout_ms.clamp(1000, 600_000);
        let policy = policy_override.unwrap_or_else(|| self.sandbox_policy.clone());

        let spec = CommandSpec::shell(command, work_dir.clone(), Duration::from_millis(timeout_ms))
            .with_policy(policy)
            .with_env(extra_env);
        let exec_env = self.sandbox_manager.prepare(&spec);

        Self::execute_interactive_sandboxed(command, &work_dir, timeout_ms, &exec_env)
    }

    /// 以同步方式执行命令，带超时（沙箱化）。
    fn execute_sync_sandboxed(
        original_command: &str,
        working_dir: &std::path::Path,
        timeout_ms: u64,
        stdin_data: Option<&str>,
        exec_env: &ExecEnv,
    ) -> Result<ShellResult> {
        let started = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        let sandbox_type = exec_env.sandbox_type;
        let sandboxed = exec_env.is_sandboxed();

        // 从 ExecEnv 构建命令
        let program = exec_env.program();
        let args = exec_env.args();

        let mut cmd = Command::new(program);
        crate::utils::suppress_console_window(&mut cmd);
        push_shell_args(&mut cmd, program, args);
        cmd.current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
        install_parent_death_signal(&mut cmd);

        if stdin_data.is_some() {
            cmd.stdin(Stdio::piped());
        }

        child_env::apply_to_command(&mut cmd, child_env::string_map_env(&exec_env.env));

        // 在 spawn 前禁用 raw mode；仅在进入时 raw mode 已激活时才恢复
        // （issue #1690）。
        let raw_mode_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if raw_mode_was_enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }
        struct SyncRawModeGuard {
            restore: bool,
        }
        impl Drop for SyncRawModeGuard {
            fn drop(&mut self) {
                if self.restore {
                    let _ = crossterm::terminal::enable_raw_mode();
                }
            }
        }
        let _guard = SyncRawModeGuard {
            restore: raw_mode_was_enabled,
        };

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to execute: {original_command}"))?;
        #[cfg(windows)]
        let windows_job = attach_windows_job(&child, original_command);

        if let Some(input) = stdin_data
            && let Some(mut stdin) = child.stdin.take()
        {
            stdin
                .write_all(input.as_bytes())
                .context("Failed to write to stdin")?;
            stdin.flush().ok();
        }

        let stdout_handle = child.stdout.take().context("Failed to capture stdout")?;
        let stderr_handle = child.stderr.take().context("Failed to capture stderr")?;

        // 生成线程来读取输出。使用下面的有界接收通道，这样被杀死的
        // 或分离的后代进程（保持管道句柄打开）无法在持有全局工具锁时
        // 卡住前台 shell 路径（#2571）。
        let stdout_rx = spawn_sync_reader_thread(stdout_handle);
        let stderr_rx = spawn_sync_reader_thread(stderr_handle);

        // 带超时等待
        if let Some(status) = child.wait_timeout(timeout)? {
            #[cfg(unix)]
            let _ = kill_child_process_group(&mut child);
            #[cfg(windows)]
            terminate_and_close_windows_job(windows_job);
            let stdout = recv_sync_reader_output(&stdout_rx);
            let stderr = recv_sync_reader_output(&stderr_rx);
            let stdout_str = String::from_utf8_lossy(&stdout).to_string();
            let stderr_str = String::from_utf8_lossy(&stderr).to_string();
            let exit_code = status.code().unwrap_or(-1);

            // 检查沙箱是否拒绝了该操作
            let sandbox_denied = SandboxManager::was_denied(sandbox_type, exit_code, &stderr_str);
            let (stdout, stdout_meta) = truncate_with_meta(&stdout_str);
            let (stderr, stderr_meta) = truncate_with_meta(&stderr_str);

            Ok(ShellResult {
                task_id: None,
                status: if status.success() {
                    ShellStatus::Completed
                } else {
                    ShellStatus::Failed
                },
                exit_code: status.code(),
                stdout,
                stderr,
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                stdout_len: stdout_meta.original_len,
                stderr_len: stderr_meta.original_len,
                stdout_omitted: stdout_meta.omitted,
                stderr_omitted: stderr_meta.omitted,
                stdout_truncated: stdout_meta.truncated,
                stderr_truncated: stderr_meta.truncated,
                sandboxed,
                sandbox_type: if sandboxed {
                    Some(sandbox_type.to_string())
                } else {
                    None
                },
                sandbox_denied,
            })
        } else {
            // 超时 —— 杀死进程
            #[cfg(unix)]
            let _ = kill_child_process_group(&mut child);
            #[cfg(windows)]
            let _ = terminate_child_and_close_windows_job(windows_job, &mut child);
            #[cfg(all(not(unix), not(windows)))]
            let _ = child.kill();
            let status = child.wait().ok();
            let stdout = recv_sync_reader_output(&stdout_rx);
            let stderr = recv_sync_reader_output(&stderr_rx);
            let stdout_str = String::from_utf8_lossy(&stdout).to_string();
            let stderr_str = String::from_utf8_lossy(&stderr).to_string();
            let (stdout, stdout_meta) = truncate_with_meta(&stdout_str);
            let (stderr, stderr_meta) = truncate_with_meta(&stderr_str);

            Ok(ShellResult {
                task_id: None,
                status: ShellStatus::TimedOut,
                exit_code: status.and_then(|s| s.code()),
                stdout,
                stderr,
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                stdout_len: stdout_meta.original_len,
                stderr_len: stderr_meta.original_len,
                stdout_omitted: stdout_meta.omitted,
                stderr_omitted: stderr_meta.omitted,
                stdout_truncated: stdout_meta.truncated,
                stderr_truncated: stderr_meta.truncated,
                sandboxed,
                sandbox_type: if sandboxed {
                    Some(sandbox_type.to_string())
                } else {
                    None
                },
                sandbox_denied: false,
            })
        }
    }

    /// 以交互方式执行命令，带超时（沙箱化）。
    fn execute_interactive_sandboxed(
        original_command: &str,
        working_dir: &std::path::Path,
        timeout_ms: u64,
        exec_env: &ExecEnv,
    ) -> Result<ShellResult> {
        let started = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        let sandbox_type = exec_env.sandbox_type;
        let sandboxed = exec_env.is_sandboxed();

        let program = exec_env.program();
        let args = exec_env.args();

        let mut cmd = Command::new(program);
        crate::utils::suppress_console_window(&mut cmd);
        push_shell_args(&mut cmd, program, args);
        cmd.current_dir(working_dir)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
        install_parent_death_signal(&mut cmd);

        // 在 spawn 前禁用 raw mode；仅在进入时 raw mode 已激活时才恢复
        // （issue #1690）。
        let raw_mode_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if raw_mode_was_enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }
        struct InteractiveRawModeGuard {
            restore: bool,
        }
        impl Drop for InteractiveRawModeGuard {
            fn drop(&mut self) {
                if self.restore {
                    let _ = crossterm::terminal::enable_raw_mode();
                }
            }
        }
        let _guard = InteractiveRawModeGuard {
            restore: raw_mode_was_enabled,
        };

        child_env::apply_to_command(&mut cmd, child_env::string_map_env(&exec_env.env));

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to execute: {original_command}"))?;
        #[cfg(windows)]
        let windows_job = attach_windows_job(&child, original_command);

        if let Some(status) = child.wait_timeout(timeout)? {
            #[cfg(windows)]
            terminate_and_close_windows_job(windows_job);
            Ok(ShellResult {
                task_id: None,
                status: if status.success() {
                    ShellStatus::Completed
                } else {
                    ShellStatus::Failed
                },
                exit_code: status.code(),
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                stdout_len: 0,
                stderr_len: 0,
                stdout_omitted: 0,
                stderr_omitted: 0,
                stdout_truncated: false,
                stderr_truncated: false,
                sandboxed,
                sandbox_type: if sandboxed {
                    Some(sandbox_type.to_string())
                } else {
                    None
                },
                sandbox_denied: false,
            })
        } else {
            #[cfg(unix)]
            let _ = kill_child_process_group(&mut child);
            #[cfg(windows)]
            let _ = terminate_child_and_close_windows_job(windows_job, &mut child);
            #[cfg(all(not(unix), not(windows)))]
            let _ = child.kill();
            let status = child.wait().ok();

            Ok(ShellResult {
                task_id: None,
                status: ShellStatus::TimedOut,
                exit_code: status.and_then(|s| s.code()),
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                stdout_len: 0,
                stderr_len: 0,
                stdout_omitted: 0,
                stderr_omitted: 0,
                stdout_truncated: false,
                stderr_truncated: false,
                sandboxed,
                sandbox_type: if sandboxed {
                    Some(sandbox_type.to_string())
                } else {
                    None
                },
                sandbox_denied: false,
            })
        }
    }

    /// 生成一个后台进程（沙箱化）。
    fn spawn_background_sandboxed(
        &mut self,
        original_command: &str,
        working_dir: &std::path::Path,
        exec_env: &ExecEnv,
        stdin_data: Option<&str>,
        tty: bool,
        owner_agent: Option<ShellJobOwner>,
    ) -> Result<ShellResult> {
        let task_id = format!("shell_{}", &Uuid::new_v4().to_string()[..8]);
        let started = Instant::now();
        let sandbox_type = exec_env.sandbox_type;
        let sandboxed = exec_env.is_sandboxed();

        // 从 ExecEnv 构建命令
        let program = exec_env.program();
        let args = exec_env.args();

        #[cfg(target_env = "ohos")]
        if tty {
            return Err(anyhow!(
                "TTY shell mode is not supported on HarmonyOS/OpenHarmony yet."
            ));
        }

        let stdout_buffer = Arc::new(Mutex::new(Vec::new()));
        let stderr_buffer = if tty {
            None
        } else {
            Some(Arc::new(Mutex::new(Vec::new())))
        };

        #[cfg(windows)]
        let mut windows_job = None;

        let (child, stdin, stdout_thread, stderr_thread) = if tty {
            #[cfg(target_env = "ohos")]
            unreachable!("OHOS TTY mode returns before PTY setup");

            #[cfg(not(target_env = "ohos"))]
            {
                let pty_system = native_pty_system();
                let pair = pty_system
                    .openpty(PtySize {
                        rows: 24,
                        cols: 80,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .context("Failed to open PTY")?;

                let mut cmd = CommandBuilder::new(program);
                for arg in args {
                    cmd.arg(arg);
                }
                cmd.cwd(working_dir);
                child_env::apply_to_pty_command(&mut cmd, child_env::string_map_env(&exec_env.env));

                let child = pair
                    .slave
                    .spawn_command(cmd)
                    .with_context(|| format!("Failed to spawn PTY command: {original_command}"))?;
                drop(pair.slave);

                let reader = pair
                    .master
                    .try_clone_reader()
                    .context("Failed to clone PTY reader")?;
                let stdout_thread = Some(spawn_reader_thread(reader, Arc::clone(&stdout_buffer)));
                let writer = pair
                    .master
                    .take_writer()
                    .context("Failed to take PTY writer")?;

                (
                    ShellChild::Pty(child),
                    Some(StdinWriter::Pty(writer)),
                    stdout_thread,
                    None,
                )
            }
        } else {
            let mut cmd = Command::new(program);
            crate::utils::suppress_console_window(&mut cmd);
            push_shell_args(&mut cmd, program, args);
            cmd.current_dir(working_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(unix)]
            {
                cmd.process_group(0);
            }

            child_env::apply_to_command(&mut cmd, child_env::string_map_env(&exec_env.env));

            let mut child = cmd
                .spawn()
                .with_context(|| format!("Failed to spawn background: {original_command}"))?;
            #[cfg(windows)]
            {
                windows_job = attach_windows_job(&child, original_command);
            }

            let stdout_handle = child.stdout.take().context("Failed to capture stdout")?;
            let stderr_handle = child.stderr.take().context("Failed to capture stderr")?;
            let stdin_handle = child.stdin.take().map(StdinWriter::Pipe);

            let stdout_thread = Some(spawn_reader_thread(
                stdout_handle,
                Arc::clone(&stdout_buffer),
            ));
            let stderr_thread = stderr_buffer
                .as_ref()
                .map(|buffer| spawn_reader_thread(stderr_handle, Arc::clone(buffer)));

            (
                ShellChild::Process(child),
                stdin_handle,
                stdout_thread,
                stderr_thread,
            )
        };

        let mut bg_shell = BackgroundShell {
            id: task_id.clone(),
            command: original_command.to_string(),
            working_dir: working_dir.to_path_buf(),
            status: ShellStatus::Running,
            exit_code: None,
            started_at: started,
            last_output_at: started,
            last_observed_output_len: 0,
            sandbox_type,
            linked_task_id: None,
            owner_agent,
            stdout_buffer,
            stderr_buffer,
            stdout_cursor: 0,
            stderr_cursor: 0,
            completion_reported: false,
            stdin,
            child: Some(child),
            #[cfg(windows)]
            windows_job,
            stdout_thread,
            stderr_thread,
        };

        if let Some(input) = stdin_data {
            bg_shell.write_stdin(input, false)?;
        }

        self.processes.insert(task_id.clone(), bg_shell);

        Ok(ShellResult {
            task_id: Some(task_id),
            status: ShellStatus::Running,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            stdout_len: 0,
            stderr_len: 0,
            stdout_omitted: 0,
            stderr_omitted: 0,
            stdout_truncated: false,
            stderr_truncated: false,
            sandboxed,
            sandbox_type: if sandboxed {
                Some(sandbox_type.to_string())
            } else {
                None
            },
            sandbox_denied: false,
        })
    }

    /// 获取后台进程的输出
    #[allow(dead_code)]
    pub fn get_output(
        &mut self,
        task_id: &str,
        block: bool,
        timeout_ms: u64,
    ) -> Result<ShellResult> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;

        if block && shell.status == ShellStatus::Running {
            let timeout = Duration::from_millis(timeout_ms.clamp(1000, 600_000));
            let deadline = Instant::now() + timeout;

            while shell.status == ShellStatus::Running && Instant::now() < deadline {
                if shell.poll() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }

            // 如果超时后仍在运行
            if shell.status == ShellStatus::Running {
                return Ok(shell.snapshot());
            }
        } else {
            shell.poll();
        }

        Ok(shell.snapshot())
    }

    /// 向后台进程的 stdin 写入数据。
    pub fn write_stdin(&mut self, task_id: &str, input: &str, close: bool) -> Result<()> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;
        shell.write_stdin(input, close)?;
        Ok(())
    }

    /// 获取后台进程的增量输出，消耗所有新输出。
    fn get_output_delta(
        &mut self,
        task_id: &str,
        wait: bool,
        timeout_ms: u64,
    ) -> Result<ShellDeltaResult> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;

        if wait && shell.status == ShellStatus::Running {
            let timeout = Duration::from_millis(timeout_ms.clamp(1000, 600_000));
            let deadline = Instant::now() + timeout;

            while shell.status == ShellStatus::Running && Instant::now() < deadline {
                if shell.poll() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        } else {
            shell.poll();
        }

        let (
            stdout_delta,
            stderr_delta,
            stdout_delta_len,
            stderr_delta_len,
            stdout_total,
            stderr_total,
        ) = shell.take_delta();
        let (stdout, stdout_meta) = truncate_with_meta(&stdout_delta);
        let (stderr, stderr_meta) = truncate_with_meta(&stderr_delta);
        let sandboxed = !matches!(shell.sandbox_type, SandboxType::None);

        let command = shell.command.clone();
        let result = ShellResult {
            task_id: Some(shell.id.clone()),
            status: shell.status.clone(),
            exit_code: shell.exit_code,
            stdout,
            stderr,
            duration_ms: u64::try_from(shell.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout_len: stdout_meta.original_len.max(stdout_delta_len),
            stderr_len: stderr_meta.original_len.max(stderr_delta_len),
            stdout_omitted: stdout_meta.omitted,
            stderr_omitted: stderr_meta.omitted,
            stdout_truncated: stdout_meta.truncated,
            stderr_truncated: stderr_meta.truncated,
            sandboxed,
            sandbox_type: if sandboxed {
                Some(shell.sandbox_type.to_string())
            } else {
                None
            },
            sandbox_denied: shell.sandbox_denied(),
        };

        Ok(ShellDeltaResult {
            command,
            result,
            stdout_total_len: stdout_total,
            stderr_total_len: stderr_total,
        })
    }

    /// 杀死一个正在运行的后台进程
    pub fn kill(&mut self, task_id: &str) -> Result<ShellResult> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;

        shell.kill()?;
        Ok(shell.snapshot())
    }

    /// 杀死所有当前正在运行的后台 shell 进程。
    pub fn kill_running(&mut self) -> Result<Vec<ShellResult>> {
        let ids = self
            .processes
            .iter()
            .filter(|(_, shell)| shell.status == ShellStatus::Running)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();

        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            results.push(self.kill(&id)?);
        }
        Ok(results)
    }

    /// 轮询后台进程并返回增量输出。
    pub fn poll_delta(
        &mut self,
        task_id: &str,
        wait: bool,
        timeout_ms: u64,
    ) -> Result<ShellDeltaResult> {
        self.get_output_delta(task_id, wait, timeout_ms)
    }

    /// 将持久化任务上下文附加到活动的 shell 作业。
    pub fn tag_linked_task(&mut self, task_id: &str, linked_task_id: Option<String>) -> Result<()> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;
        shell.linked_task_id = linked_task_id;
        Ok(())
    }

    /// 检查活动或过期作业的完整输出。
    pub fn inspect_job(&mut self, task_id: &str) -> Result<ShellJobDetail> {
        if let Some(shell) = self.processes.get_mut(task_id) {
            shell.poll();
            return Ok(shell.job_detail());
        }
        if let Some(snapshot) = self.stale_jobs.get(task_id) {
            return Ok(ShellJobDetail {
                snapshot: snapshot.clone(),
                stdout: snapshot.stdout_tail.clone(),
                stderr: snapshot.stderr_tail.clone(),
            });
        }
        Err(anyhow!("Task {task_id} not found"))
    }

    /// 列出 TUI 中所有活动及已知过期的后台 shell 作业。
    pub fn list_jobs(&mut self) -> Vec<ShellJobSnapshot> {
        for shell in self.processes.values_mut() {
            shell.poll();
        }
        // 回收完成时间超过 1 小时的进程以限制内存增长。
        self.cleanup(Duration::from_secs(3600));

        let mut jobs = self
            .processes
            .values()
            .map(BackgroundShell::job_snapshot)
            .collect::<Vec<_>>();
        jobs.extend(self.stale_jobs.values().cloned());
        jobs.sort_by(|a, b| {
            job_status_rank(&a.status, a.stale)
                .cmp(&job_status_rank(&b.status, b.stale))
                .then_with(|| a.id.cmp(&b.id))
        });
        jobs
    }

    /// 排出尚未报告给运行时状态的已完成后台 shell 作业。
    pub fn drain_finished_jobs(&mut self) -> Vec<ShellCompletionEvent> {
        let mut events = Vec::new();
        for shell in self.processes.values_mut() {
            shell.poll();
            if shell.status != ShellStatus::Running && !shell.completion_reported {
                shell.completion_reported = true;
                events.push(shell.completion_event());
            }
        }
        events.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        events
    }

    /// 记住重启后过期的作业，以便 UI 可以显示它而不是隐藏它。
    #[allow(dead_code)]
    pub fn remember_stale_job(
        &mut self,
        id: impl Into<String>,
        command: impl Into<String>,
        cwd: PathBuf,
        linked_task_id: Option<String>,
    ) {
        let id = id.into();
        self.stale_jobs.insert(
            id.clone(),
            ShellJobSnapshot {
                id: id.clone(),
                job_id: id,
                command: command.into(),
                cwd,
                status: ShellStatus::Killed,
                exit_code: None,
                elapsed_ms: 0,
                stdout_tail: String::new(),
                stderr_tail: "Process is no longer attached to this TUI session.".to_string(),
                stdout_len: 0,
                stderr_len: 0,
                stdin_available: false,
                stale: true,
                elapsed_since_output_ms: None,
                linked_task_id,
                owner_agent_id: None,
                owner_agent_name: None,
            },
        );
    }

    /// 清理完成时间超过指定时长的进程
    pub fn cleanup(&mut self, max_age: Duration) {
        let _now = Instant::now();
        self.processes.retain(|_, shell| {
            if shell.status == ShellStatus::Running {
                true
            } else {
                shell.started_at.elapsed() < max_age
            }
        });
    }
}

fn job_status_rank(status: &ShellStatus, stale: bool) -> u8 {
    if stale {
        return 4;
    }
    match status {
        ShellStatus::Running => 0,
        ShellStatus::Failed | ShellStatus::TimedOut => 1,
        ShellStatus::Killed => 2,
        ShellStatus::Completed => 3,
    }
}

/// `ShellManager` 的线程安全包装器
pub type SharedShellManager = Arc<Mutex<ShellManager>>;

/// 创建一个新的共享 shell 管理器，使用默认沙箱策略。
pub fn new_shared_shell_manager(workspace: PathBuf) -> SharedShellManager {
    Arc::new(Mutex::new(ShellManager::new(workspace)))
}

// === ToolSpec 实现 ===

use crate::command_safety::{
    SafetyLevel, analyze_command, extract_primary_command, is_parallel_readonly_command,
};
use crate::execpolicy::{ExecPolicyDecision, load_default_policy};
use crate::features::Feature;
use crate::tools::cargo_failure_summary::summarize_cargo_failure;
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_u64, required_str,
};
use async_trait::async_trait;
use serde_json::json;

const FOREGROUND_TIMEOUT_RECOVERY_HINT: &str = "Foreground exec_shell is for bounded commands. \
The timed-out process was killed; rerun long work with task_shell_start or exec_shell with \
background: true, then poll with task_shell_wait or exec_shell_wait.";

const MACOS_PROVENANCE_HINT: &str = "Docker buildx failed to update its activity file due to a macOS \
com.apple.provenance restriction. Files created by Docker Desktop's signed process carry a \
kernel-enforced provenance tag that blocks writes from child processes (including the TUI \
shell sandbox). Workarounds: (1) run the Docker build from a regular terminal outside the \
TUI, or (2) disable BuildKit with DOCKER_BUILDKIT=0 (only works if your Dockerfiles do not \
use RUN --mount directives).";

/// shell 结果的人类可读退出状态：当进程返回数字代码时显示该代码，
/// 否则显示 "terminated by signal"（而不是向用户泄漏 `Some(127)` / `None` 的 Debug 输出）。
fn exit_code_label(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("exit code {code}"),
        None => "terminated by signal".to_string(),
    }
}
const PYTHON_BUILD_DEPENDENCY_HINT: &str = "Python build dependency missing: setuptools is not \
available in the active environment. Install the declared build requirements first, for example \
`python -m pip install -U pip setuptools wheel build`, then rerun the build command.";

fn attach_cargo_failure_summary(
    metadata: &mut serde_json::Value,
    command: &str,
    result: &ShellResult,
) {
    if let Some(summary) =
        summarize_cargo_failure(command, &result.stdout, &result.stderr, result.exit_code)
    {
        metadata["cargo_failure_summary"] = summary.to_metadata_value();
    }
}

fn attach_python_build_dependency_hint(
    metadata: &mut serde_json::Value,
    hint: Option<&'static str>,
) {
    if let Some(hint) = hint {
        metadata["python_build_dependency_hint"] = json!({
            "kind": "missing_setuptools",
            "hint": hint,
            "recommended_first_step": "python -m pip install -U pip setuptools wheel build",
        });
    }
}

pub(crate) fn looks_like_macos_provenance_failure(result: &ShellResult) -> bool {
    if matches!(result.status, ShellStatus::Completed) && result.exit_code == Some(0) {
        return false;
    }
    let combined = format!("{}\n{}", result.stdout, result.stderr).to_ascii_lowercase();
    combined.contains("com.apple.provenance")
        || combined.contains("update builder last activity")
        || (combined.contains("buildx/activity") && combined.contains("operation not permitted"))
}

fn macos_provenance_hint(result: &ShellResult) -> Option<&'static str> {
    if looks_like_macos_provenance_failure(result) {
        Some(MACOS_PROVENANCE_HINT)
    } else {
        None
    }
}

fn python_build_dependency_hint(command: &str, result: &ShellResult) -> Option<&'static str> {
    if matches!(result.status, ShellStatus::Completed) && result.exit_code == Some(0) {
        return None;
    }

    let command = command.to_ascii_lowercase();
    let combined = format!("{}\n{}", result.stdout, result.stderr).to_ascii_lowercase();
    let mentions_missing_setuptools = [
        "no module named 'setuptools'",
        "no module named \"setuptools\"",
        "setuptools is not available",
        "cannot import 'setuptools",
        "cannot import \"setuptools",
        "missing dependencies",
    ]
    .iter()
    .any(|needle| combined.contains(needle))
        && combined.contains("setuptools");
    if !mentions_missing_setuptools {
        return None;
    }

    let pythonish_command = [
        "python",
        "pip",
        "pytest",
        "tox",
        "nox",
        "cython",
        "setup.py",
        "build_ext",
    ]
    .iter()
    .any(|needle| command.contains(needle));
    let pythonish_output = [
        "setup.py",
        "pyproject.toml",
        "build_meta",
        "build_ext",
        "pep 517",
        "cython",
    ]
    .iter()
    .any(|needle| combined.contains(needle));

    if pythonish_command || pythonish_output {
        Some(PYTHON_BUILD_DEPENDENCY_HINT)
    } else {
        None
    }
}

fn command_likely_needs_network(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    let Some(primary) = extract_primary_command(&normalized) else {
        return false;
    };
    let primary = primary.rsplit(['/', '\\']).next().unwrap_or(primary);

    match primary {
        "curl" | "wget" | "fetch" | "nc" | "netcat" | "ncat" | "ssh" | "scp" | "sftp" | "rsync"
        | "ftp" | "ping" | "traceroute" | "nslookup" | "dig" | "host" | "nmap" | "gh" | "hub" => {
            true
        }
        "git" => [
            " fetch",
            " pull",
            " clone",
            " ls-remote",
            " submodule",
            " push",
        ]
        .iter()
        .any(|needle| normalized.contains(needle)),
        "cargo" => [" install", " fetch", " update", " publish", " search"]
            .iter()
            .any(|needle| normalized.contains(needle)),
        "npm" | "pnpm" | "yarn" => [" install", " i", " add", " update", " publish"]
            .iter()
            .any(|needle| normalized.contains(needle)),
        "pip" | "pip3" | "uv" | "poetry" => [" install", " add", " sync", " update"]
            .iter()
            .any(|needle| normalized.contains(needle)),
        "brew" | "apt" | "apt-get" | "yum" | "dnf" | "pacman" => true,
        "go" => [" get", " install", " mod download"]
            .iter()
            .any(|needle| normalized.contains(needle)),
        _ => false,
    }
}

fn looks_like_network_blocked_failure(result: &ShellResult) -> bool {
    if matches!(result.status, ShellStatus::Completed | ShellStatus::Running)
        || result.exit_code == Some(0)
    {
        return false;
    }

    if result.stdout.trim() == "000" {
        return true;
    }
    if result.sandboxed && result.stdout.is_empty() && result.stderr.is_empty() {
        return true;
    }

    let output = format!("{}\n{}", result.stdout, result.stderr).to_ascii_lowercase();
    [
        "operation not permitted",
        "network is unreachable",
        "could not resolve host",
        "couldn't resolve host",
        "failed to resolve",
        "temporary failure in name resolution",
        "name or service not known",
        "nodename nor servname provided",
        "no address associated",
        "failed to connect",
        "couldn't connect",
        "connection timed out",
        "connection reset",
    ]
    .iter()
    .any(|pattern| output.contains(pattern))
}

fn shell_network_restricted_hint<'a>(
    context: &'a ToolContext,
    command: &str,
    result: &ShellResult,
) -> Option<&'a str> {
    let hint = context.shell_network_denied_hint.as_deref()?;
    let policy_blocks_network = context
        .elevated_sandbox_policy
        .as_ref()
        .is_some_and(|policy| !policy.has_network_access());
    if !policy_blocks_network || !command_likely_needs_network(command) {
        return None;
    }
    if result.sandbox_denied || looks_like_network_blocked_failure(result) {
        Some(hint)
    } else {
        None
    }
}

fn shell_job_owner_from_context(context: &ToolContext) -> Option<ShellJobOwner> {
    let agent_id = context
        .owner_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let agent_name = context
        .owner_agent_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(agent_id);
    Some(ShellJobOwner {
        agent_id: agent_id.to_string(),
        agent_name: agent_name.to_string(),
    })
}

fn attach_shell_owner_metadata(metadata: &mut serde_json::Value, context: &ToolContext) {
    let Some(owner) = shell_job_owner_from_context(context) else {
        return;
    };
    metadata["owner_agent_id"] = json!(owner.agent_id);
    metadata["owner_agent_name"] = json!(owner.agent_name);
}

fn exec_shell_input_is_parallel_readonly(input: &serde_json::Value) -> bool {
    let Some(command) = input.get("command").and_then(serde_json::Value::as_str) else {
        return false;
    };
    if ["background", "interactive", "tty", "combined_output"]
        .iter()
        .any(|key| input.get(*key).and_then(serde_json::Value::as_bool) == Some(true))
    {
        return false;
    }
    if ["stdin", "input", "data"]
        .iter()
        .any(|key| input.get(*key).is_some())
    {
        return false;
    }

    is_parallel_readonly_command(command)
}

fn exec_shell_input_starts_detached(input: &serde_json::Value) -> bool {
    input
        .get("command")
        .and_then(serde_json::Value::as_str)
        .is_some()
        && input
            .get("interactive")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        && (input.get("background").and_then(serde_json::Value::as_bool) == Some(true)
            || input.get("tty").and_then(serde_json::Value::as_bool) == Some(true))
}

async fn execute_foreground_via_background(
    context: &ToolContext,
    command: &str,
    timeout_ms: u64,
    stdin_data: Option<&str>,
    tty: bool,
    policy_override: Option<ExecutionSandboxPolicy>,
    extra_env: HashMap<String, String>,
) -> Result<ShellResult> {
    let timeout_ms = timeout_ms.clamp(1000, 600_000);
    let spawned = {
        let mut manager = context
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("shell manager lock poisoned"))?;
        manager.clear_foreground_background_request();
        manager.execute_with_options_env(
            command,
            None,
            timeout_ms,
            true,
            stdin_data,
            tty,
            policy_override,
            extra_env,
        )?
    };
    let task_id = spawned
        .task_id
        .ok_or_else(|| anyhow!("foreground shell did not return a process id"))?;

    if stdin_data.is_some() {
        let mut manager = context
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("shell manager lock poisoned"))?;
        manager.write_stdin(&task_id, "", true)?;
    }

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if context
            .cancel_token
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| anyhow!("shell manager lock poisoned"))?;
            return manager.kill(&task_id);
        }

        let snapshot = {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| anyhow!("shell manager lock poisoned"))?;
            if manager.take_foreground_background_request() {
                return manager.get_output(&task_id, false, 0);
            }
            manager.get_output(&task_id, false, 0)?
        };

        if snapshot.status != ShellStatus::Running {
            return Ok(snapshot);
        }

        if Instant::now() >= deadline {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| anyhow!("shell manager lock poisoned"))?;
            let mut result = manager.kill(&task_id)?;
            result.status = ShellStatus::TimedOut;
            return Ok(result);
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 用于执行 shell 命令的工具。
pub struct ExecShellTool;

#[async_trait]
impl ToolSpec for ExecShellTool {
    fn name(&self) -> &'static str {
        "exec_shell"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command in the workspace directory. Foreground mode is for bounded commands; use background=true or task_shell_start for work expected to take >5 seconds. Background jobs return immediately and report completion through task/status state instead of resuming the model."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 120000, max: 600000)"
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background and return task_id (default: false). Returns immediately; completion is tracked in task/status state. Prefer this for commands expected to take >5 seconds, including builds, test suites, servers, CI polling, sleep, or other long-running work. Use exec_shell_wait only when you need early output, final output, or a true dependency barrier."
                },
                "interactive": {
                    "type": "boolean",
                    "description": "Run interactively with terminal IO (default: false)"
                },
                "stdin": {
                    "type": "string",
                    "description": "Optional stdin data to send before waiting (non-interactive only)"
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional working directory for the command"
                },
                "tty": {
                    "type": "boolean",
                    "description": "Allocate a pseudo-terminal for interactive programs (implies background)"
                },
                "combined_output": {
                    "type": "boolean",
                    "description": "Capture stdout and stderr as one chronological PTY stream (default false). In foreground mode, waits for completion; in background mode, implies tty."
                }
            },
            "required": ["command"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::ExecutesCode,
            ToolCapability::Sandboxable,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    fn approval_requirement_for(&self, input: &serde_json::Value) -> ApprovalRequirement {
        if exec_shell_input_is_parallel_readonly(input) {
            ApprovalRequirement::Auto
        } else {
            self.approval_requirement()
        }
    }

    fn is_read_only_for(&self, input: &serde_json::Value) -> bool {
        exec_shell_input_is_parallel_readonly(input)
    }

    fn supports_parallel_for(&self, input: &serde_json::Value) -> bool {
        exec_shell_input_is_parallel_readonly(input)
    }

    fn starts_detached_for(&self, input: &serde_json::Value) -> bool {
        exec_shell_input_starts_detached(input)
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let command = required_str(&input, "command")?;
        match context.shell_policy {
            ShellPolicy::None => {
                return Ok(ToolResult::error(
                    "Shell tools are disabled by the active permission profile.",
                ));
            }
            ShellPolicy::ReadOnly if !exec_shell_input_is_parallel_readonly(&input) => {
                return Ok(ToolResult::error(
                    "Shell command blocked by read-only shell policy. Use a non-mutating, non-background inspection command, or switch to Act mode (`/mode act`) for write-capable shell work.",
                ));
            }
            ShellPolicy::ReadOnly | ShellPolicy::Full => {}
        }
        let timeout_ms = optional_u64(&input, "timeout_ms", 120_000).min(600_000);
        let background = optional_bool(&input, "background", false);
        let interactive = optional_bool(&input, "interactive", false);
        let combined_output = optional_bool(&input, "combined_output", false);
        let tty = optional_bool(&input, "tty", false) || (combined_output && background);
        let stdin_data = input
            .get("stdin")
            .or_else(|| input.get("input"))
            .or_else(|| input.get("data"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);

        if interactive && background {
            return Ok(ToolResult::error(
                "Interactive commands cannot run in background mode.",
            ));
        }
        if interactive && (tty || combined_output) {
            return Ok(ToolResult::error(
                "Interactive mode cannot be combined with TTY or combined_output sessions.",
            ));
        }
        if interactive && stdin_data.is_some() {
            return Ok(ToolResult::error(
                "Interactive mode cannot be combined with stdin data.",
            ));
        }

        let background = background || tty;

        let mut execpolicy_decision: Option<ExecPolicyDecision> = None;
        if context.features.enabled(Feature::ExecPolicy)
            && let Some(policy) = load_default_policy()
                .map_err(|e| ToolError::execution_failed(format!("execpolicy load failed: {e}")))?
        {
            let decision = policy.evaluate(command);
            execpolicy_decision = Some(decision.clone());
            if let ExecPolicyDecision::Deny(reason) = decision {
                return Ok(ToolResult {
                    content: format!("BLOCKED: {reason}"),
                    success: false,
                    metadata: Some(json!({
                        "execpolicy": {
                            "decision": "deny",
                            "reason": reason,
                        }
                    })),
                });
            }
        }

        // 安全检查（始终为元数据运行，但仅在不处于 YOLO 模式时阻止执行）
        let safety = analyze_command(command);
        if !context.auto_approve {
            match safety.level {
                SafetyLevel::Dangerous => {
                    let reasons = safety.reasons.join("; ");
                    let suggestions = if safety.suggestions.is_empty() {
                        String::new()
                    } else {
                        format!("\nSuggestions: {}", safety.suggestions.join("; "))
                    };
                    return Ok(ToolResult {
                        content: format!(
                            "BLOCKED: This command was blocked for safety reasons.\n\nReasons: {reasons}{suggestions}\n\nNote: allow_shell=true exposes shell tools, but it does not disable built-in shell safety validation."
                        ),
                        success: false,
                        metadata: Some(json!({
                            "safety_level": "dangerous",
                            "blocked": true,
                            "reasons": safety.reasons,
                            "suggestions": safety.suggestions,
                        })),
                    });
                }
                SafetyLevel::RequiresApproval | SafetyLevel::Safe | SafetyLevel::WorkspaceSafe => {
                    // 正常继续
                }
            }
        }

        let policy_override = context.elevated_sandbox_policy.clone();
        let working_dir = match input
            .get("cwd")
            .or_else(|| input.get("working_dir"))
            .and_then(serde_json::Value::as_str)
        {
            Some(dir) => {
                // 验证 cwd 是否在工作区边界内（与文件工具相同）
                let resolved = context.resolve_path(dir)?;
                Some(resolved.to_string_lossy().to_string())
            }
            None => None,
        };

        // #456 — 从任何已配置的 `shell_env` 钩子收集环境变量。同步运行，
        // 捕获 stdout，解析 `KEY=VAL` 行，审计日志记录键名（绝不记录值）。
        // 未配置钩子时为空操作。
        let extra_env = if let Some(hook_executor) = &context.runtime.hook_executor {
            let hook_ctx = crate::hooks::HookContext::new()
                .with_tool_name("exec_shell")
                .with_tool_args(&input);
            hook_executor.collect_shell_env(&hook_ctx)
        } else {
            std::collections::HashMap::new()
        };

        // 当配置了外部沙箱后端时通过其路由。
        if let Some(backend) = &context.sandbox_backend {
            if interactive {
                return Ok(ToolResult::error(
                    "Interactive mode is not supported with external sandbox backends.",
                ));
            }
            if background {
                return Ok(ToolResult::error(
                    "Background mode is not supported with external sandbox backends.",
                ));
            }
            if tty {
                return Ok(ToolResult::error(
                    "TTY mode is not supported with external sandbox backends.",
                ));
            }

            let started = std::time::Instant::now();
            let backend_result = backend.exec(command, &extra_env).await;

            let result = match backend_result {
                Ok(output) => {
                    let (stdout, stdout_meta) = truncate_with_meta(&output.stdout);
                    let (stderr, stderr_meta) = truncate_with_meta(&output.stderr);
                    ShellResult {
                        task_id: None,
                        status: if output.exit_code == 0 {
                            ShellStatus::Completed
                        } else {
                            ShellStatus::Failed
                        },
                        exit_code: Some(output.exit_code),
                        stdout,
                        stderr,
                        duration_ms: u64::try_from(started.elapsed().as_millis())
                            .unwrap_or(u64::MAX),
                        stdout_len: stdout_meta.original_len,
                        stderr_len: stderr_meta.original_len,
                        stdout_omitted: stdout_meta.omitted,
                        stderr_omitted: stderr_meta.omitted,
                        stdout_truncated: stdout_meta.truncated,
                        stderr_truncated: stderr_meta.truncated,
                        sandboxed: true,
                        sandbox_type: Some("opensandbox".to_string()),
                        sandbox_denied: false,
                    }
                }
                Err(e) => {
                    return Ok(ToolResult::error(format!("Sandbox backend error: {e}")));
                }
            };

            // 构建结果（复用下面的现有输出渲染）。
            let stdout_summary = summarize_output(&result.stdout);
            let stderr_summary = summarize_output(&result.stderr);
            let summary = if !stderr_summary.is_empty() {
                stderr_summary.clone()
            } else {
                stdout_summary.clone()
            };
            let python_dependency_hint = python_build_dependency_hint(command, &result);
            let mut output = if result.stdout.is_empty() && result.stderr.is_empty() {
                "(no output)".to_string()
            } else if result.stderr.is_empty() {
                result.stdout.clone()
            } else {
                format!("{}\n\nSTDERR:\n{}", result.stdout, result.stderr)
            };
            if let Some(hint) = python_dependency_hint {
                output = format!("{hint}\n\n{output}");
            }

            let mut metadata = json!({
                "exit_code": result.exit_code,
                "status": format!("{:?}", result.status),
                "duration_ms": result.duration_ms,
                "sandboxed": true,
                "sandbox_type": "opensandbox",
                "sandbox_denied": false,
                "task_id": result.task_id,
                "stdout_len": result.stdout_len,
                "stderr_len": result.stderr_len,
                "stdout_truncated": result.stdout_truncated,
                "stderr_truncated": result.stderr_truncated,
                "stdout_omitted": result.stdout_omitted,
                "stderr_omitted": result.stderr_omitted,
                "summary": summary,
                "stdout_summary": stdout_summary,
                "stderr_summary": stderr_summary,
                "safety_level": format!("{:?}", safety.level),
                "interactive": false,
                "canceled": false,
                "sandbox_backend": "opensandbox",
            });
            attach_shell_owner_metadata(&mut metadata, context);
            attach_cargo_failure_summary(&mut metadata, command, &result);
            attach_python_build_dependency_hint(&mut metadata, python_dependency_hint);

            return Ok(ToolResult {
                content: output,
                success: result.status == ShellStatus::Completed,
                metadata: Some(metadata),
            });
        }

        let result = if interactive {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            manager.execute_interactive_with_policy_env(
                command,
                working_dir.as_deref(),
                timeout_ms,
                policy_override,
                extra_env,
            )
        } else if background {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            manager.execute_with_options_env_for_owner(
                command,
                working_dir.as_deref(),
                timeout_ms,
                true,
                stdin_data.as_deref(),
                tty,
                policy_override,
                extra_env,
                shell_job_owner_from_context(context),
            )
        } else {
            execute_foreground_via_background(
                context,
                command,
                timeout_ms,
                stdin_data.as_deref(),
                combined_output,
                policy_override,
                extra_env,
            )
            .await
        };

        match result {
            Ok(result) => {
                let backgrounded_foreground =
                    !background && !interactive && result.status == ShellStatus::Running;
                if (background || backgrounded_foreground)
                    && let (Some(shell_id), Some(task_id)) = (
                        result.task_id.as_deref(),
                        context.runtime.active_task_id.clone(),
                    )
                    && let Ok(mut manager) = context.shell_manager.lock()
                {
                    let _ = manager.tag_linked_task(shell_id, Some(task_id));
                }

                let was_cancelled = context
                    .cancel_token
                    .as_ref()
                    .is_some_and(|token| token.is_cancelled());
                let task_id_str = result.task_id.clone().unwrap_or_default();
                let stdout_summary = summarize_output(&result.stdout);
                let stderr_summary = summarize_output(&result.stderr);
                let summary = if !stderr_summary.is_empty() {
                    stderr_summary.clone()
                } else {
                    stdout_summary.clone()
                };
                let network_restricted_hint =
                    shell_network_restricted_hint(context, command, &result).map(str::to_string);
                let provenance_hint = macos_provenance_hint(&result);
                let python_dependency_hint = python_build_dependency_hint(command, &result);
                let mut output = if interactive {
                    format!(
                        "Interactive command completed (exit code: {:?})",
                        result.exit_code
                    )
                } else if result.status == ShellStatus::Completed {
                    if result.stdout.is_empty() && result.stderr.is_empty() {
                        "(no output)".to_string()
                    } else if result.stderr.is_empty() {
                        result.stdout.clone()
                    } else {
                        format!("{}\n\nSTDERR:\n{}", result.stdout, result.stderr)
                    }
                } else if result.status == ShellStatus::Running {
                    if backgrounded_foreground {
                        format!(
                            "Foreground shell wait moved to /jobs: {task_id_str}\n\nReturns immediately; completion is tracked in task/status state. Keep working; call exec_shell_wait only if you need early output, final output, or wait=true at a true dependency."
                        )
                    } else {
                        format!(
                            "Background task started: {task_id_str}\n\nReturns immediately; completion is tracked in task/status state. Keep working; call exec_shell_wait only if you need early output, final output, or wait=true at a true dependency."
                        )
                    }
                } else if result.status == ShellStatus::Killed && was_cancelled {
                    format!(
                        "Command canceled; process killed.\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
                        result.stdout, result.stderr
                    )
                } else if result.status == ShellStatus::TimedOut {
                    format!(
                        "Command timed out after {timeout_ms}ms; process killed.\n\n{FOREGROUND_TIMEOUT_RECOVERY_HINT}\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
                        result.stdout, result.stderr
                    )
                } else {
                    format!(
                        "Command failed ({})\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
                        exit_code_label(result.exit_code),
                        result.stdout,
                        result.stderr
                    )
                };
                if let Some(hint) = network_restricted_hint.as_deref() {
                    output = format!("{hint}\n\n{output}");
                }
                if let Some(hint) = provenance_hint {
                    output = format!("{hint}\n\n{output}");
                }
                if let Some(hint) = python_dependency_hint {
                    output = format!("{hint}\n\n{output}");
                }

                let mut metadata = json!({
                    "exit_code": result.exit_code,
                    "status": format!("{:?}", result.status),
                    "duration_ms": result.duration_ms,
                    "sandboxed": result.sandboxed,
                    "sandbox_type": result.sandbox_type,
                    "sandbox_denied": result.sandbox_denied,
                    "task_id": result.task_id,
                    "stdout_len": result.stdout_len,
                    "stderr_len": result.stderr_len,
                    "stdout_truncated": result.stdout_truncated,
                    "stderr_truncated": result.stderr_truncated,
                    "stdout_omitted": result.stdout_omitted,
                    "stderr_omitted": result.stderr_omitted,
                    "summary": summary,
                    "stdout_summary": stdout_summary,
                    "stderr_summary": stderr_summary,
                    "safety_level": format!("{:?}", safety.level),
                    "interactive": interactive,
                    "combined_output": combined_output,
                    "canceled": was_cancelled,
                    "execpolicy": execpolicy_decision.as_ref().map(|decision| match decision {
                        ExecPolicyDecision::Allow => json!({
                            "decision": "allow",
                        }),
                        ExecPolicyDecision::Deny(reason) => json!({
                            "decision": "deny",
                            "reason": reason,
                        }),
                        ExecPolicyDecision::AskUser(reason) => json!({
                            "decision": "ask_user",
                            "reason": reason,
                        }),
                    }),
                });
                metadata["backgrounded"] = json!(background || backgrounded_foreground);
                if background || backgrounded_foreground {
                    metadata["auto_resume_on_completion"] = json!(false);
                    metadata["completion_surface"] = json!("task_status");
                    metadata["background_policy"] = json!("nonblocking");
                }
                if result.status == ShellStatus::TimedOut && !background && !interactive {
                    metadata["foreground_timeout_recovery"] = json!({
                        "process_killed": true,
                        "hint": FOREGROUND_TIMEOUT_RECOVERY_HINT,
                        "recommended_tools": [
                            "task_shell_start",
                            "task_shell_wait",
                            "exec_shell",
                            "exec_shell_wait"
                        ],
                        "exec_shell_background": true,
                        "poll_with": ["task_shell_wait", "exec_shell_wait"]
                    });
                }
                if let Some(hint) = network_restricted_hint {
                    metadata["sandbox_network_restricted"] = json!(true);
                    metadata["sandbox_network_denied_hint"] = json!(hint);
                }
                if provenance_hint.is_some() {
                    metadata["macos_provenance_restricted"] = json!(true);
                }
                attach_shell_owner_metadata(&mut metadata, context);
                attach_cargo_failure_summary(&mut metadata, command, &result);
                attach_python_build_dependency_hint(&mut metadata, python_dependency_hint);

                Ok(ToolResult {
                    content: output,
                    success: result.status == ShellStatus::Completed
                        || result.status == ShellStatus::Running,
                    metadata: Some(metadata),
                })
            }
            Err(e) => Ok(ToolResult::error(format!("Shell execution failed: {e}"))),
        }
    }
}

pub struct ShellWaitTool {
    name: &'static str,
}

impl ShellWaitTool {
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

pub struct ShellInteractTool {
    name: &'static str,
}

impl ShellInteractTool {
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

fn required_task_id(input: &serde_json::Value) -> Result<&str, ToolError> {
    input
        .get("task_id")
        .or_else(|| input.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ToolError::missing_field("task_id"))
}

fn build_shell_delta_tool_result(delta: ShellDeltaResult, context: &ToolContext) -> ToolResult {
    let result = delta.result;
    let network_restricted_hint =
        shell_network_restricted_hint(context, &delta.command, &result).map(str::to_string);
    let provenance_hint = macos_provenance_hint(&result);
    let python_dependency_hint = python_build_dependency_hint(&delta.command, &result);
    let stdout_summary = summarize_output(&result.stdout);
    let stderr_summary = summarize_output(&result.stderr);
    let summary = if !stderr_summary.is_empty() {
        stderr_summary.clone()
    } else {
        stdout_summary.clone()
    };

    let mut output = if result.stdout.is_empty() && result.stderr.is_empty() {
        match result.status {
            ShellStatus::Running => "Background task running (no new output).".to_string(),
            ShellStatus::Completed => "(no new output)".to_string(),
            ShellStatus::Failed => {
                format!("Command failed ({})", exit_code_label(result.exit_code))
            }
            ShellStatus::TimedOut => "Command timed out (no new output).".to_string(),
            ShellStatus::Killed => "Command killed (no new output).".to_string(),
        }
    } else if result.stderr.is_empty() {
        result.stdout.clone()
    } else {
        format!("{}\n\nSTDERR:\n{}", result.stdout, result.stderr)
    };
    if let Some(hint) = network_restricted_hint.as_deref() {
        output = format!("{hint}\n\n{output}");
    }
    if let Some(hint) = provenance_hint {
        output = format!("{hint}\n\n{output}");
    }
    if let Some(hint) = python_dependency_hint {
        output = format!("{hint}\n\n{output}");
    }

    let mut metadata = json!({
        "exit_code": result.exit_code,
        "status": format!("{:?}", result.status),
        "duration_ms": result.duration_ms,
        "sandboxed": result.sandboxed,
        "sandbox_type": result.sandbox_type,
        "sandbox_denied": result.sandbox_denied,
        "task_id": result.task_id,
        "stdout_len": result.stdout_len,
        "stderr_len": result.stderr_len,
        "stdout_truncated": result.stdout_truncated,
        "stderr_truncated": result.stderr_truncated,
        "stdout_omitted": result.stdout_omitted,
        "stderr_omitted": result.stderr_omitted,
        "stdout_total_len": delta.stdout_total_len,
        "stderr_total_len": delta.stderr_total_len,
        "summary": summary,
        "stdout_summary": stdout_summary,
        "stderr_summary": stderr_summary,
        "command": delta.command,
        "stream_delta": true,
    });
    attach_shell_owner_metadata(&mut metadata, context);
    attach_cargo_failure_summary(&mut metadata, &delta.command, &result);
    attach_python_build_dependency_hint(&mut metadata, python_dependency_hint);

    let mut tool_result = ToolResult {
        content: output,
        success: matches!(result.status, ShellStatus::Completed | ShellStatus::Running),
        metadata: Some(metadata),
    };
    if let Some(hint) = network_restricted_hint
        && let Some(metadata) = tool_result.metadata.as_mut()
        && let Some(object) = metadata.as_object_mut()
    {
        object.insert("sandbox_network_restricted".to_string(), json!(true));
        object.insert("sandbox_network_denied_hint".to_string(), json!(hint));
    }
    if provenance_hint.is_some()
        && let Some(metadata) = tool_result.metadata.as_mut()
        && let Some(object) = metadata.as_object_mut()
    {
        object.insert("macos_provenance_restricted".to_string(), json!(true));
    }
    tool_result
}

async fn wait_for_shell_delta_cancellable(
    context: &ToolContext,
    task_id: &str,
    timeout_ms: u64,
) -> Result<(ShellDeltaResult, bool), ToolError> {
    let timeout_ms = timeout_ms.clamp(1000, 600_000);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut stdout_accum = String::new();
    let mut stderr_accum = String::new();

    let (command, result, stdout_total_len, stderr_total_len) = loop {
        if context
            .cancel_token
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            let delta = manager
                .get_output_delta(task_id, false, 0)
                .map_err(|err| ToolError::execution_failed(err.to_string()))?;
            append_shell_delta_output(&mut stdout_accum, &mut stderr_accum, &delta.result);
            return Ok((
                shell_delta_with_accumulated_output(
                    delta.command,
                    delta.result,
                    &stdout_accum,
                    &stderr_accum,
                    delta.stdout_total_len,
                    delta.stderr_total_len,
                ),
                true,
            ));
        }

        let delta = {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            manager
                .get_output_delta(task_id, false, 0)
                .map_err(|err| ToolError::execution_failed(err.to_string()))?
        };

        let stdout_total_len = delta.stdout_total_len;
        let stderr_total_len = delta.stderr_total_len;
        let command = delta.command.clone();
        append_shell_delta_output(&mut stdout_accum, &mut stderr_accum, &delta.result);

        let status = delta.result.status.clone();
        if status != ShellStatus::Running || Instant::now() >= deadline {
            break (command, delta.result, stdout_total_len, stderr_total_len);
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    Ok((
        shell_delta_with_accumulated_output(
            command,
            result,
            &stdout_accum,
            &stderr_accum,
            stdout_total_len,
            stderr_total_len,
        ),
        false,
    ))
}

fn append_shell_delta_output(
    stdout_accum: &mut String,
    stderr_accum: &mut String,
    result: &ShellResult,
) {
    if !result.stdout.is_empty() {
        stdout_accum.push_str(&result.stdout);
    }
    if !result.stderr.is_empty() {
        stderr_accum.push_str(&result.stderr);
    }
}

fn shell_delta_with_accumulated_output(
    command: String,
    mut result: ShellResult,
    stdout_accum: &str,
    stderr_accum: &str,
    stdout_total_len: usize,
    stderr_total_len: usize,
) -> ShellDeltaResult {
    let (stdout, stdout_meta) = truncate_with_meta(stdout_accum);
    let (stderr, stderr_meta) = truncate_with_meta(stderr_accum);
    result.stdout = stdout;
    result.stderr = stderr;
    result.stdout_len = stdout_meta.original_len;
    result.stderr_len = stderr_meta.original_len;
    result.stdout_omitted = stdout_meta.omitted;
    result.stderr_omitted = stderr_meta.omitted;
    result.stdout_truncated = stdout_meta.truncated;
    result.stderr_truncated = stderr_meta.truncated;

    ShellDeltaResult {
        command,
        result,
        stdout_total_len,
        stderr_total_len,
    }
}

pub struct ShellCancelTool;

#[async_trait]
impl ToolSpec for ShellCancelTool {
    fn name(&self) -> &'static str {
        "exec_shell_cancel"
    }

    fn description(&self) -> &'static str {
        "Cancel a running background shell task by task_id, or cancel all running background shell tasks with all=true."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID returned by exec_shell or task_shell_start"
                },
                "id": {
                    "type": "string",
                    "description": "Alias for task_id"
                },
                "all": {
                    "type": "boolean",
                    "description": "Cancel all currently running background shell tasks"
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let cancel_all = optional_bool(&input, "all", false);
        let mut manager = context
            .shell_manager
            .lock()
            .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;

        if cancel_all {
            let results = manager
                .kill_running()
                .map_err(|err| ToolError::execution_failed(err.to_string()))?;
            if results.is_empty() {
                return Ok(ToolResult {
                    content: "No running background commands.".to_string(),
                    success: true,
                    metadata: Some(json!({
                        "status": "Noop",
                        "canceled": 0,
                        "task_ids": [],
                    })),
                });
            }

            let task_ids = results
                .iter()
                .filter_map(|result| result.task_id.clone())
                .collect::<Vec<_>>();
            return Ok(ToolResult {
                content: format!(
                    "Canceled {} background command{}: {}",
                    task_ids.len(),
                    if task_ids.len() == 1 { "" } else { "s" },
                    task_ids.join(", ")
                ),
                success: true,
                metadata: Some(json!({
                    "status": "Killed",
                    "canceled": task_ids.len(),
                    "task_ids": task_ids,
                })),
            });
        }

        let task_id = required_task_id(&input)?;
        let result = manager
            .kill(task_id)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        let task_id = result
            .task_id
            .clone()
            .unwrap_or_else(|| task_id.to_string());
        Ok(ToolResult {
            content: format!("Canceled background command: {task_id}"),
            success: true,
            metadata: Some(json!({
                "status": format!("{:?}", result.status),
                "task_id": task_id,
                "exit_code": result.exit_code,
                "duration_ms": result.duration_ms,
            })),
        })
    }
}

#[async_trait]
impl ToolSpec for ShellWaitTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn model_visible(&self) -> bool {
        // `exec_wait` 是遗留别名；只有 `exec_shell_wait` 对模型可见。
        self.name == "exec_shell_wait"
    }

    fn description(&self) -> &'static str {
        "Inspect a background shell task and return incremental output without blocking by default. Set wait=true only for a deliberate dependency barrier. Turn cancellation stops waiting but leaves the background task running."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID returned by exec_shell"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 30000, max: 600000). Use a higher value for long-running builds, CI watchers, and interactive commands that are expected to keep producing output."
                },
                "wait": {
                    "type": "boolean",
                    "default": false,
                    "description": "Snapshot the latest background output and return immediately (default). Background job completions are tracked in task/status state, so normally do not wait. Set wait=true only for a deliberate barrier at a true dependency or final gate."
                }
            },
            "required": ["task_id"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let task_id = required_task_id(&input)?;
        let wait = optional_bool(&input, "wait", false);
        let timeout_ms = optional_u64(&input, "timeout_ms", 30_000);

        let (delta, wait_canceled) = if wait {
            wait_for_shell_delta_cancellable(context, task_id, timeout_ms).await?
        } else {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            let delta = manager
                .get_output_delta(task_id, false, timeout_ms)
                .map_err(|err| ToolError::execution_failed(err.to_string()))?;
            (delta, false)
        };

        let status = delta.result.status.clone();
        let mut result = build_shell_delta_tool_result(delta, context);
        if wait_canceled {
            if matches!(status, ShellStatus::Running) {
                result.content = format!(
                    "Wait canceled; background shell task {task_id} is still running.\n\n{}",
                    result.content
                );
            }
            if let Some(metadata) = result.metadata.as_mut()
                && let Some(object) = metadata.as_object_mut()
            {
                object.insert("wait_canceled".to_string(), json!(true));
            }
        }

        Ok(result)
    }
}

#[async_trait]
impl ToolSpec for ShellInteractTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn model_visible(&self) -> bool {
        // `exec_interact` 是遗留别名；只有 `exec_shell_interact` 对模型可见。
        self.name == "exec_shell_interact"
    }

    fn description(&self) -> &'static str {
        "Send input to a background shell task and return incremental output."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID returned by exec_shell"
                },
                "input": {
                    "type": "string",
                    "description": "Input to send to the task's stdin"
                },
                "stdin": {
                    "type": "string",
                    "description": "Alias for input"
                },
                "data": {
                    "type": "string",
                    "description": "Alias for input"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Wait for output after sending input (default: 1000)"
                },
                "close_stdin": {
                    "type": "boolean",
                    "description": "Close stdin after sending input"
                }
            },
            "required": ["task_id"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::ExecutesCode,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let task_id = required_task_id(&input)?;
        let close_stdin = optional_bool(&input, "close_stdin", false);
        let timeout_ms = optional_u64(&input, "timeout_ms", 1_000);
        let interaction_input = input
            .get("input")
            .or_else(|| input.get("stdin"))
            .or_else(|| input.get("data"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");

        {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            if !interaction_input.is_empty() || close_stdin {
                manager
                    .write_stdin(task_id, interaction_input, close_stdin)
                    .map_err(|err| ToolError::execution_failed(err.to_string()))?;
            }
        }

        let mut elapsed = 0u64;
        loop {
            if context
                .cancel_token
                .as_ref()
                .is_some_and(|token| token.is_cancelled())
            {
                let mut manager = context
                    .shell_manager
                    .lock()
                    .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
                let delta = manager
                    .get_output_delta(task_id, false, 0)
                    .map_err(|err| ToolError::execution_failed(err.to_string()))?;
                let mut result = build_shell_delta_tool_result(delta, context);
                if let Some(metadata) = result.metadata.as_mut()
                    && let Some(object) = metadata.as_object_mut()
                {
                    object.insert("wait_canceled".to_string(), json!(true));
                }
                return Ok(result);
            }

            let delta = {
                let mut manager = context
                    .shell_manager
                    .lock()
                    .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
                manager
                    .get_output_delta(task_id, false, 0)
                    .map_err(|err| ToolError::execution_failed(err.to_string()))?
            };

            if !delta.result.stdout.is_empty()
                || !delta.result.stderr.is_empty()
                || delta.result.status != ShellStatus::Running
                || elapsed >= timeout_ms
            {
                return Ok(build_shell_delta_tool_result(delta, context));
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
            elapsed = elapsed.saturating_add(50);
        }
    }
}

/// 用于将笔记追加到笔记文件的工具。
pub struct NoteTool;

#[async_trait]
impl ToolSpec for NoteTool {
    fn name(&self) -> &'static str {
        "note"
    }

    fn description(&self) -> &'static str {
        "Append a note to the agent notes file for persistent context across sessions."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The note content to append"
                }
            },
            "required": ["content"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto // 笔记是低风险的
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let note_content = required_str(&input, "content")?;

        // 确保父目录存在
        if let Some(parent) = context.notes_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ToolError::execution_failed(format!("Failed to create notes directory: {e}"))
            })?;
        }

        // 追加到笔记文件
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&context.notes_path)
            .map_err(|e| ToolError::execution_failed(format!("Failed to open notes file: {e}")))?;

        writeln!(file, "\n---\n{note_content}")
            .map_err(|e| ToolError::execution_failed(format!("Failed to write note: {e}")))?;

        Ok(ToolResult::success(format!(
            "Note appended to {}",
            context.notes_path.display()
        )))
    }
}

#[cfg(test)]
mod tests;
