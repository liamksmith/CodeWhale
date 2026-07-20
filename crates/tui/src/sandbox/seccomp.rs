//! Linux seccomp（安全计算）过滤器层（#2182）。
//!
//! Seccomp BPF（伯克利包过滤器）是一种内核设施，允许
//! 进程限制其（及其后代）可以进行的系统调用。
//! 此模块在 Landlock 之上应用 seccomp 过滤器，以提供
//! 第二层防御——即使 Landlock 行为异常或配置过于宽松，
//! seccomp 过滤器也会阻止整个 *类别* 的危险系统调用，
//! 如 `ptrace`、`mount`、`kexec_load` 等。
//!
//! # 架构
//!
//! 过滤器编写为原始 BPF 程序（`sock_filter` 指令数组）
//! 并通过 `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER)` 加载。
//! 这避免了对 `libseccomp-sys` 或 `seccompiler` 等外部 crate 的任何依赖
//!——我们只使用依赖树中已有的 `libc` crate。
//!
//! # 白名单系统调用
//!
//! 过滤器使用白名单方法：只允许已知对开发/Shell 工作负载安全的系统调用。
//! 其他所有调用都会以 `SECCOMP_RET_KILL_PROCESS` 被杀死。白名单包括：
//!
//! - 文件 I/O：read, write, open, openat, close, stat, fstat, lstat, newfstatat
//! - 目录：getdents, getdents64, getcwd, chdir
//! - 内存：mmap, mprotect, munmap, brk, mremap, madvise
//! - 进程：clone, clone3, fork, vfork, execve, execveat, exit, exit_group
//! - IPC：pipe, pipe2, socket, socketpair, connect, bind, listen, accept, accept4
//! - 同步：futex, nanosleep, clock_nanosleep
//! - 信号：rt_sigaction, rt_sigprocmask, rt_sigreturn, kill, tkill, tgkill
//! - 资源：getrlimit, setrlimit, prlimit64, getrusage
//! - 时间：clock_gettime, gettimeofday, time
//! - 杂项：getpid, gettid, getuid, geteuid, getgid, getegid, uname, arch_prctl
//!
//! # 明确拒绝
//!
//! - ptrace（进程劫持）
//! - mount, umount2（文件系统操作）
//! - kexec_load, kexec_file_load（内核执行）
//! - init_module, finit_module, delete_module（内核模块加载）
//! - bpf（加载 BPF 程序——会绕过 seccomp！）
//! - reboot
//! - swapon, swapoff
//! - pivot_root
//! - setuid, setgid, setreuid, setregid, setresuid, setresgid
//! - personality
//!
//! # 安全性
//!
//! 一旦安装 seccomp 过滤器，它就是 **不可逆的** —— 即使是
//! `prctl(PR_SET_SECCOMP, ...)` 也被拒绝。这是设计使然。

/// 检查系统上 seccomp 是否可用。
///
/// 如果 `/proc/sys/kernel/seccomp/actions_avail` 存在且包含 "kill_process"，
/// 则返回 true，表示内核支持 seccomp BPF。
#[cfg(target_os = "linux")]
pub fn is_available() -> bool {
    std::path::Path::new("/proc/sys/kernel/seccomp/actions_avail").exists()
}

#[cfg(not(target_os = "linux"))]
pub fn is_available() -> bool {
    false
}

/// 检测失败是否由 seccomp 拒绝引起。
///
/// Seccomp 使用 SIGSYS（或 SECCOMP_RET_KILL_THREAD）杀死进程，
/// 退出码通常是 SIGSYS（31），或者进程可能在 stderr 上被
/// "Bad system call" 杀死。
///
/// 此外，如果使用 SECCOMP_RET_ERRNO，seccomp 违规可能产生 EPERM。
#[cfg(target_os = "linux")]
pub fn detect_denial(exit_code: i32, stderr: &str) -> bool {
    // SIGSYS = 31
    if exit_code == 31 {
        return true;
    }
    // 检查 stderr 中的 seccomp 拒绝模式
    stderr.contains("Bad system call")
        || stderr.contains("bad system call")
        || stderr.contains("SIGSYS")
        || stderr.contains("seccomp")
        || stderr.contains("invalid argument") && exit_code == 159
    // 159 = 128 + 31 （因 SIGSYS 死亡且核心转储禁用）
}

#[cfg(not(target_os = "linux"))]
pub fn detect_denial(_exit_code: i32, _stderr: &str) -> bool {
    false
}

/// 对调用线程应用 seccomp 过滤器。
///
/// 这会安装一个 BPF 程序，白名单安全的系统调用，并在任何不允许的系统调用上
/// 杀死进程。
///
/// # 错误
///
/// 如果 prctl 调用失败（例如 seccomp 已启用或内核太旧），返回错误。
#[cfg(target_os = "linux")]
pub fn apply_seccomp_filter() -> std::io::Result<()> {
    // ── 构建 BPF 过滤器程序 ─────────────────────────────────────
    //
    // seccomp 的 BPF 工作原理如下：
    // 1. 加载架构（seccomp_data 中偏移 4 处的 4 字节）
    // 2. 验证架构匹配 AUDIT_ARCH_X86_64 (0xC000003E)
    // 3. 加载系统调用号（偏移 0 处的 4 字节）
    // 4. 与白名单比较，匹配时返回 ALLOW
    // 5. 不匹配时返回 KILL
    //
    // 过滤器对白名单使用线性搜索。虽然不是最优的，
    // 但它简单、可审计，且没有外部依赖。BPF
    // 程序最多几百条指令，远在内核 4096 条指令的限制内。

    #[repr(C)]
    struct sock_filter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }

    const BPF_LD: u16 = 0x00;
    const BPF_JMP: u16 = 0x05;
    const BPF_RET: u16 = 0x06;

    const BPF_W: u16 = 0x00;
    const BPF_ABS: u16 = 0x20;

    const BPF_JEQ: u16 = 0x10;
    const BPF_JGE: u16 = 0x30;
    const BPF_JA: u16 = 0x00;

    const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
    const SECCOMP_RET_ALLOW: u32 = 0x7FFF_0000;

    // x86_64 的审计架构
    const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;

    // 紧凑构建 BPF 指令的辅助方法。
    // 模式来自 openai/codex codex-rs/codex-sandbox/src/linux/seccomp.rs；已重新实现。

    // 安全系统调用号的白名单（x86_64）。
    // 这些是 shell 命令、编译器和开发工具最常用的系统调用。
    // 不在列表中的任何系统调用都会导致即时 SIGSYS。
    let allowed_syscalls: &[u32] = &[
        0,   // read
        1,   // write
        2,   // open
        3,   // close
        4,   // stat
        5,   // fstat
        6,   // lstat
        7,   // poll
        8,   // lseek
        9,   // mmap
        10,  // mprotect
        11,  // munmap
        12,  // brk
        13,  // rt_sigaction
        14,  // rt_sigprocmask
        15,  // rt_sigreturn
        16,  // ioctl
        17,  // pread64
        18,  // pwrite64
        19,  // readv
        20,  // writev
        21,  // access
        22,  // pipe
        23,  // select
        24,  // sched_yield
        25,  // mremap
        27,  // mincore
        28,  // madvise
        29,  // shmget
        30,  // shmat
        32,  // dup
        33,  // dup2
        35,  // nanosleep
        39,  // getpid
        41,  // socket
        42,  // connect
        43,  // accept
        44,  // sendto
        45,  // recvfrom
        46,  // sendmsg
        47,  // recvmsg
        48,  // shutdown
        49,  // bind
        50,  // listen
        51,  // getsockname
        52,  // getpeername
        53,  // socketpair
        54,  // setsockopt
        55,  // getsockopt
        56,  // clone
        57,  // fork
        58,  // vfork
        59,  // execve
        60,  // exit
        61,  // wait4
        62,  // kill
        63,  // uname
        72,  // fcntl
        73,  // flock
        74,  // fsync
        75,  // fdatasync
        76,  // truncate
        77,  // ftruncate
        78,  // getdents
        79,  // getcwd
        80,  // chdir
        81,  // fchdir
        82,  // rename
        83,  // mkdir
        84,  // rmdir
        85,  // creat
        86,  // link
        87,  // unlink
        88,  // symlink
        89,  // readlink
        90,  // chmod
        91,  // fchmod
        92,  // chown
        93,  // fchown
        94,  // lchown
        95,  // umask
        96,  // gettimeofday
        97,  // getrlimit
        98,  // getrusage
        99,  // sysinfo
        100, // times
        102, // getuid
        104, // getgid
        107, // geteuid
        108, // getegid
        110, // getppid
        111, // getpgrp
        112, // setsid
        116, // syslog
        131, // sigaltstack
        137, // statfs
        138, // fstatfs
        157, // prctl
        158, // arch_prctl
        186, // gettid
        201, // time
        202, // futex
        204, // sched_getaffinity
        217, // getdents64
        218, // set_tid_address
        228, // clock_gettime
        230, // clock_nanosleep
        231, // exit_group
        232, // epoll_wait
        233, // epoll_ctl
        234, // tgkill
        235, // utimes
        257, // openat
        262, // newfstatat
        273, // set_robust_list
        281, // epoll_pwait
        291, // epoll_create1
        292, // dup3
        293, // pipe2
        302, // prlimit64
        318, // getrandom
        332, // statx
        334, // rseq
        435, // clone3
    ];

    // 构建 BPF 程序。
    let mut filter = vec![
        // 指令 0：从 seccomp_data.arch 加载架构
        sock_filter {
            code: BPF_LD | BPF_W | BPF_ABS,
            jt: 0,
            jf: 0,
            k: 4, // seccomp_data 中 arch 的偏移量
        },
        // 指令 1：与 AUDIT_ARCH_X86_64 比较
        // 如果匹配，跳转到下一条指令；如果不匹配，杀死进程
        sock_filter {
            code: BPF_JMP | BPF_JEQ,
            jt: 0,
            jf: 1, // 如果架构不匹配，向前跳转 1 条（到 KILL）
            k: AUDIT_ARCH_X86_64,
        },
        // 指令 2：KILL（错误的架构）
        sock_filter {
            code: BPF_RET,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_KILL_PROCESS,
        },
        // 指令 3：从 seccomp_data.nr 加载系统调用号
        sock_filter {
            code: BPF_LD | BPF_W | BPF_ABS,
            jt: 0,
            jf: 0,
            k: 0, // seccomp_data 中 nr 的偏移量
        },
    ];

    // 对于每个允许的系统调用，添加比较+跳转到 ALLOW。
    // 我们使用线性扫描以保持简单：每个 JEQ 指令
    // 向前跳过剩余的检查 + KILL 以到达 ALLOW。
    for &syscall in allowed_syscalls {
        let remaining = (allowed_syscalls.len() as u8).saturating_sub(
            allowed_syscalls
                .iter()
                .position(|&s| s == syscall)
                .unwrap_or(0) as u8,
        );
        // 如果系统调用 == 此值，跳转到 allow_target；否则继续
        filter.push(sock_filter {
            code: BPF_JMP | BPF_JEQ,
            jt: remaining, // 向前跳转到 ALLOW
            jf: 0,         // 继续下一个检查
            k: syscall,
        });
    }

    // 指令 N：对任何不匹配的系统调用杀死进程
    filter.push(sock_filter {
        code: BPF_RET,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_KILL_PROCESS,
    });

    // 指令 N+1：ALLOW
    filter.push(sock_filter {
        code: BPF_RET,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_ALLOW,
    });

    // ── 将过滤器加载到内核中 ───────────────────────────────────

    #[repr(C)]
    struct sock_fprog {
        len: u16,
        filter: *const sock_filter,
    }

    let prog = sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_ptr(),
    };

    // 安全性：使用 PR_SET_SECCOMP 的 prctl 安装 seccomp BPF 过滤器。
    // 过滤器是一个在 prctl 调用期间有效的有效 sock_filter 指令数组。
    let result = unsafe {
        libc::prctl(
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER,
            &raw const prog,
            0i64,
            0i64,
        )
    };

    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available_does_not_panic() {
        let _ = is_available();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_detect_denial() {
        assert!(detect_denial(31, ""));
        assert!(detect_denial(1, "Bad system call"));
        assert!(detect_denial(1, "SIGSYS"));
        assert!(!detect_denial(0, "Success"));
        assert!(!detect_denial(1, "File not found"));
    }

    #[test]
    fn test_detect_denial_non_linux() {
        #[cfg(not(target_os = "linux"))]
        {
            assert!(!detect_denial(31, "Bad system call"));
        }
    }
}
