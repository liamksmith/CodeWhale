# 沙箱威胁模型

CodeWhale 执行由 AI 推理生成的 shell 命令。沙箱模块限制这些命令对主机系统可以执行的操作。本文档描述每个平台沙箱实际强制执行的内容、哪些是尽力而为的以及哪些明确不在范围内。

## 平台概述

| 机制 | 平台 | 类型 | 状态 |
|---|---|---|---|
| Seatbelt | macOS | 强制访问控制 | 已强制执行 |
| Landlock | Linux | 文件系统访问控制 | 已强制执行 |
| seccomp BPF | Linux | 系统调用过滤器 | 已强制执行 |
| 进程加固 | Linux | 内核 prctl / rlimit | 已强制执行 |
| Bubblewrap (bwrap) | Linux | 命名空间隔离 | 可选 |
| Windows Job Object | Windows | 进程树限制 | v1 (PR #2220) |

## 威胁模型：每一层应对什么

### 1. 进程加固（仅 Linux）

**运行时机：** 在任何线程生成之前、Tokio 启动之前、任何数据加载到内存之前。

**它做什么：**

- `PR_SET_DUMPABLE=0` — 阻止 ptrace，使 `/proc/<pid>/` 归 root 所有
- `PR_SET_NO_NEW_PRIVS=1` — 不可逆；子进程永远不能提升权限
- `RLIMIT_CORE=0` — 没有核心转储，因此敏感数据永远不会写入磁盘

**它防范什么：**
- 通过 ptrace/strace/gdb 进行进程检查
- 通过 setuid/setgid/fscaps 进行权限提升
- 核心转储泄露 API 密钥、令牌、提示内容

**它不防范什么：**
- 被入侵的子进程读取其父进程的 `/proc/<pid>/mem`（已被 `PR_SET_DUMPABLE=0` 阻止，使 `/proc/<pid>/` 归 root 所有）
- 绕过 prctl 的内核漏洞利用

### 2. Landlock（Linux，内核 5.13+）

**运行时机：** 在生成时通过辅助脚本或 `landlock_restrict_self` 应用于每个子进程。只能由进程自身限制——父进程不能强制对子进程施加 Landlock。

**它做什么：**
- 将文件系统访问限制到路径白名单
- 句柄：`EXECUTE`、`READ_FILE`、`READ_DIR`、`WRITE_FILE`、`REMOVE_DIR`、`REMOVE_FILE`、`MAKE_DIR`、`MAKE_REG`、`MAKE_SYM`、`TRUNCATE`

**它防范什么：**
- 读取工作区外的文件（例如 `/etc/passwd`、`~/.ssh`）
- 写入系统目录（`/usr`、`/bin`、`/lib`）
- 在受保护位置创建或删除文件

**它不防范什么：**
- 网络访问（Landlock 仅文件系统）
- 进程检查（对此使用 seccomp）
- 读取已映射的文件（Landlock 在 `open()` 时生效）

**检测：** `detect_denial()` 检查 stderr 中的 `Permission denied`、`Operation not permitted`、`EACCES`、`EPERM`。

### 3. seccomp BPF（仅 Linux）

**运行时机：** 通过子进程上的 `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER)` 安装。

**它做什么：**
- 约 100 个安全系统调用的白名单（文件 I/O、内存、进程、IPC、同步、信号、时间）
- **明确拒绝：** `ptrace`、`mount`、`umount2`、`kexec_load`、`kexec_file_load`、`init_module`、`finit_module`、`delete_module`、`bpf`、`reboot`、`swapon`、`swapoff`、`pivot_root`、`setuid`/`setgid`/`setreuid`/`setregid`/`setresuid`/`setresgid`、`personality`
- 任何不在白名单上的系统调用 → `SECCOMP_RET_KILL_PROCESS`（SIGSYS）

**它防范什么：**
- 通过 ptrace 劫持进程
- 挂载文件系统（绕过 Landlock 只读限制）
- 加载内核模块
- 加载 BPF 程序（会绕过 seccomp 自身！）
- 重启系统
- 通过 setuid/setgid 更改权限

**它不防范什么：**
- 合法使用允许的系统调用进行恶意目的
- 通过允许的系统调用进行侧信道攻击（例如计时）

**检测：** `detect_denial()` 检查退出码 31（SIGSYS）或 stderr 中的 `Bad system call`、`bad system call`、`SIGSYS`、`seccomp`。

### 4. Bubblewrap / bwrap（Linux，可选）

**运行时机：** 如果 `/usr/bin/bwrap` 存在且配置键 `[sandbox] prefer_bwrap = true` 被设置。作为子命令的外部包装器运行。

**它做什么：**
- 使用 `--unshare-all` 创建新的挂载命名空间
- 以只读方式绑定挂载整个根文件系统
- 以读写方式绑定挂载工作区目录
- 使用 `--chdir` 进入工作区

**它防范什么：**
- 工作区外的任何文件系统写入（比单独的 Landlock 更强，因为它在命名空间级别强制执行，而不仅仅是文件系统访问）
- 意外修改系统文件

**它不防范什么：**
- 网络访问（bwrap 默认情况下不使用 `--unshare-all` 创建网络命名空间；子进程仍然有完整的网络访问）
- 进程检查
- 内存攻击

**安装：** 用户必须自己安装 bubblewrap：
- Ubuntu/Debian：`apt install bubblewrap`
- Fedora：`dnf install bubblewrap`
- Arch：`pacman -S bubblewrap`

CodeWhale 不捆绑 bwrap。

**回退：** 如果未安装 bwrap，沙箱回退到仅 Landlock。

### 5. Seatbelt（macOS）

**运行时机：** 通过 `sandbox-exec` 包装命令应用。seatbelt 配置文件基于 `SandboxPolicy` 动态生成。

**它做什么：**
- 基于策略配置文件限制文件系统访问
- 可以限制网络访问（当 `network_access: false` 时）

**它防范什么：**
- 读取/写入允许路径之外的文件
- 网络连接（当配置时）

**它不防范什么：**
- 进程检查（Seatbelt 不阻止 ptrace）
- 系统调用级别的攻击

**检测：** 检查 stderr 中的 `file-write` 和 `network` 拒绝模式。

### 6. Windows Job Object（v1，PR #2220）

**运行时机：** 在进程生成时通过 `PROC_THREAD_ATTRIBUTE_JOB_LIST` 和受限令牌分配应用。

**它做什么（v1）：**
- 带有 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job Object——父进程退出时所有子进程终止
- 内存上限：每个进程 1 GB，每个作业 2 GB
- 活跃进程限制：64
- UI 限制：无桌面句柄访问
- 受限令牌：移除 Administrators 组 SID，设置中低完整性级别

**推迟的功能（v2）：**
- WFP（Windows 过滤平台）防火墙规则——v1 中网络是开放的
- 生成时的文件系统 ACL 集成（推迟）
- AppContainer 隔离
- 注册表键隔离

**检测：** 检查 stderr 中的 `Access is denied`、`STATUS_ACCESS_DENIED`、`ERROR_ACCESS_DENIED`、`ERROR_PRIVILEGE_NOT_HELD`、`ERROR_ACCESS_DISABLED_BY_POLICY` 以及完整性/AppContainer 模式。

## 纵深防御

Linux 沙箱按顺序应用各层：

```
进程加固 (prctl)    ← 在线程之前
    ↓
Landlock (文件系统)  ← 在子进程生成时
    ↓
seccomp BPF (系统调用) ← 在子进程生成时
    ↓
bwrap (命名空间隔离)  ← 可选外部包装器
```

每一层应对不同的威胁面。seccomp 无法保护文件系统（那是 Landlock 的工作）。Landlock 无法阻止 ptrace（那是 seccomp + PR_SET_DUMPABLE 的工作）。bwrap 添加了 Landlock 和 seccomp 都无法提供的命名空间级别隔离。

## 配置

`~/.codewhale/config.toml` 中的相关配置键：

```toml
# 沙箱策略模式
sandbox_mode = "workspace-write"  # read-only | workspace-write | danger-full-access | external-sandbox

# Linux bubblewrap 直通
prefer_bwrap = false              # 需要安装 `bubblewrap` 包

# 外部沙箱后端
sandbox_backend = "none"          # "none" 或 "opensandbox"
sandbox_url = "http://localhost:8080"
sandbox_api_key = "YOUR_API_KEY"
```

环境变量覆盖：

- `DEEPSEEK_SANDBOX_MODE` → `sandbox_mode`
- `DEEPSEEK_PREFER_BWRAP=true` → `prefer_bwrap`
- `DEEPSEEK_SANDBOX_BACKEND` → `sandbox_backend`
- `DEEPSEEK_SANDBOX_URL` → `sandbox_url`
- `DEEPSEEK_SANDBOX_API_KEY` → `sandbox_api_key`

## 检测沙箱拒绝

当命令失败时，沙箱管理器检查拒绝模式：

| 平台 | 拒绝机制 | 退出码 | Stderr 模式 |
|---|---|---|---|
| macOS Seatbelt | sandbox-exec 违规 | 非零 | `file-write`、`network` |
| Linux Landlock | EACCES / EPERM | 非零 | `Permission denied`、`Operation not permitted` |
| Linux seccomp | SIGSYS (31) | 31 或 159 | `Bad system call`、`SIGSYS` |
| Linux bwrap | 挂载/命名空间失败 | 非零 | 各不相同 |
| Windows | 访问拒绝 / 权限 | 非零 | `Access is denied`、`ERROR_PRIVILEGE_NOT_HELD` |

`SandboxManager` 上的 `was_denied()` 方法聚合所有平台特定的检查。`denial_message()` 方法返回人类可读的解释。

## 限制

### 沙箱不防范什么

- **网络攻击** — 只有 macOS Seatbelt 可以阻止网络；Linux 和 Windows v1 保持网络开放
- **内存攻击** — 没有平台阻止子进程读取自己的内存或利用内存损坏漏洞
- **计时侧信道** — Linux 上允许的系统调用可用于基于计时的信息泄露
- **资源耗尽** — Linux job object 限制内存和进程计数，但不限制 CPU、文件描述符或磁盘 I/O
- **内核漏洞** — 如果内核本身存在漏洞，沙箱无法阻止利用（这适用于所有平台）
- **供应链** — 如果子进程下载并执行不受信任的代码，沙箱限制该代码可以做什么，但不能阻止下载

### 平台特定差距

- **Linux：** Landlock 仅保护文件系统访问。seccomp 添加系统调用过滤，但使用可能需要为新的系统调用更新的白名单。
- **macOS：** Seatbelt 配置文件在运行时生成。配置错误的配置文件可能过于宽松。
- **Windows v1：** 生成时没有文件系统 ACL 强制执行。网络完全开放。Job Object 仅是进程树级别的。

## 相关

- `crates/tui/src/sandbox/` — 实现
- `crates/config/src/lib.rs` — 配置键
- `crates/tui/src/tools/diagnostics.rs` — `diagnostics` 工具报告 `sandbox_available`、`sandbox_type`、`bwrap_available`、`cgroup_version`
- `config.example.toml` — 带注释的配置参考
- Issue #2180 — 本文档
- Issue #2182 — seccomp 过滤器实现
- Issue #2183 — 进程加固
- Issue #2184 — bwrap 直通
- Issue #2185 — Windows Job Object v1
- Issue #2186 — SandboxExecutor trait 统一
- Issue #2187 — 沙箱对等测试
