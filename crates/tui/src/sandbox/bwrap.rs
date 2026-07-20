//! Bubblewrap (bwrap) Linux 沙箱透传 (#2184)。
//!
//! Bubblewrap 是由 Flatpak 和其他项目使用的无需 setuid 的容器运行时。
//! 它创建了一个具有可配置绑定挂载的新挂载命名空间，
//! 提供文件系统隔离，无需 root 权限。
//!
//! # 工作原理
//!
//! 当 `/usr/bin/bwrap` 存在且配置键 `[sandbox] prefer_bwrap` 设置为 `true` 时，
//! exec_shell 命令将通过 bwrap 而不是仅依赖 Landlock 来路由。bwrap 调用如下：
//!
//! ```text
//! bwrap \
//!   --ro-bind / / \
//!   --bind <cwd> <cwd> \
//!   --chdir <cwd> \
//!   --unshare-all \
//!   -- <program> <args>
//! ```
//!
//! 这会创建整个文件系统的只读视图，仅将工作目录设为可写。
//!
//! # 重要说明
//!
//! 我们并不附带 bwrap。用户必须自行安装：
//!
//! - Ubuntu/Debian：`apt install bubblewrap`
//! - Fedora：`dnf install bubblewrap`
//! - Arch：`pacman -S bubblewrap`
//!
//! 如果未安装 bwrap，我们将回退到 Landlock。

/// bubblewrap 二进制文件的规范路径。
#[cfg(target_os = "linux")]
pub const BWRAP_PATH: &str = "/usr/bin/bwrap";

/// 检查 bubblewrap 是否已安装且可执行。
#[cfg(target_os = "linux")]
pub fn is_available() -> bool {
    std::path::Path::new(BWRAP_PATH).exists()
}

#[cfg(not(target_os = "linux"))]
pub fn is_available() -> bool {
    false
}

/// 构建一个包装给定程序和参数的 bwrap 命令。
///
/// 返回的命令向量适合用作 `ExecEnv.command`——
/// 它用 bwrap 调用替换正常的 program+args，
/// 设置只读根文件系统，仅将指定的工作目录设为可写。
///
/// # 参数
///
/// - `cwd` — 工作目录，将被设置为可写绑定挂载
/// - `program` — 要在容器内运行的程序
/// - `args` — 传递给程序的参数
///
/// # 返回值
///
/// 表示完整 bwrap 调用的 `Vec<String>`。
#[cfg(target_os = "linux")]
pub fn build_bwrap_command(cwd: &std::path::Path, program: &str, args: &[String]) -> Vec<String> {
    let mut cmd: Vec<String> = Vec::with_capacity(10 + args.len());

    cmd.push(BWRAP_PATH.to_string());

    // 只读绑定挂载整个根文件系统。
    cmd.push("--ro-bind".to_string());
    cmd.push("/".to_string());
    cmd.push("/".to_string());

    // 以读写方式绑定挂载工作目录。
    let cwd_str = cwd.to_string_lossy().to_string();
    cmd.push("--bind".to_string());
    cmd.push(cwd_str.clone());
    cmd.push(cwd_str.clone());

    // 在容器内切换到工作目录。
    cmd.push("--chdir".to_string());
    cmd.push(cwd_str);

    // 取消共享所有命名空间以实现最大隔离。
    cmd.push("--unshare-all".to_string());

    // bwrap 参数与要运行的命令之间的分隔符。
    cmd.push("--".to_string());

    // 实际的程序和参数。
    cmd.push(program.to_string());
    cmd.extend(args.iter().cloned());

    cmd
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
    fn test_build_bwrap_command_structure() {
        let cwd = std::path::Path::new("/home/user/project");
        let cmd = build_bwrap_command(cwd, "sh", &["-c".to_string(), "echo hi".to_string()]);

        // 应以 bwrap 开头
        assert_eq!(cmd[0], "/usr/bin/bwrap");

        // 应包含根目录的 ro-bind
        assert!(cmd.contains(&"--ro-bind".to_string()));

        // 应包含 --chdir
        assert!(cmd.contains(&"--chdir".to_string()));

        // 应以命令结尾
        assert_eq!(cmd[cmd.len() - 1], "echo hi");
        assert_eq!(cmd[cmd.len() - 2], "-c");
        assert_eq!(cmd[cmd.len() - 3], "sh");
    }
}
