# HarmonyOS 和 OpenHarmony

本页面介绍在 HarmonyOS PC 和 OpenHarmony 交叉构建环境中的 CodeWhale 使用。

## 在 HarmonyOS PC 上运行

当 HarmonyOS PC 的用户空间兼容 glibc 时，可以使用普通的 Linux ARM64 包：

```bash
npm i -g codewhale
codewhale --version
```

你也可以从 GitHub Releases 页面下载 `codewhale-linux-arm64` 和 `codewhale-tui-linux-arm64`，并将两个二进制文件都放到 `PATH` 上。

## 交叉编译到 OpenHarmony

仓库不检入特定于机器的 SDK 路径。设置 `OHOS_NATIVE_SDK` 为 OpenHarmony 原生 SDK 目录，该目录应包含 `llvm/bin`、`sysroot` 和 `build/cmake/ohos.toolchain.cmake`。

在 Windows PowerShell 上：

```powershell
$env:OHOS_NATIVE_SDK="<path-to-openharmony-native-sdk>"
. .\scripts\ohos-env.ps1
rustup target add aarch64-unknown-linux-ohos
cargo build --target aarch64-unknown-linux-ohos -p codewhale-cli
```

在 Linux 或 macOS 上：

```bash
export OHOS_NATIVE_SDK=/path/to/openharmony/native
. ./scripts/ohos-env.sh
rustup target add aarch64-unknown-linux-ohos
cargo build --target aarch64-unknown-linux-ohos -p codewhale-cli
```

设置脚本会导出 Cargo 的目标特定 `linker`、`AR`、`CC`、`CXX`、`CFLAGS`、`CXXFLAGS`、`CARGO_ENCODED_RUSTFLAGS`、`CC_SHELL_ESCAPED_FLAGS` 和 CMake 工具链变量，用于 `aarch64-unknown-linux-ohos`。

## 编译器包装器

对于临时的编译器调用，使用 `scripts/ohos/` 中的包装器。它们读取相同的 `OHOS_NATIVE_SDK` 变量，且不包含本地路径。

Windows PowerShell：

```powershell
.\scripts\ohos\ohos-clang.ps1 --version
.\scripts\ohos\ohos-clangxx.ps1 --version
```

Linux 或 macOS：

```bash
sh ./scripts/ohos/ohos-clang.sh --version
sh ./scripts/ohos/ohos-clangxx.sh --version
```

如果你想直接以 `./scripts/ohos/ohos-clang.sh` 的方式运行 POSIX 包装器，请先赋予它们可执行权限：

```bash
chmod +x ./scripts/ohos/ohos-clang.sh ./scripts/ohos/ohos-clangxx.sh
```

## 链接器和工具链路径

仓库不检入 Cargo 链接器路径或 CMake 工具链路径。Cargo 无法在 `linker` 或 CMake 工具链路径值中展开环境变量，因此这些值由 `scripts/ohos-env.ps1` 和 `scripts/ohos-env.sh` 导出。

## 依赖守卫

发布准备会运行一个无 SDK 的依赖检查：

```bash
./scripts/release/check-ohos-deps.sh
```

该守卫解析 `codewhale-tui` 在 `aarch64-unknown-linux-ohos` 目标上的依赖图，如果不受支持的主机/UI crate 重新进入目标图则失败：`nix` 0.28/0.29、`portable-pty`、`starlark`、`arboard` 或 `keyring`。这不能替代真实的 SDK/sysroot 构建，但可以在发布前捕获已知的 `starlark -> rustyline -> nix` 和 PTY/keyring 回归。
