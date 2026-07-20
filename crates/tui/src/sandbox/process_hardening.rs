//! Linux 沙箱纵深防御的进程强化 (#2183)。
//!
//! 本模块对 codewhale-tui 进程本身应用内核级限制。
//! 与限制为 shell 命令生成的子进程的 Landlock/seccomp 不同，
//! 这些强化措施保护 *父级* TUI 进程免受信息泄漏和权限提升向量的影响。
//!
//! # 顺序约束
//!
//! `apply_process_hardening()` 必须在 Tokio 运行时启动 **之前** 以及
//! 任何工作线程生成 **之前** 调用。原因是：
//!
//! 1. `PR_SET_DUMPABLE`——一旦设置为 0，该进程不能被 ptrace，
//!    且 `/proc/self/` 变为 root 所有。这必须在任何线程存在之前完成，
//!    因为内核按线程组应用 dumpable 状态，在线程活跃后更改它
//!    可能与 `/proc` 查找竞态。
//!
//! 2. `PR_SET_NO_NEW_PRIVS`——阻止该进程及其所有后代
//!    通过 setuid/setgid/fscaps 获得新特权。这是不可逆的，
//!    必须在执行任何可能（错误地）依赖特权边界的辅助二进制文件
//!    或子进程之前应用。
//!
//! 3. `RLIMIT_CORE`——禁用核心转储，以便敏感的内存数据
//!   （API 密钥、令牌、提示内容）在崩溃时永远不会写入磁盘。
//!    在数据加载到内存之前设置此选项是最安全的姿态。
//!
//! # 平台支持
//!
//! 这些强化措施仅适用于 Linux（它们使用 `libc` crate 中的 `prctl` 和 `setrlimit`）。
//! 在非 Linux 平台上，`apply_process_hardening()` 是一个打印调试级别日志的空操作。

/// 应用进程级强化措施。
///
/// 在 Linux 上执行以下操作：
/// - 将 `PR_SET_DUMPABLE` 设置为 0（阻止 ptrace、核心转储）
/// - 将 `PR_SET_NO_NEW_PRIVS` 设置为 1（不可逆的无新特权）
/// - 将 `RLIMIT_CORE` 设置为 0（禁用核心转储）
///
/// 在非 Linux 平台上这是一个空操作。
///
/// # Panics
///
/// 不会 panic。失败会通过 `tracing::warn` 记录，因为强化是纵深防御——
/// 即使这些 prctl 失败（例如在某些受限制的容器中），
/// 沙箱仍然保护子进程。
pub fn apply_process_hardening() {
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    {
        apply_linux_hardening();
    }
    #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
    {
        tracing::debug!("Process hardening skipped: not on Linux");
    }
}

/// Linux 特有的强化实现。
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
fn apply_linux_hardening() {
    // ── PR_SET_DUMPABLE = 0 ────────────────────────────────────────────────
    //
    // 当 dumpable 为 0 时：
    // - 非 root 用户无法 ptrace 该进程
    // - /proc/<pid>/ 归 root:root 所有（模式 0400）
    // - 不产生核心转储
    //
    // 来自 openai/codex codex-rs/codex-sandbox/src/linux.rs 的模式；重新实现。
    //
    // 安全性：带有 PR_SET_DUMPABLE 的 prctl 仅修改调用进程。
    let result = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0i64, 0i64, 0i64, 0i64) };
    if result != 0 {
        let err = std::io::Error::last_os_error();
        tracing::warn!(
            "PR_SET_DUMPABLE failed ({}); continuing without this hardening",
            err
        );
    } else {
        tracing::debug!("PR_SET_DUMPABLE=0 applied");
    }

    // ── PR_SET_NO_NEW_PRIVS = 1 ────────────────────────────────────────────
    //
    // 一旦设置，此进程及其任何后代都无法通过 setuid、setgid、
    // 文件 capabilities 或 SELinux 转换等 LSM 获得新特权。
    // 这是内核提供的最强大的反权限提升原语。
    //
    // 来自 openai/codex codex-rs/codex-sandbox/src/linux.rs 的模式；重新实现。
    //
    // 安全性：带有 PR_SET_NO_NEW_PRIVS 的 prctl 仅修改调用进程
    // 及其未来的后代。
    let result = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1i64, 0i64, 0i64, 0i64) };
    if result != 0 {
        let err = std::io::Error::last_os_error();
        tracing::warn!(
            "PR_SET_NO_NEW_PRIVS failed ({}); continuing without this hardening",
            err
        );
    } else {
        tracing::debug!("PR_SET_NO_NEW_PRIVS=1 applied");
    }

    // ── RLIMIT_CORE = 0 ────────────────────────────────────────────────────
    //
    // 在 rlimit 级别禁用核心转储。与 PR_SET_DUMPABLE=0 结合使用，
    // 提供双重保障，确保永远不会写入核心文件。
    //
    // 安全性：setrlimit 仅修改调用进程的资源限制。
    let rlim_core = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let result = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const rlim_core) };
    if result != 0 {
        let err = std::io::Error::last_os_error();
        tracing::warn!(
            "RLIMIT_CORE failed ({}); continuing without this hardening",
            err
        );
    } else {
        tracing::debug!("RLIMIT_CORE=0 applied");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_process_hardening_does_not_panic() {
        // 此测试存在是为了确保即使在不支持强化的平台上，
        // 函数也可以在不 panic 的情况下调用。
        apply_process_hardening();
    }
}
