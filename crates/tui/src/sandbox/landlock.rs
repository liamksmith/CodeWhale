//! Linux Landlock 沙箱实现。
//!
//! Landlock 是 Linux 内核 5.13 引入的安全机制，允许进程限制自身的访问权限。
//! 与 macOS 上使用外部 sandbox-exec 包装器的 Seatbelt 不同，Landlock 直接将限制
//! 应用于当前进程。
//!
//! # 要求
//!
//! - 启用了 Landlock 的 Linux 内核 5.13 或更高版本
//! - 内核必须使用 `CONFIG_SECURITY_LANDLOCK=y` 编译
//!
//! # 工作原理
//!
//! 1. 使用所需限制创建 landlock 规则集
//! 2. 添加规则以允许特定文件路径
//! 3. 使用规则集限制进程
//!
//! 注意：一旦限制，进程无法获得更多权限。

use super::{CommandSpec, SandboxPolicy};
use std::ffi::CString;
use std::path::Path;

/// 检查系统上是否可用 Landlock。
pub fn is_available() -> bool {
    // 检查 landlock 系统调用是否可用
    #[cfg(target_os = "linux")]
    {
        // 尝试创建一个最小规则集来测试可用性
        // Landlock ABI 版本检查
        // 安全保证：系统调用对 ABI 探测使用空规则集指针，不会解引用它。
        unsafe {
            let result = libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<libc::c_void>(),
                0usize,
                LANDLOCK_CREATE_RULESET_VERSION,
            );
            result >= 0
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// 获取内核支持的 Landlock ABI 版本。
#[cfg(target_os = "linux")]
pub fn get_abi_version() -> Option<i32> {
    // 安全保证：系统调用对 ABI 探测使用空规则集指针，不会解引用它。
    unsafe {
        let result = libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        );
        if result >= 0 {
            i32::try_from(result).ok()
        } else {
            None
        }
    }
}

// Landlock 系统调用常量（尚未在 libc crate 中）
#[cfg(target_os = "linux")]
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1 << 0;

#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_EXECUTE: u64 = 1 << 0;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 2;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 3;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 12;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_REFER: u64 = 1 << 13;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_TRUNCATE: u64 = 1 << 14;

// 组合
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_READ: u64 = LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR;

#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_WRITE: u64 = LANDLOCK_ACCESS_FS_WRITE_FILE
    | LANDLOCK_ACCESS_FS_REMOVE_DIR
    | LANDLOCK_ACCESS_FS_REMOVE_FILE
    | LANDLOCK_ACCESS_FS_MAKE_DIR
    | LANDLOCK_ACCESS_FS_MAKE_REG
    | LANDLOCK_ACCESS_FS_MAKE_SYM
    | LANDLOCK_ACCESS_FS_TRUNCATE;

/// Landlock 规则集属性结构
#[cfg(target_os = "linux")]
#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
}

/// Landlock 路径下层属性结构
#[cfg(target_os = "linux")]
#[repr(C)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

/// 规则类型常量
#[cfg(target_os = "linux")]
const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;

/// 已配置的 Landlock 沙箱
#[cfg(target_os = "linux")]
pub struct LandlockSandbox {
    ruleset_fd: i32,
    policy: SandboxPolicy,
}

#[cfg(target_os = "linux")]
impl LandlockSandbox {
    /// 从策略创建新的 Landlock 沙箱
    pub fn from_policy(policy: &SandboxPolicy) -> std::io::Result<Self> {
        // 确定要处理（限制）的文件系统访问
        let handled_access =
            LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ | LANDLOCK_ACCESS_FS_WRITE;

        let attr = LandlockRulesetAttr {
            handled_access_fs: handled_access,
        };

        // 创建规则集
        // 安全保证：`attr` 在系统调用期间是有效指针，大小正确。
        let ruleset_fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &raw const attr,
                std::mem::size_of::<LandlockRulesetAttr>(),
                0u32,
            )
        };

        if ruleset_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let ruleset_fd = i32::try_from(ruleset_fd).map_err(|_| {
            std::io::Error::other("Failed to create Landlock ruleset: file descriptor out of range")
        })?;

        Ok(Self {
            ruleset_fd,
            policy: policy.clone(),
        })
    }

    /// 为路径添加只读规则
    pub fn allow_read(&self, path: &Path) -> std::io::Result<()> {
        self.add_rule(path, LANDLOCK_ACCESS_FS_READ | LANDLOCK_ACCESS_FS_EXECUTE)
    }

    /// 为路径添加读写规则
    pub fn allow_write(&self, path: &Path) -> std::io::Result<()> {
        self.add_rule(
            path,
            LANDLOCK_ACCESS_FS_READ | LANDLOCK_ACCESS_FS_WRITE | LANDLOCK_ACCESS_FS_EXECUTE,
        )
    }

    /// 向规则集添加路径规则
    fn add_rule(&self, path: &Path, access: u64) -> std::io::Result<()> {
        let path_cstr = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid path"))?;

        // 打开路径获取文件描述符
        // 安全保证：`path_cstr` 以 NUL 结尾且在调用期间保持有效。
        let fd = unsafe { libc::open(path_cstr.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };

        if fd < 0 {
            // 路径不存在，跳过此规则
            return Ok(());
        }

        let attr = LandlockPathBeneathAttr {
            allowed_access: access,
            parent_fd: fd,
        };

        // 安全保证：`attr` 在系统调用期间是有效指针。
        let result = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                self.ruleset_fd,
                LANDLOCK_RULE_PATH_BENEATH,
                &raw const attr,
                0u32,
            )
        };

        // 安全保证：`fd` 是来自 libc::open 的有效文件描述符。
        unsafe {
            libc::close(fd);
        }

        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(())
    }

    /// 将沙箱应用于当前进程
    ///
    /// 警告：这对当前进程是不可逆的！
    pub fn apply(&self) -> std::io::Result<()> {
        // 首先，使用 prctl 丢弃特权
        // 安全保证：prctl 调用使用常量参数，不访问内存。
        let result = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }

        // 现在限制进程
        // 安全保证：系统调用使用有效的规则集 fd，没有指针参数。
        let result =
            unsafe { libc::syscall(libc::SYS_landlock_restrict_self, self.ruleset_fd, 0u32) };

        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for LandlockSandbox {
    fn drop(&mut self) {
        // 安全保证：`ruleset_fd` 是由 landlock 创建的有效描述符。
        unsafe {
            libc::close(self.ruleset_fd);
        }
    }
}

/// 创建一个在运行命令之前设置 Landlock 的辅助脚本。
///
/// 由于 Landlock 限制当前进程，我们需要一个辅助工具：
/// 1. 设置 Landlock 规则集
/// 2. 应用限制
/// 3. 执行目标命令
///
/// 返回要运行的命令及其辅助参数。
#[cfg(target_os = "linux")]
pub fn create_landlock_wrapper(
    spec: &CommandSpec,
    _writable_paths: &[std::path::PathBuf],
    _readable_paths: &[std::path::PathBuf],
) -> Vec<String> {
    // 为简化起见，我们将使用一个通过辅助二进制文件应用 Landlock 的 shell 包装器
    // 在生产中，这将是一个作为 CLI 一部分的编译二进制文件

    // 目前，只需返回原始命令而不进行沙箱处理
    // 完整实现将包含一个编译好的 landlock-helper 二进制文件
    let mut cmd = vec![spec.program.clone()];
    cmd.extend(spec.args.clone());
    cmd
}

/// 检测故障是由 Landlock 还是 seccomp 拒绝引起的。
///
/// 检查 Landlock 特定模式（EACCES/EPERM）和 seccomp 特定模式
///（Bad system call / SIGSYS）。Seccomp 违规通过相同的 `was_denied`
/// 路径报告，因此调用者不需要区分是哪个层阻止了操作。
#[cfg(target_os = "linux")]
pub fn detect_denial(exit_code: i32, stderr: &str) -> bool {
    if exit_code == 0 {
        return false;
    }

    // Landlock 拒绝通常导致 EACCES 或 EPERM。
    let landlock_denial = stderr.contains("Permission denied")
        || stderr.contains("Operation not permitted")
        || stderr.contains("EACCES")
        || stderr.contains("EPERM");

    // Seccomp 拒绝（#2182）：SIGSYS（退出码 31 或 "Bad system call"）。
    let seccomp_denial = exit_code == 31
        || stderr.contains("Bad system call")
        || stderr.contains("bad system call")
        || stderr.contains("SIGSYS")
        || stderr.contains("seccomp");

    landlock_denial || seccomp_denial
}

// 非 Linux 平台的桩实现
#[cfg(not(target_os = "linux"))]
pub fn get_abi_version() -> Option<i32> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn detect_denial(_exit_code: i32, _stderr: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available() {
        // 此测试无论平台如何都会通过
        let _ = is_available();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_get_abi_version() {
        // 可用性取决于内核配置
        let _ = get_abi_version();
    }

    #[test]
    fn test_detect_denial() {
        #[cfg(target_os = "linux")]
        {
            assert!(detect_denial(1, "Permission denied"));
            assert!(detect_denial(1, "Operation not permitted"));
            assert!(!detect_denial(0, "Success"));
        }
    }
}
