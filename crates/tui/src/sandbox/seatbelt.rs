//! macOS Seatbelt（sandbox-exec）配置文件生成。
//!
//! Seatbelt 是 Apple 的强制访问控制框架，使用基于 Scheme 的策略语言
//! 来定义进程可以访问哪些系统资源。此模块根据配置的 `SandboxPolicy`
//! 动态生成沙箱配置文件。
//!
//! # 工作原理
//!
//! 1. 以 SBPL 格式生成 Seatbelt 策略字符串
//! 2. 调用 `/usr/bin/sandbox-exec -p <policy>` 运行命令
//! 3. 内核强制执行策略，阻止未授权的操作
//!
//! # 参考
//!
//! - Apple 的 sandbox(7) 手册页
//! - <https://reverse.put.as/wp-content/uploads/2011/09/Apple-Sandbox-Guide-v1.0.pdf>

// 注意：cfg(target_os = "macos") 已在 mod.rs 的模块级别应用

use super::policy::SandboxPolicy;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// macOS 上 sandbox-exec 二进制文件的路径。
pub const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

/// 提供最小进程功能的基础 Seatbelt 策略。
///
/// 此策略：
/// - 默认拒绝所有操作
/// - 允许进程执行和分支
/// - 允许同一沙箱内的信号
/// - 允许读取用户偏好（许多工具需要）
/// - 允许基本进程内省
/// - 允许写入 /dev/null
/// - 允许读取 sysctl 值
/// - 允许 POSIX 信号量和伪 TTY 操作
const SEATBELT_BASE_POLICY: &str = r#"
(version 1)
(deny default)

; 核心进程操作
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))

; 用户偏好（许多 CLI 工具需要）
(allow user-preference-read)

; 基本 I/O 到 /dev/null
(allow file-write-data
  (require-all
    (path "/dev/null")
    (vnode-type CHARACTER-DEVICE)))

; 系统信息
(allow sysctl-read)

; IPC 原语
(allow ipc-posix-sem)
(allow ipc-posix-shm-read*)
(allow ipc-posix-shm-write-create)
(allow ipc-posix-shm-write-data)
(allow ipc-posix-shm-write-unlink)

; 终端支持（shell 命令必需的）
(allow pseudo-tty)
(allow file-read* file-write* file-ioctl (literal "/dev/ptmx"))
(allow file-read* file-write* file-ioctl (literal "/dev/tty"))
(allow file-read* file-write* file-ioctl (regex #"^/dev/ttys[0-9]+$"))

; macOS 特定设备访问
(allow file-read* (literal "/dev/urandom"))
(allow file-read* (literal "/dev/random"))
(allow file-ioctl (literal "/dev/dtracehelper"))

; Mach IPC（许多系统服务需要）
(allow mach-lookup)
"#;

/// 网络访问策略补充。
const SEATBELT_NETWORK_POLICY: &str = r"
; 网络访问
(allow network-outbound)
(allow network-inbound)
(allow system-socket)
(allow network-bind)
";

/// 检查此系统上是否可用 sandbox-exec 并允许使用。
pub fn is_available() -> bool {
    static SEATBELT_AVAILABLE: OnceLock<bool> = OnceLock::new();

    *SEATBELT_AVAILABLE.get_or_init(|| {
        if !Path::new(SANDBOX_EXEC_PATH).exists() {
            return false;
        }

        let output = Command::new(SANDBOX_EXEC_PATH)
            .args(["-p", "(version 1)(allow default)", "--", "/usr/bin/true"])
            .output();

        match output {
            Ok(result) => result.status.success(),
            Err(_) => false,
        }
    })
}

/// 创建 sandbox-exec 的命令行参数。
///
/// 返回应预先添加到命令前面的参数 Vec。
/// 格式为：`sandbox-exec -p <policy> -D KEY=VALUE ... -- <original command>`
pub fn create_seatbelt_args(
    command: Vec<String>,
    policy: &SandboxPolicy,
    sandbox_cwd: &Path,
) -> Vec<String> {
    let full_policy = generate_policy(policy, sandbox_cwd);
    let params = generate_params(policy, sandbox_cwd);

    let mut args = vec!["-p".to_string(), full_policy];

    // 为变量替换添加参数定义
    for (key, value) in params {
        args.push(format!("-D{}={}", key, value.to_string_lossy()));
    }

    // sandbox-exec 参数与实际命令之间的分隔符
    args.push("--".to_string());
    args.extend(command);

    args
}

/// 为给定的策略生成完整的 Seatbelt 策略字符串。
fn generate_policy(policy: &SandboxPolicy, cwd: &Path) -> String {
    let mut full_policy = SEATBELT_BASE_POLICY.to_string();

    // 添加读取访问策略
    if SandboxPolicy::has_full_disk_read_access() {
        full_policy.push_str("\n; 完整文件系统读取访问\n(allow file-read*)");
    }

    // 添加写入访问策略
    let file_write_policy = generate_write_policy(policy, cwd);
    if !file_write_policy.is_empty() {
        full_policy.push_str("\n\n; 写入访问策略\n");
        full_policy.push_str(&file_write_policy);
    }

    // 如果启用则添加网络策略
    if policy.has_network_access() {
        full_policy.push('\n');
        full_policy.push_str(SEATBELT_NETWORK_POLICY);
    }

    // 添加 Darwin 用户缓存目录访问（许多 macOS 工具需要）
    full_policy.push_str("\n\n; Darwin 用户缓存目录\n");
    full_policy
        .push_str(r#"(allow file-read* file-write* (subpath (param "DARWIN_USER_CACHE_DIR")))"#);

    // 添加工具经常需要的常见 macOS 目录
    full_policy.push_str("\n\n; 常见 macOS 目录\n");
    full_policy.push_str(r#"(allow file-read* (subpath "/usr/lib"))"#);
    full_policy.push('\n');
    full_policy.push_str(r#"(allow file-read* (subpath "/usr/share"))"#);
    full_policy.push('\n');
    full_policy.push_str(r#"(allow file-read* (subpath "/System/Library"))"#);
    full_policy.push('\n');
    full_policy.push_str(r#"(allow file-read* (subpath "/Library/Preferences"))"#);
    full_policy.push('\n');
    full_policy.push_str(r#"(allow file-read* (subpath "/private/var/db"))"#);

    // Cargo home（#558）：cargo build/test/publish 需要访问 ~/.cargo/registry
    // 和 ~/.cargo/git 以获取 crate 元数据、下载的 tarball 和解压的
    // 源码。沙箱化的 workspace-write 之前拒绝这些访问，
    // 使得在 TUI 的 shell 工具内无法运行 `cargo publish`。
    // 读取访问始终允许；当策略允许任何写入时，也会授予写入访问
    //（注册表缓存需要可变才能在缓存未命中时由 `cargo build` 填充）。
    // 当既未设置 `CARGO_HOME` 也未设置 `HOME` 时完全跳过——没有
    // 其中一个，我们就无法将路径接入策略参数。
    if resolve_cargo_home().is_some() {
        full_policy.push_str("\n\n; Cargo home (~/.cargo) — 注册表/索引/git 缓存\n");
        full_policy.push_str(r#"(allow file-read* (subpath (param "CARGO_HOME")))"#);
        if !matches!(policy, SandboxPolicy::ReadOnly) {
            full_policy.push('\n');
            full_policy.push_str(r#"(allow file-write* (subpath (param "CARGO_HOME_REGISTRY")))"#);
            full_policy.push('\n');
            full_policy.push_str(r#"(allow file-write* (subpath (param "CARGO_HOME_GIT")))"#);
        }
    }

    // npm 缓存（#1267）：基于 npx 的 MCP 服务器在首次运行时下载
    // 包时会写入 ~/.npm。没有写入访问，npx 子进程会立即失败
    // 并显示"Stdio transport closed"，使得在默认 workspace-write
    // 策略下，所有 stdio MCP 服务器在 macOS 上均不可用。
    // 读取访问始终允许；写入访问镜像 cargo 的模式——
    // 对所有允许写入的策略授予访问，对 ReadOnly 策略跳过。
    // 当既未设置 `NPM_CONFIG_CACHE` 也未设置 `HOME` 时完全跳过。
    if resolve_npm_cache_dir().is_some() {
        full_policy.push_str("\n\n; npm cache (~/.npm) — npx 包下载\n");
        full_policy.push_str(r#"(allow file-read* (subpath (param "NPM_CACHE_DIR")))"#);
        if !matches!(policy, SandboxPolicy::ReadOnly) {
            full_policy.push('\n');
            full_policy.push_str(r#"(allow file-write* (subpath (param "NPM_CACHE_DIR")))"#);
        }
    }

    full_policy
}

/// 解析用户的 cargo home —— `CARGO_HOME`（如果已设置），否则为 `$HOME/.cargo`。
/// 仅当两种环境变量均未设置的主机上返回 `None`
///（在真正的 macOS 用户账户上基本不会发生；可以在未导出 `HOME` 的
/// CI 容器中发生）。
fn resolve_cargo_home() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("CARGO_HOME")
        && !explicit.trim().is_empty()
    {
        return Some(PathBuf::from(explicit));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".cargo"))
}

/// 解析 npm 缓存目录 —— `NPM_CONFIG_CACHE`（如果已设置），否则为 `$HOME/.npm`。
/// 仅当两种环境变量均未设置时返回 `None`。
fn resolve_npm_cache_dir() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("NPM_CONFIG_CACHE")
        && !explicit.trim().is_empty()
    {
        return Some(PathBuf::from(explicit));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".npm"))
}

/// 生成 Seatbelt 策略的写入访问部分。
fn generate_write_policy(policy: &SandboxPolicy, cwd: &Path) -> String {
    // 完全磁盘写入访问
    if policy.has_full_disk_write_access() {
        return r#"(allow file-write* (regex #"^/"))"#.to_string();
    }

    // 只读——无需写入策略
    if matches!(policy, SandboxPolicy::ReadOnly) {
        return String::new();
    }

    // 工作区写入——枚举允许的路径
    let writable_roots = policy.get_writable_roots(cwd);
    if writable_roots.is_empty() {
        return String::new();
    }

    let mut policies = Vec::new();

    for (index, root) in writable_roots.iter().enumerate() {
        let root_param = format!("WRITABLE_ROOT_{index}");

        if root.read_only_subpaths.is_empty() {
            // 简单情况：整个子树可写
            policies.push(format!("(subpath (param \"{root_param}\"))"));
        } else {
            // 复杂情况：可写但有只读例外
            // 使用 require-all 组合 subpath 与每个例外的 require-not
            let mut parts = vec![format!("(subpath (param \"{}\"))", root_param)];

            for (subpath_index, _) in root.read_only_subpaths.iter().enumerate() {
                let ro_param = format!("WRITABLE_ROOT_{index}_RO_{subpath_index}");
                parts.push(format!("(require-not (subpath (param \"{ro_param}\")))"));
            }

            policies.push(format!("(require-all {})", parts.join(" ")));
        }
    }

    if policies.is_empty() {
        return String::new();
    }

    // 使用 allow 组合所有写入策略
    format!("(allow file-write*\n  {})", policies.join("\n  "))
}

/// 为策略中的变量替换生成参数定义。
///
/// sandbox-exec 允许使用 -DKEY=VALUE 来替换策略中的 `(param "KEY")`。
fn generate_params(policy: &SandboxPolicy, cwd: &Path) -> Vec<(String, PathBuf)> {
    let mut params = Vec::new();

    // 添加可写根目录参数
    let writable_roots = policy.get_writable_roots(cwd);

    for (index, root) in writable_roots.iter().enumerate() {
        let canonical = root
            .root
            .canonicalize()
            .unwrap_or_else(|_| root.root.clone());
        params.push((format!("WRITABLE_ROOT_{index}"), canonical));

        // 添加只读子路径的参数
        for (subpath_index, subpath) in root.read_only_subpaths.iter().enumerate() {
            let canonical_subpath = subpath.canonicalize().unwrap_or_else(|_| subpath.clone());
            params.push((
                format!("WRITABLE_ROOT_{index}_RO_{subpath_index}"),
                canonical_subpath,
            ));
        }
    }

    // 添加 Darwin 用户缓存目录
    if let Some(cache_dir) = get_darwin_user_cache_dir() {
        params.push(("DARWIN_USER_CACHE_DIR".to_string(), cache_dir));
    } else {
        // 回退到合理的默认值
        if let Ok(home) = std::env::var("HOME") {
            params.push((
                "DARWIN_USER_CACHE_DIR".to_string(),
                PathBuf::from(format!("{home}/Library/Caches")),
            ));
        }
    }

    // Cargo home（#558）：与 `generate_policy` 在 `resolve_cargo_home()`
    // 成功时发出的策略行配对。两个辅助函数使用相同的回退链，
    // 因此策略文本和 -DKEY=VALUE 参数保持同步——只发出一个而缺少
    // 另一个，sandbox-exec 会拒绝加载配置文件。
    if let Some(home) = resolve_cargo_home() {
        let canonical_home = home.canonicalize().unwrap_or_else(|_| home.clone());
        params.push((
            "CARGO_HOME_REGISTRY".to_string(),
            canonical_home.join("registry"),
        ));
        params.push(("CARGO_HOME_GIT".to_string(), canonical_home.join("git")));
        params.push(("CARGO_HOME".to_string(), canonical_home));
    }

    // npm 缓存（#1267）：与 `generate_policy` 在 `resolve_npm_cache_dir()`
    // 成功时发出的策略行配对。两个辅助函数使用相同的回退链，
    // 因此策略文本和 -DKEY=VALUE 参数保持同步。
    if let Some(npm_cache) = resolve_npm_cache_dir() {
        let canonical = npm_cache
            .canonicalize()
            .unwrap_or_else(|_| npm_cache.clone());
        params.push(("NPM_CACHE_DIR".to_string(), canonical));
    }

    params
}

/// 使用 confstr 获取 Darwin 用户缓存目录。
///
/// 返回 macOS 分配的用户级缓存目录，通常是类似
/// /var/folders/xx/xxx.../C/ 的路径。
fn get_darwin_user_cache_dir() -> Option<PathBuf> {
    // 使用 libc 调用 confstr 获取 _CS_DARWIN_USER_CACHE_DIR
    let mut buf = vec![0i8; (libc::PATH_MAX as usize) + 1];

    // 安全性：`buf` 是一个大小为 PATH_MAX + 1 的可写缓冲区，用于 confstr。
    let len =
        unsafe { libc::confstr(libc::_CS_DARWIN_USER_CACHE_DIR, buf.as_mut_ptr(), buf.len()) };

    if len == 0 {
        return None;
    }

    // 将 C 字符串转换为 Rust PathBuf
    // 安全性：当 len > 0 时，confstr 保证 `buf` 中的字符串以 NUL 结尾。
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    let path_str = cstr.to_str().ok()?;
    let path = PathBuf::from(path_str);

    // 尝试规范化，但如果失败则返回原始路径
    path.canonicalize().ok().or(Some(path))
}

/// 从命令输出检测沙箱拒绝。
///
/// 如果输出表明沙箱阻止了操作，则返回 true。
pub fn detect_denial(exit_code: i32, stderr: &str) -> bool {
    if exit_code == 0 {
        return false;
    }

    // 常见的沙箱拒绝消息
    let denial_patterns = [
        "Operation not permitted",
        "sandbox-exec",
        "deny(",
        "Sandbox: ",
    ];

    denial_patterns.iter().any(|p| stderr.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 修改 HOME/CARGO_HOME 的测试使用 crate::test_support::lock_test_env()
    // 以避免与在此 crate 中读取这些变量的兄弟测试竞争。
    #[test]
    fn test_is_available() {
        // 此测试仅检查函数不会 panic
        // 在 macOS 上应返回 true，在其他平台上返回 false
        let _ = is_available();
    }

    #[test]
    fn test_generate_policy_default() {
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");
        let result = generate_policy(&policy, cwd);

        assert!(result.contains("(version 1)"));
        assert!(result.contains("(deny default)"));
        assert!(result.contains("(allow file-read*)"));
        assert!(result.contains("file-write*"));
        // 默认策略没有网络
        assert!(!result.contains("network-outbound"));
    }

    #[test]
    fn test_generate_policy_with_network() {
        let policy = SandboxPolicy::workspace_with_network();
        let cwd = Path::new("/tmp/test");
        let result = generate_policy(&policy, cwd);

        assert!(result.contains("network-outbound"));
        assert!(result.contains("network-inbound"));
    }

    #[test]
    fn test_generate_policy_read_only() {
        let policy = SandboxPolicy::ReadOnly;
        let cwd = Path::new("/tmp/test");
        let result = generate_policy(&policy, cwd);

        assert!(result.contains("(allow file-read*)"));
        // 不应有工作区写入规则
        assert!(!result.contains("WRITABLE_ROOT"));
    }

    #[test]
    fn test_generate_params() {
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");
        let params = generate_params(&policy, cwd);

        // 应至少有一个缓存目录参数
        assert!(params.iter().any(|(k, _)| k == "DARWIN_USER_CACHE_DIR"));
    }

    /// #558：cargo publish 需要访问 ~/.cargo/registry；seatbelt
    /// 必须允许其内部的读+写。策略文本和参数表必须同步——
    /// 只发出一个而缺少另一个，sandbox-exec 会拒绝加载配置文件。
    #[test]
    fn test_cargo_home_paths_emitted_in_policy_and_params_when_home_set() {
        let _guard = crate::test_support::lock_test_env();

        // 安全性：HOME / CARGO_HOME 是进程全局的。lock_test_env
        // 序列化修改它们的测试，我们总是在返回前恢复之前的值。
        let saved_home = std::env::var_os("HOME");
        let saved_cargo = std::env::var_os("CARGO_HOME");
        unsafe {
            std::env::set_var("HOME", "/tmp/seatbelt-cargo-test");
            std::env::remove_var("CARGO_HOME");
        }

        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");

        let policy_text = generate_policy(&policy, cwd);
        assert!(policy_text.contains(r#"(allow file-read* (subpath (param "CARGO_HOME")))"#));
        assert!(policy_text.contains("CARGO_HOME_REGISTRY"));
        assert!(policy_text.contains("CARGO_HOME_GIT"));

        let params = generate_params(&policy, cwd);
        assert!(params.iter().any(|(k, _)| k == "CARGO_HOME"));
        assert!(params.iter().any(|(k, _)| k == "CARGO_HOME_REGISTRY"));
        assert!(params.iter().any(|(k, _)| k == "CARGO_HOME_GIT"));

        // 只读策略仍应发出 CARGO_HOME 读取规则但跳过写入。
        let read_only_text = generate_policy(&SandboxPolicy::ReadOnly, cwd);
        assert!(
            read_only_text.contains(r#"(allow file-read* (subpath (param "CARGO_HOME")))"#),
            "read-only mode should still allow reading the cargo registry: {read_only_text}"
        );
        assert!(
            !read_only_text
                .contains(r#"(allow file-write* (subpath (param "CARGO_HOME_REGISTRY")))"#),
            "read-only mode must NOT grant write access to the cargo registry"
        );

        // 恢复。
        // 安全性：恢复测试在入口处保存的先前值。
        unsafe {
            match saved_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match saved_cargo {
                Some(v) => std::env::set_var("CARGO_HOME", v),
                None => std::env::remove_var("CARGO_HOME"),
            }
        }
    }

    /// #558：如果既未设置 `CARGO_HOME` 也未设置 `HOME`，则 cargo 行
    /// 及其参数都必须省略——只发出一个而缺少另一个会在加载时
    /// 导致 sandbox-exec 崩溃。
    #[test]
    fn test_cargo_home_skipped_when_no_env() {
        let _guard = crate::test_support::lock_test_env();

        let saved_home = std::env::var_os("HOME");
        let saved_cargo = std::env::var_os("CARGO_HOME");
        // 安全性：HOME/CARGO_HOME 是进程全局的；lock_test_env 序列化
        // 此处的修改，我们在返回前恢复之前的值。
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("CARGO_HOME");
        }

        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");
        let policy_text = generate_policy(&policy, cwd);
        let params = generate_params(&policy, cwd);

        assert!(!policy_text.contains("CARGO_HOME"));
        assert!(!params.iter().any(|(k, _)| k.starts_with("CARGO_HOME")));

        // 恢复。
        // 安全性：恢复测试在入口处保存的先前值。
        unsafe {
            match saved_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match saved_cargo {
                Some(v) => std::env::set_var("CARGO_HOME", v),
                None => std::env::remove_var("CARGO_HOME"),
            }
        }
    }

    /// #1267：npx MCP 服务器在首次运行时写入 ~/.npm；seatbelt 必须
    /// 允许写入 npm 缓存目录。策略文本和参数表必须同步——
    /// 只发出一个而缺少另一个，sandbox-exec 会拒绝加载配置文件。
    #[test]
    fn test_npm_cache_paths_emitted_in_policy_and_params_when_home_set() {
        let _guard = crate::test_support::lock_test_env();

        let saved_home = std::env::var_os("HOME");
        let saved_npm = std::env::var_os("NPM_CONFIG_CACHE");
        // 安全性：HOME/NPM_CONFIG_CACHE 是进程全局的；lock_test_env
        // 序列化此处的修改，我们总是恢复之前的值。
        unsafe {
            std::env::set_var("HOME", "/tmp/seatbelt-npm-test");
            std::env::remove_var("NPM_CONFIG_CACHE");
        }

        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");

        let policy_text = generate_policy(&policy, cwd);
        assert!(
            policy_text.contains(r#"(allow file-read* (subpath (param "NPM_CACHE_DIR")))"#),
            "npm cache read rule missing from policy"
        );
        assert!(
            policy_text.contains(r#"(allow file-write* (subpath (param "NPM_CACHE_DIR")))"#),
            "npm cache write rule missing from default policy"
        );

        let params = generate_params(&policy, cwd);
        assert!(
            params.iter().any(|(k, _)| k == "NPM_CACHE_DIR"),
            "NPM_CACHE_DIR param missing"
        );

        // ReadOnly 策略：读取访问允许，写入访问必须不存在。
        let read_only_text = generate_policy(&SandboxPolicy::ReadOnly, cwd);
        assert!(
            read_only_text.contains(r#"(allow file-read* (subpath (param "NPM_CACHE_DIR")))"#),
            "read-only mode should allow reading the npm cache"
        );
        assert!(
            !read_only_text.contains(r#"(allow file-write* (subpath (param "NPM_CACHE_DIR")))"#),
            "read-only mode must NOT grant write access to the npm cache"
        );

        // 恢复。
        // 安全性：恢复测试在入口处保存的先前值。
        unsafe {
            match saved_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match saved_npm {
                Some(v) => std::env::set_var("NPM_CONFIG_CACHE", v),
                None => std::env::remove_var("NPM_CONFIG_CACHE"),
            }
        }
    }

    /// #1267：如果既未设置 `NPM_CONFIG_CACHE` 也未设置 `HOME`，则 npm 行
    /// 及其参数都必须省略。
    #[test]
    fn test_npm_cache_skipped_when_no_env() {
        let _guard = crate::test_support::lock_test_env();

        let saved_home = std::env::var_os("HOME");
        let saved_npm = std::env::var_os("NPM_CONFIG_CACHE");
        // 安全性：HOME/NPM_CONFIG_CACHE 是进程全局的；lock_test_env
        // 序列化此处的修改，我们在返回前恢复之前的值。
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("NPM_CONFIG_CACHE");
        }

        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");
        let policy_text = generate_policy(&policy, cwd);
        let params = generate_params(&policy, cwd);

        assert!(!policy_text.contains("NPM_CACHE_DIR"));
        assert!(!params.iter().any(|(k, _)| k == "NPM_CACHE_DIR"));

        // 恢复。
        // 安全性：恢复测试在入口处保存的先前值。
        unsafe {
            match saved_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match saved_npm {
                Some(v) => std::env::set_var("NPM_CONFIG_CACHE", v),
                None => std::env::remove_var("NPM_CONFIG_CACHE"),
            }
        }
    }

    #[test]
    fn test_generate_policy_allows_dev_tty() {
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");
        let policy_text = generate_policy(&policy, cwd);

        assert!(
            policy_text
                .contains(r#"(allow file-read* file-write* file-ioctl (literal "/dev/tty"))"#),
            "TTY-mode shells need /dev/tty access for sshpass/sudo prompts"
        );
    }

    #[test]
    fn test_create_seatbelt_args() {
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");
        let command = vec!["echo".to_string(), "hello".to_string()];

        let args = create_seatbelt_args(command, &policy, cwd);

        // 应以 -p 和策略开头
        assert_eq!(args[0], "-p");
        assert!(args[1].contains("(version 1)"));

        // 应包含分隔符
        assert!(args.contains(&"--".to_string()));

        // 应以原始命令结尾
        assert!(args.contains(&"echo".to_string()));
        assert!(args.contains(&"hello".to_string()));
    }

    #[test]
    fn test_detect_denial() {
        assert!(detect_denial(1, "Operation not permitted"));
        assert!(detect_denial(1, "Sandbox: ls denied file-write*"));
        assert!(!detect_denial(0, "Operation not permitted"));
        assert!(!detect_denial(1, "File not found"));
    }
}
