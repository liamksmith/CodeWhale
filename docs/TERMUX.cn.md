# Termux / Android arm64 支持

CodeWhale 通过 [Termux](https://termux.dev) 在 Android arm64 上原生运行。
本文档涵盖安装路径以及您应该了解的特定平台行为差异。

## 安装

请参阅 [`INSTALL.md`](./INSTALL.md) → "Android / Termux arm64" 了解当前的安装步骤。简要版本：

```sh
# 在 Termux 内（pkg install rust git ...）
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

或者，当发布包含 `codewhale-android-arm64.tar.gz` 时，将其解压到 `$PREFIX/bin`。

> **不要**在 Termux 中安装 GNU libc 的 `codewhale-linux-arm64` 归档。
> Android 使用 Bionic libc，而非 glibc — Linux 二进制文件将无法运行。

## Android 上的平台行为

CodeWhale 的安全模型有两个独立的层次：

1. **操作系统文件系统沙箱** — Seatbelt（macOS）、Landlock（Linux）或无。此层限制 *shell 命令* 在内核级别可以访问的内容。
2. **CodeWhale 自身的门禁** — 工作区信任、批准提示、`allow_shell`/`disallowed-tools` 以及文件工具权限系统。这些是应用级别的，在每个平台上行为相同。

### 沙箱：不可用（类型 = none）

Android 不暴露 Landlock、Seatbelt 或任何 CodeWhale 可以使用的等效强制访问控制 API。在 Android 上，`codewhale doctor` 报告 **沙箱类型：none**。

- `get_platform_sandbox()` 在 Android 上返回 `None`。
- 没有 Linux 专用沙箱模块（Landlock、bwrap）编译进 Android 构建——它们被 `#[cfg(target_os = "linux")]` 门控，而 Rust 将 `android` 视为与 `linux` 不同的目标。
- Shell 命令在没有操作系统级别文件系统限制的情况下运行。依赖 CodeWhale 的批准门禁和工作区信任来保证安全。

### 批准：仍然适用

CodeWhale 的批准系统（对风险操作的交互式提示、`allow_shell`、`--disallowed-tools`）完全是应用级别的。它在 Android 上的行为相同——缺乏操作系统沙箱不会削弱它。

### 密钥存储：基于文件的

Android 没有操作系统密钥环（没有 Secret Service / dbus）。CodeWhale 回退到**基于文件的密钥存储**：`~/.codewhale/secrets/`（Termux 主目录）下的明文 JSON 文件，仅受 `0600` 文件权限保护——它们**在静态时未加密**。在单用户 Termux 上，这与 `~/.ssh` 私钥的保护级别相同。

- 通过 `codewhale setup` 或 `/provider` 设置的 API 密钥存储在这些权限保护的文件中；`codewhale auth set` 还会将配置的密钥写入 `config.toml`，因此将这两个文件都视为敏感文件。
- `codewhale doctor` 报告哪个密钥后端处于活动状态。

### 自更新

Android 上的 `codewhale update` 请求 `codewhale-android-arm64` 和 `codewhale-tui-android-arm64` 发布资产——永远不会请求 Linux arm64 资产。GNU libc（glibc）兼容性飞行前检查仅限 Linux，在 Android（Bionic libc）上完全跳过。

## 已知限制（首次 Termux 发布）

| 功能 | 状态 | 备注 |
|---------|--------|-------|
| 操作系统沙箱 | ❌ 不可用 | Android 上没有 Landlock/bwrap/Seatbelt |
| 操作系统密钥环 | ❌ 不可用 | 回退到基于文件的密钥 |
| 批准 / 门禁 | ✅ 完整 | 应用级别，平台无关 |
| 文件工具 | ✅ 完整 | 受工作区信任管理 |
| 自更新 | ✅ 完整 | 选择 Android 资产 |
| Shell 执行 | ⚠️ 无限制 | 在没有操作系统沙箱的情况下运行；依赖批准 |

## 相关问题

- #4236 — Epic：官方 Termux / Android arm64 支持
- #4238 — 明确 Android 沙箱和密钥存储行为
- #4240 — 构建和打包 Android arm64 发布资产
- #4241 — 教导更新器在 Termux 上选择 Android 资产
- #4242 — 运行 Termux 运行时质量保证
