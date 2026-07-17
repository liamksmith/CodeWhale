# 安装 CodeWhale

本页面涵盖每种受支持的安装路径以及最常见的"安装失败"情况，包括 **Linux ARM64** 和其他不太常见的平台。

如果你只想要简短版本，请参见
[主 README](../README.md#install) 或
[简体中文 README](../README.zh-CN.md#安装)。

在 macOS 和 Linux 上，网站安装器是最短的安装/更新路径：

```bash
curl -fsSL https://codewhale.net/install.sh | sh
```

它会下载匹配的 `codewhale`、`codew` 和 `codewhale-tui` 发布二进制文件，根据 `codewhale-artifacts-sha256.txt` 进行验证，默认安装到 `~/.local/bin`，并暴露 `codew` 便捷命令。

---

## 1. 支持的平台

CodeWhale 为以下平台/架构组合提供匹配的 `codewhale`、`codew` 和 `codewhale-tui` 预构建二进制文件。Linux ARM64 从 v0.8.8 起可用。Linux RISC-V 预构建文件暂时暂停，因为锁定的 `rquickjs-sys` 依赖项未提供 `riscv64gc-unknown-linux-gnu` 绑定。

| 平台         | 架构 | npm 安装 | `cargo install` | GitHub 发布资源                                  |
| ------------ | ------------ | :---------: | :-------------: | ----------------------------------------------------- |
| Linux        | x64 (x86_64) |     ✅      |       ✅        | `codewhale-linux-x64`、`codew-linux-x64`、`codewhale-tui-linux-x64`        |
| Linux        | arm64        |     ✅      |       ✅        | `codewhale-linux-arm64`、`codew-linux-arm64`、`codewhale-tui-linux-arm64`    |
| Android / Termux | arm64 (aarch64) | ❌¹ | ✅² | 发布时提供 `codewhale-android-arm64.tar.gz` Termux 归档文件 |
| Linux        | riscv64      |     ❌¹     |       ❌³       | 在上游绑定就绪之前暂时不支持 |
| macOS        | x64          |     ✅      |       ✅        | `codewhale-macos-x64`、`codew-macos-x64`、`codewhale-tui-macos-x64`        |
| macOS        | arm64（M 系列）| ✅      |       ✅        | `codewhale-macos-arm64`、`codew-macos-arm64`、`codewhale-tui-macos-arm64`    |
| Windows      | x64          |     ✅      |       ✅        | `codewhale-windows-x64.exe`、`codew-windows-x64.exe`、`codewhale-tui-windows-x64.exe` |
| Linux x64 on musl (Alpine) | ✅（静态）|    ✅      |       ✅        | 静态 `codewhale-tui-linux-x64`（musl）资源           |
| 其他 Linux（musl 非 x64、其他架构）| — | ❌¹ | ✅² | 从源代码构建                                     |
| FreeBSD / OpenBSD              | — |   ❌      |       ✅²       | 从源代码构建                                     |

¹ npm 包会以明确的错误退出并引导你到此页面。
² 前提是你的工具链可以编译最近的 Rust 工作区；参见下方的[从源代码构建](#7-从源代码构建)。
³ RISC-V 源代码构建目前需要上游 `rquickjs-sys` RISC-V 绑定或启用 bindgen 的依赖项构建。

Android / Termux 与 Linux arm64 不是相同的目标。不要在 Termux 中安装 GNU libc 的 `codewhale-linux-arm64` 归档文件；当发布或发布候选版本提供时，使用 Termux 专用的 Android 归档文件，或在 Termux 中从源代码构建。

Linux **x64** 发布资源自 v0.8.65 起已是**静态（musl）构建**。它们没有 glibc 依赖，可以在任何 x86_64 Linux 上运行，包括 Ubuntu 22.04、Debian stable、RHEL/CentOS 和 Alpine/musl。SQLite 通过 `rusqlite` 捆绑到二进制文件中，因此不需要单独的 `libsqlite3` 运行时包。

Linux **arm64** 发布资源仍然是 GNU libc（glibc）构建。它们动态链接正常的 Linux 运行时库，如 `libdbus-1` 和 `libc`，并且在 Ubuntu 24.04 上构建，因此可能需要 `GLIBC_2.39`。

### Linux glibc 下限（arm64）

此下限仅适用于 **GNU libc** arm64 资源。静态 x64（musl）资源没有 `GLIBC_*` 符号，因此可以通过安装预检并在旧系统上无错误运行。在当前 v0.8.67 发布通道中，GNU 资源在 Ubuntu 24.04 上构建，可能需要 `GLIBC_2.39`。Ubuntu 22.04 自带 glibc 2.35，因此这些 arm64 二进制文件会失败并显示如下错误：

```text
version `GLIBC_2.39' not found
```

npm 包装器、`codewhale update` 以及 Unix 归档安装器预检会在安装前检查 Linux GNU 二进制文件，并引导旧系统使用 Cargo/源代码构建。如果你在 Ubuntu 22.04 arm64、Debian stable、RHEL/CentOS 或非 x64 资源的其他较旧的 GNU 基础上，请使用：

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

未来的发布工程可能会添加静态（musl）arm64 资源，从而完全消除 glibc 下限；在此之前，x64 是静态的，使用旧发行版的 arm64 用户应从源代码构建。

> **Linux ARM64 说明（v0.8.7 及更早版本）。** v0.8.7 及更早版本**不**发布 Linux ARM64 预构建文件；使用 HarmonyOS 轻薄本、Asahi Linux、Raspberry Pi、AWS Graviton 等的用户会从 `npm i -g codewhale` 看到 `Unsupported architecture: arm64`。v0.8.8 同时发布 `codewhale-linux-arm64` 和 `codewhale-tui-linux-arm64`，因此在任何基于 glibc 的 ARM64 Linux 上普通的 `npm i -g codewhale` 都可以正常工作。如果你停留在 v0.8.7，请跳转到[从源代码构建](#7-从源代码构建)——`cargo install` 可以正常工作。有关 HarmonyOS PC 和 OpenHarmony 交叉构建设置，请参见 [HarmonyOS 和 OpenHarmony](HarmonyOS.md)。

### Android / Termux arm64

Termux 在 Android 的 Bionic libc 上运行，并使用 `$PREFIX` 作为其 Unix 前缀，因此需要 Termux 专用的 Android arm64 归档文件。Linux arm64 发布资源是为普通 Linux 发行版构建的 GNU libc 构建，不应在 Android 上使用。

首先安装最小的归档/运行时工具：

```bash
pkg update
pkg install -y ca-certificates curl tar gzip coreutils
```

当发布包含 `codewhale-android-arm64.tar.gz` 时，使用归档文件附带的安装器进行安装。传递 `PREFIX="$PREFIX"` 很重要：安装器默认使用 `~/.local`，而 Termux 用户通常期望命令在 `$PREFIX/bin` 下。

```bash
cd "$HOME"
curl -L -O https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-android-arm64.tar.gz
curl -L -O https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-bundles-sha256.txt
sha256sum -c codewhale-bundles-sha256.txt --ignore-missing

tar xzf codewhale-android-arm64.tar.gz
cd codewhale-android-arm64
PREFIX="$PREFIX" ./install.sh
hash -r
```

如果你从源代码进行验证或在本地构建发布候选版本，在运行 Cargo 之前安装构建包：

```bash
pkg install -y rust clang pkg-config make git
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

首次运行设置与其他平台相同。建议使用 `codewhale auth set` 或提供商环境变量；不要假设 Termux 内存在桌面 Secret Service 密钥环。

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
```

维护者应使用以下可重复的冒烟清单来验证 Termux / Android arm64 发布候选版本：

```bash
command -v codewhale codew codewhale-tui
test -x "$PREFIX/bin/codewhale"
test -x "$PREFIX/bin/codew"
test -x "$PREFIX/bin/codewhale-tui"

codewhale --version
codewhale doctor
codewhale exec --auto "run pwd"
codewhale-tui --version
```

已知限制：

- 沙箱行为必须在设备上验证。Android 内核和 Termux 打包可能不暴露与桌面/服务器 Linux 文档中相同的 Landlock、seccomp 或 Bubblewrap 行为。
- OS 密钥环行为是尽力而为的。如果 Termux 无法提供可用的密钥存储，请使用 `codewhale auth status` 确认实际来源，并回退到提供商环境变量或配置支持的认证。
- 终端渲染因 Android 终端应用而异。TUI 始终拥有备用屏幕；`--no-alt-screen` 仅作为已弃用的兼容性空操作接受。如果终端应用无法渲染全屏 TUI，请改用 `codewhale exec` 进行无头运行。

---

## 2. 下载安全性和校验和

官方发布二进制文件仅从 `https://github.com/Hmbown/CodeWhale/releases` 和名为 `codewhale` 的 npm 包发布。不要从外观相似的仓库、归档文件或搜索结果镜像安装发布资源，除非你有意信任该镜像。

每个 GitHub 发布都包含校验和清单。使用 `codewhale-artifacts-sha256.txt` 用于裸二进制文件，`codewhale-bundles-sha256.txt` 用于 `.tar.gz` / `.zip` 平台归档文件。如果你手动下载二进制文件，在运行之前进行验证：

```bash
# 从包含已下载二进制文件的目录中运行。
curl -L -O https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-artifacts-sha256.txt
sha256sum -c codewhale-artifacts-sha256.txt --ignore-missing
```

在 macOS 上，使用 `shasum -a 256 -c codewhale-artifacts-sha256.txt` 代替 `sha256sum`。

如果杀毒软件标记了官方发布二进制文件，在确切的工件被识别之前将其视为未解决。请在 GitHub issue 中包含以下所有内容：

- 发布标签，例如 `v0.8.36`
- 确切的下载 URL
- 文件名，例如 `codewhale-linux-x64`
- 你机器上的文件 SHA-256
- 杀毒软件产品名称和检测名称

这使维护者能够区分官方工件的误报与来自冒充仓库或镜像的下载。

---

## 3. 通过 npm 安装

npm 是推荐的安装路径。`codewhale` 包装器发布在 v0.8.68（Node 18+；包装器在 v0.8.56 及更高版本可用）。

```bash
npm install -g codewhale
codewhale --version   # 0.8.68
```

`postinstall` 从匹配的 GitHub 发布下载正确的一对二进制文件，验证 SHA-256 清单，并将 `codewhale`、`codew` 和 `codewhale-tui` 暴露在你的 `PATH` 上。

有用的环境变量：

| 变量                            | 用途                                                                                |
| ----------------------------------- | -------------------------------------------------------------------------------------- |
| `CODEWHALE_VERSION`                 | 固定包装器下载的发布版本（规范）                                                    |
| `DEEPSEEK_TUI_VERSION`              | `CODEWHALE_VERSION` 的旧版别名（默认为 `codewhaleBinaryVersion`）            |
| `DEEPSEEK_TUI_GITHUB_REPO`          | 将下载器指向一个 fork（`owner/repo`）                                          |
| `DEEPSEEK_TUI_RELEASE_BASE_URL`     | 覆盖下载根 URL（例如内部镜像或发布资源代理）            |
| `DEEPSEEK_TUI_FORCE_DOWNLOAD=1`     | 即使缓存的二进制标记匹配也重新下载                                     |
| `DEEPSEEK_TUI_DISABLE_INSTALL=1`    | 完全跳过 `postinstall` 下载（CI 冒烟测试、预置二进制文件）                 |
| `DEEPSEEK_TUI_OPTIONAL_INSTALL=1`   | 不要在下载/解压错误时使 `npm install` 失败 — 在 CI 矩阵中有用            |

> **从中国大陆 npm 下载慢？** 如果 `npm install` 本身很慢（不仅仅是 postinstall 二进制文件下载），请使用 npm 注册表镜像：
> ```bash
> npm config set registry https://registry.npmmirror.com
> npm install -g codewhale
> ```
> 如果你偏好 Cargo 而非 npm，另请参见[第 4 节](#4-通过-cargo-安装任何-tier-1-rust-目标)。

---

## 4. 通过 Cargo 安装（任何 Tier-1 Rust 目标）

如果 GitHub 发布速度慢、被阻止，或者你使用的是不受支持的架构，可以直接从 crates.io 安装。两个 crate 都是必需的 — 调度器在运行时委托给 TUI 运行时。

```bash
# 需要 Rust 1.88+（https://rustup.rs）
cargo install codewhale-cli --locked   # 提供 `codewhale` 和 `codew`
cargo install codewhale-tui     --locked   # 提供 `codewhale-tui`
codewhale --version
```

> **Linux：首先安装构建时依赖项。** `cargo install` 从源代码编译，在 Linux 上，`codewhale-tui` crate 链接到 `libdbus-1`（用于 D-Bus secret-service 凭据存储后端）。在运行 `cargo install` 之前安装所需的系统包：
>
> ```bash
> # Debian / Ubuntu
> sudo apt-get install -y build-essential pkg-config libdbus-1-dev
>
> # Fedora / RHEL
> sudo dnf install -y gcc make pkgconf-pkg-config dbus-devel
> ```
>
> 如果你使用 npm 包装器或下载 GitHub Release 二进制文件，则**不需要**这些构建时包 — 预构建的二进制文件只需要运行时库（`libdbus-1`），而该库在大多数桌面 Linux 安装中已存在。

### 中国/镜像友好安装

从中国大陆安装时，同时为 **rustup**（Rust 工具链安装器）和 **Cargo**（包注册表）配置镜像，以避免 TLS 超时和下载失败。

**第 1 步：通过 rustup 镜像安装 Rust**

```bash
# PowerShell
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
(New-Object Net.WebClient).DownloadFile('https://win.rustup.rs/x86_64', 'rustup-init.exe')

# git-bash / msys2
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
./rustup-init.exe -y --default-toolchain stable

# Linux / macOS
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
```

如果 TUNA 镜像在你的网络上速度慢，`rsproxy.cn` 是 Linux/macOS 的另一个 rustup 镜像选项：

```bash
export RUSTUP_DIST_SERVER=https://rsproxy.cn
export RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
```

`RUSTUP_DIST_SERVER` 和 `RUSTUP_UPDATE_ROOT` 环境变量必须在运行 rustup-init **之前**设置；否则工具链下载会遇到与安装器相同的 TLS 握手问题。

**第 2 步：配置 Cargo 注册表镜像**

```toml
# ~/.cargo/config.toml
[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"
```

`rsproxy`、腾讯 COS 和阿里云 OSS 镜像的工作方式相同；选择你网络上最快的那个。

## 5. 通过 Nix 安装

**试用**

如果你已经安装了支持 flake 的 Nix，运行：

```sh
nix run github:Hmbown/CodeWhale
```

Nix 构建 `codewhale-tui`，然后启动 `codewhale` 调度器。在 `--` 之后传递参数，例如：

```sh
nix run github:Hmbown/CodeWhale -- --help
```

### Flake

将输入添加到 `flake.nix`：

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    codewhale-tui.url = "github:Hmbown/CodeWhale";
    codewhale-tui.inputs.nixpkgs.follows = "nixpkgs";
  };
}
```

安装到 NixOS 模块中：

```nix
{
  outputs = { self, nixpkgs, codewhale-tui }:
  let
    # 将 system "x86_64-linux" 替换为你的系统
    system = "x86_64-linux";
  in
  {
    # 将 `yourhostname` 更改为你的实际主机名
    nixosConfigurations.yourhostname = nixpkgs.lib.nixosSystem {
      inherit system;
      modules = [
        # ...
        {
          environment.systemPackages = [ codewhale-tui.packages.${system}.default ];
        }
      ];
    };
  };
}
```

---

## Homebrew（旧版 tap）

Homebrew 目前仅提供旧版 `deepseek-tui` tap，在 formula 重命名为 `codewhale` 之前保持兼容。它安装相同的当前发布二进制文件：

```bash
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

使用 `brew upgrade deepseek-tui` 更新。目前还没有 `codewhale` formula；一旦重命名完成，本节将切换到它。

---

## 6. 从 GitHub Releases 手动下载

每个平台在 Releases 页面上以**两种形式**出现（这是有意为之 — 参见 #3208）：**裸二进制文件**（`codewhale-<platform>`、`codew-<platform>` 和 `codewhale-tui-<platform>`，无扩展名）和一个 **`.tar.gz` / `.zip` 归档文件**（`codewhale-<platform>.tar.gz`），后者包含相同的命令加上一个 `install.sh`。npm 包装器和应用内 `codewhale update` 下载匹配的运行时二进制文件；归档文件是最简单的手动安装方式（参见 §5）。以下步骤直接使用裸二进制文件。

从 [Releases 页面](https://github.com/Hmbown/CodeWhale/releases) 获取适合你平台的匹配命令集，并将它们并排放置在 `PATH` 上的目录中（例如 `~/.local/bin`）：

```bash
# Linux ARM64 示例
mkdir -p ~/.local/bin
curl -L -o ~/.local/bin/codewhale      \
    https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-linux-arm64
curl -L -o ~/.local/bin/codew          \
    https://github.com/Hmbown/CodeWhale/releases/latest/download/codew-linux-arm64
curl -L -o ~/.local/bin/codewhale-tui  \
    https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-tui-linux-arm64
chmod +x ~/.local/bin/codewhale ~/.local/bin/codew ~/.local/bin/codewhale-tui
codewhale --version
```

> **macOS Gatekeeper 说明。** 如果你用浏览器下载了二进制文件，macOS 可能会以"Apple 无法验证"的警告阻止它们。清除所有三个二进制文件的隔离属性并重试：
> ```bash
> xattr -d com.apple.quarantine ~/.local/bin/codewhale ~/.local/bin/codew ~/.local/bin/codewhale-tui 2>/dev/null || true
> ```

根据每次发布的 SHA-256 清单验证完整性：

```bash
curl -L -o /tmp/codewhale-artifacts-sha256.txt \
    https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-artifacts-sha256.txt
( cd ~/.local/bin && sha256sum -c /tmp/codewhale-artifacts-sha256.txt --ignore-missing )
```

（在 macOS 上使用 `shasum -a 256 -c` 代替 `sha256sum`。）

### 回滚到之前的发布版本

如果新版本在你的机器上有问题，显式安装上一个已知良好的版本。将 `X.Y.Z` 替换为你要恢复的版本。

```bash
# npm 包装器，仅适用于已发布到 npm 的版本
npm install -g codewhale@X.Y.Z

# Cargo 安装路径；两个 crate 都是必需的
cargo install codewhale-cli --version X.Y.Z --locked --force
cargo install codewhale-tui --version X.Y.Z --locked --force
```

对于手动安装，从确切的发布标签下载匹配的二进制文件或平台归档文件，并验证来自同一标签的匹配校验和清单：

```bash
# 单独二进制文件
curl -L -o codewhale-artifacts-sha256.txt \
  https://github.com/Hmbown/CodeWhale/releases/download/vX.Y.Z/codewhale-artifacts-sha256.txt

# 平台归档文件
curl -L -o codewhale-bundles-sha256.txt \
  https://github.com/Hmbown/CodeWhale/releases/download/vX.Y.Z/codewhale-bundles-sha256.txt
```

在 CodeWhale 工作区内，`/restore list [N]` 列出侧 git 文件快照，`/restore <N>` 从所选快照恢复文件。该工作区回滚不会更改你安装的二进制版本，也不会重写对话历史。

### Windows Scoop

`codewhale` 包列在 Scoop 的主桶中：

```powershell
scoop update
scoop install codewhale
codewhale --version
```

Scoop 清单在此仓库的发布工作流之外维护，可能落后于 GitHub/npm/Cargo 发布。当你需要立即获得最新版本时，请使用 npm 或手动 GitHub 发布下载。

### Windows NSIS 安装器

从 v0.8.50 开始，为喜欢传统双击安装的 Windows 用户提供了独立的基于 NSIS 的安装器（不需要 npm、Scoop 或 Cargo）。

**下载** `CodeWhaleSetup.exe`，来自 [Releases 页面](https://github.com/Hmbown/CodeWhale/releases/latest)。

**安装**：双击安装可执行文件。安装器会：

- 将 `codewhale.exe`、`codew.exe` 和 `codewhale-tui.exe` 并排安装到 `%LOCALAPPDATA%\Programs\CodeWhale\bin`
- 将安装目录添加到**当前用户**的 `PATH`
- 在 Windows **应用和功能**中注册以便于卸载

**静默安装**（适用于 IT 管理员、SCCM、Intune）：

```powershell
CodeWhaleSetup.exe /S
```

安装器是每用户安装，不需要提权。在目标用户上下文中运行静默安装，或使用可以为每个需要 CodeWhale 的用户配置文件运行安装器的部署工具。

发布构建的安装器目前未签名，可能触发 Windows SmartScreen。在部署前从 `codewhale-artifacts-sha256.txt` 验证 SHA-256 校验和，如果你的环境需要签名的应用程序包，请在内部部署管道中对安装器进行签名。

**自行构建安装器**（需要 [NSIS](https://nsis.sourceforge.io)）：

```powershell
cd scripts\installer
# 将 codewhale.exe 和 codewhale-tui.exe 放在这里，然后：
makensis /DVERSION=<version> codewhale.nsi
```

**手动回退** — 如果安装器被组策略阻止，请参见 [CLASSROOM_INSTALL.md](CLASSROOM_INSTALL.md) 指南了解逐步的 PowerShell 命令。

> **部署到教室或实验室？** 请参见完整的[教室安装清单](CLASSROOM_INSTALL.md)了解静默安装、API 密钥配置、镜像说明和故障排除。

---

## 7. 从源代码构建

这是针对我们不提供发布版本的平台的通用方式，包括 musl 非 x64、LoongArch、FreeBSD 和 2024 年之前的 ARM64 发行版。Linux RISC-V 目前也需要上游 `rquickjs-sys` RISC-V 绑定或启用 bindgen 的依赖项构建，然后源代码构建才能正常工作。

### 先决条件

- **Rust** 1.88 或更高版本 — 使用 [rustup](https://rustup.rs) 安装。
- **Linux 构建时依赖项**（Debian/Ubuntu/openEuler/Kylin）：
  ```bash
  sudo apt-get install -y build-essential pkg-config libdbus-1-dev
  # openEuler / RHEL 系列：
  # sudo dnf install -y gcc make pkgconf-pkg-config dbus-devel
  ```
- 不需要 `cmake`。

### 构建和安装

```bash
git clone https://github.com/Hmbown/CodeWhale.git
cd CodeWhale

cargo install --path crates/cli --locked   # 提供 `codewhale` 和 `codew`
cargo install --path crates/tui --locked   # 提供 `codewhale-tui`

codewhale --version
```

这些命令默认安装到 `~/.cargo/bin/`；确保该目录在你的 `PATH` 上。

### 从 x64 交叉编译到 ARM64 Linux

如果你想在 x64 Linux 主机上构建 ARM64 Linux 二进制文件（例如用于 HarmonyOS / openEuler ARM64 轻薄本），使用 [`cross`](https://github.com/cross-rs/cross)，它将官方 Rust 交叉目标包装在 Docker 容器中：

```bash
# 一次性操作
rustup target add aarch64-unknown-linux-gnu
cargo install cross --locked

# 每次构建
cross build --release --target aarch64-unknown-linux-gnu -p codewhale-cli
cross build --release --target aarch64-unknown-linux-gnu -p codewhale-tui
```

生成的二进制文件位于 `target/aarch64-unknown-linux-gnu/release/codewhale` 和 `target/aarch64-unknown-linux-gnu/release/codewhale-tui`。将匹配的一对复制到 ARM64 主机（例如通过 `scp`）并 `chmod +x` 它们。

如果你没有 Docker，可以直接安装交叉链接器并让 Cargo 完成工作：

```bash
sudo apt-get install -y gcc-aarch64-linux-gnu
rustup target add aarch64-unknown-linux-gnu

cat >> ~/.cargo/config.toml <<'EOF'
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
EOF

cargo build --release --target aarch64-unknown-linux-gnu -p codewhale-cli
cargo build --release --target aarch64-unknown-linux-gnu -p codewhale-tui
```

如果你的发行版基于 musl，同样的方法也适用于 `aarch64-unknown-linux-musl`。

### Windows 从源代码构建

在 Windows 上构建需要来自 [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022)（免费的工作负载可选安装器，而非完整 IDE）的 **MSVC C 工具链**。

**先决条件（Windows）**

1. 安装 Visual Studio 2022 Build Tools — 选择 **"使用 C++ 进行桌面开发"** 工作负载。
2. 安装 [Rust](https://rustup.rs) 1.88+（如果从中国大陆下载，请参见上方的[中国镜像说明](#中国镜像友好安装)）。
3. 安装 [Git for Windows](https://git-scm.com/download/win)（提供 `git` 和 `git-bash` 终端）。

**推荐终端**：Windows Terminal、`git-bash` 或 PowerShell。`cmd.exe` 可以工作，但缓冲区小且 PATH 行为有限。

**设置 MSVC 环境**

Visual Studio Build Tools 将 `cl.exe` 安装到版本化目录，但**不**将其添加到全局 `PATH`。你必须手动设置环境或使用开发人员命令提示符。所需变量为：

```powershell
# 调整版本号以匹配你的安装
$msvc = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207"
$sdk   = "C:\Program Files (x86)\Windows Kits\10"
$sdkv  = "10.0.26100.0"

$env:INCLUDE  = "$msvc\include;$msvc\atlmfc\include;$sdk\Include\$sdkv\ucrt;$sdk\Include\$sdkv\um;$sdk\Include\$sdkv\shared"
$env:LIB      = "$msvc\lib\x64;$msvc\atlmfc\lib\x64;$sdk\Lib\$sdkv\ucrt\x64;$sdk\Lib\$sdkv\um\x64"
$env:LIBPATH  = "$msvc\lib\x64;$msvc\atlmfc\lib\x64"
$env:CC       = "$msvc\bin\Hostx64\x64\cl.exe"
$env:CXX      = "$msvc\bin\Hostx64\x64\cl.exe"
$env:PATH     = "$msvc\bin\Hostx64\x64;$env:PATH"
```

或者，打开 **"Developer Command Prompt for VS 2022"**（安装 Build Tools 后从开始菜单可用），它运行 `vcvars64.bat` 来自动配置上述所有内容。然后在该会话中将 `cargo` 添加到 `PATH`，并从项目根目录运行 `cargo build`。

**Cargo 注册表镜像** — 在 Windows 上，镜像配置放到 `%USERPROFILE%\.cargo\config.toml`。参见上方的[第 2 步](#中国镜像友好安装)。

**构建**

```bash
git clone https://github.com/Hmbown/CodeWhale.git
cd CodeWhale
set CARGO_HTTP_CHECK_REVOKE=false   # 在某些中国 ISP 后可能需要
cargo build --release
```

二进制文件出现在 `target\release\codewhale.exe`、`target\release\codew.exe` 和 `target\release\codewhale-tui.exe`。

> 不想构建？通过 npm、Cargo、GitHub Releases 或 CNB 镜像安装 — 参见上面的章节。

---

## 8. 故障排除

### `Unsupported architecture: arm64 on platform linux`

你使用的是早于 v0.8.8 的版本，该版本不发布 Linux ARM64 二进制文件。要么升级（`npm i -g codewhale@latest`），要么按照[第 4 节](#4-通过-cargo-安装任何-tier-1-rust-目标)使用 `cargo install`。

### 运行时 `MISSING_COMPANION_BINARY`

调度器（`codewhale`）需要 TUI 运行时（`codewhale-tui`）在同一个 `PATH` 上。如果你通过 `cargo install` 只安装了一个 crate，请安装两者：

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui     --locked
```

### `codewhale update` 报告 `no asset found for platform codewhale-linux-aarch64`

这是 v0.8.7 中的 [#503](https://github.com/Hmbown/CodeWhale/issues/503) — 自更新器使用了 Rust 的 `aarch64`/`x86_64` 架构名称，而不是发布资源的 `arm64`/`x64`。在 v0.8.8 之前的变通方法：

```bash
npm i -g codewhale@latest
# 或
cargo install codewhale-cli --locked
```

### 从中国大陆 npm 下载慢或超时

将 `CODEWHALE_RELEASE_BASE_URL` 设置为镜像的发布资源目录（rsproxy、TUNA、腾讯 COS、阿里云 OSS），或完全跳过 npm 并使用[第 4 节](#4-通过-cargo-安装任何-tier-1-rust-目标)中的 Cargo 镜像设置。旧版 `DEEPSEEK_TUI_RELEASE_BASE_URL` 名称仍然被接受。

### 从中国大陆 `codewhale update` 被 GitHub 阻止

`codewhale update` 通常会联系 GitHub Releases 获取元数据和二进制资源。在 GitHub 被阻止或不可靠的网络上，改用 CNB 源镜像并从发布标签安装两个二进制文件：

要检查最新发布而不下载或替换二进制文件，运行 `codewhale update --check`。

```bash
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag vX.Y.Z codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag vX.Y.Z codewhale-tui     --locked --force
```

如果你运营二进制资源镜像，`codewhale update` 可以直接使用它：

```bash
CODEWHALE_RELEASE_BASE_URL=https://your-mirror.example.com/CodeWhale/vX.Y.Z/ \
DEEPSEEK_TUI_VERSION=X.Y.Z \
codewhale update
```

镜像目录必须包含来自 GitHub 发布的 `codewhale-artifacts-sha256.txt` 和平台二进制文件。旧版 `DEEPSEEK_TUI_RELEASE_BASE_URL` 镜像变量作为别名保持支持。

### Debian/Ubuntu：`cargo install` 报告 `feature edition2024 is required`

某些 Debian/Ubuntu 发行版包提供的旧版 Cargo 无法解析 Rust 2024 crate。例如，Ubuntu 24.04 上的 Cargo 1.75.0 在构建之前会失败并显示：

```text
feature `edition2024` is required
The package requires the Cargo feature called `edition2024`, but that feature
is not stabilized in this version of Cargo
```

通过 rustup 安装当前稳定版 Rust，然后重新运行[第 4 节](#4-通过-cargo-安装任何-tier-1-rust-目标)中的两个 Cargo 安装命令。对于中国大陆网络，以下基于 rsproxy 的序列已验证可以工作：

```bash
export RUSTUP_DIST_SERVER=https://rsproxy.cn
export RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup default stable
cargo install codewhale-cli --locked
cargo install codewhale-tui     --locked
```

之后，`which cargo` 应该指向 `~/.cargo/bin/cargo`，而不是 `/usr/bin/cargo`。

### Debian/Ubuntu：构建时 `error: linker 'cc' not found`

安装 C 工具链：

```bash
sudo apt-get install -y build-essential pkg-config libdbus-1-dev
```

### WSL2 / Ubuntu：构建时找不到 `dbus-1` 或 `pkg-config`

WSL2 使用与 Ubuntu 相同的 Linux 源代码构建路径。如果 `cargo install codewhale-tui --locked` 在编译密钥环或 D-Bus 密钥存储 crate 时失败，在 WSL 发行版中安装 Linux 构建依赖项，然后重新运行两个 Cargo 安装命令：

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libdbus-1-dev
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

预构建的 npm/GitHub 二进制文件不需要这些构建时包；它们仅适用于 WSL2 从源代码编译 CodeWhale 时。

### 包装器安装成功但找不到 `codewhale`

`npm i -g` 安装到 `$(npm prefix -g)/bin`；确保该目录在你的 shell 的 `PATH` 上。使用 nvm：`nvm use --lts && hash -r`。

### Windows：`rustup-init` 出现 `TLS handshake eof` 或 `CRYPT_E_REVOCATION_OFFLINE`

从 GFW 或某些中国 ISP 后连接到 `static.rust-lang.org` 的 TLS 握手失败。在运行安装器**之前**设置 rustup 镜像环境变量：

```bash
# git-bash / msys2
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
./rustup-init.exe -y --default-toolchain stable
```

如果在 Rust 安装后从 Cargo 看到 `CRYPT_E_REVOCATION_OFFLINE`，在 `cargo build` 期间也设置 `CARGO_HTTP_CHECK_REVOKE=false`。

### Windows：`cargo build` 期间找不到 MSVC 编译器（`cl.exe`）

Visual Studio Build Tools 不会将 `cl.exe` 添加到全局 `PATH`。你可以：

1. 从开始菜单打开 **"Developer Command Prompt for VS 2022"**，在该窗口中添加 `%USERPROFILE%\.cargo\bin` 到 `PATH`，并从那里运行 `cargo build`；或
2. 手动设置 MSVC 环境变量 — 参见 [Windows 从源代码构建](#windows-从源代码构建) 部分中的 PowerShell 代码片段。

验证编译器可访问：`cl.exe /?` 应该打印帮助文本。

### Windows：Cargo 执行构建脚本时出现 `拒绝访问 (os error 5)`

第三方杀毒软件（火绒、360、Kaspersky 等）可能会阻止 Cargo 执行新编译的构建脚本二进制文件（例如 `libsqlite3-sys`、`aws-lc-sys`、`instability`）。该错误与路径无关 — 移动 `target-dir` 没有帮助。

**症状**：`could not execute process ... build-script-build (never executed)`

**变通方法**（选择一项）：

1. **将项目的 `target/` 目录添加到你的 AV 排除列表中。**
2. **在 `cargo build` 期间临时关闭杀毒软件。**
3. **改用 GitHub Release 安装器/归档文件** — 发布资源提供预构建二进制文件，完全跳过 Cargo 构建（[第 6 节](#6-从-github-releases-手动下载)）。
4. **使用 `cargo install codewhale-cli --locked`** 从 crates.io 安装 — 这会改变二进制路径，某些 AV 工具会对此区别对待。

要验证构建脚本二进制文件本身是有效的（未损坏），在 `target/debug/build/<crate>/build-script-build` 下找到它并手动运行：

```bash
target/debug/build/libsqlite3-sys-*/build-script-build
# 如果运行但 panic 并显示 "NotPresent"（没有 C 编译器），则二进制文件没问题 —
# AV 专门阻止了 Cargo 的进程生成路径。
```

### npm 二进制下载超时

如果 `codewhale` 等待数秒并从 `github.com` 获取时打印 `connect ETIMEDOUT` 或 `EAI_AGAIN`，说明 npm 包装器安装成功，但从 GitHub Releases 下载预构建二进制文件在你的网络上被阻止或不可靠。此下载与 npm 注册表包下载是分开的。

使用以下路径之一：

1. 设置代理并重试：

   ```bash
   export HTTPS_PROXY=http://your-proxy:port
   codewhale
   ```

2. 内部镜像发布资源并设置 `DEEPSEEK_TUI_RELEASE_BASE_URL`：

   ```bash
   export DEEPSEEK_TUI_RELEASE_BASE_URL=https://your-mirror.example.com/CodeWhale/
   codewhale
   ```

   目录必须包含来自 GitHub 发布的 `codewhale-artifacts-sha256.txt` 和平台二进制文件。

3. 通过 Cargo 安装，它在本地构建，不下载 GitHub 发布资源。参见[第 4 节](#4-通过-cargo-安装任何-tier-1-rust-目标)。

4. 从 [Releases 页面](https://github.com/Hmbown/CodeWhale/releases) 手动下载 `codewhale` 和 `codewhale-tui`，将它们放在 `PATH` 上的目录中，并使其可执行。参见[第 6 节](#6-从-github-releases-手动下载)。

---

## 9. 验证你的安装

```bash
codewhale --version
codewhale doctor       # 检查 API 密钥、提供商、运行时和 PATH 完整性
codewhale doctor --json
```

`doctor` 如果发现问题会以非零状态退出，并打印结构化的修复提示。如果需要帮助，将 JSON 输出粘贴到 GitHub issue 中。
