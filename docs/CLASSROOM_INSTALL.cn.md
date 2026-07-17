# CodeWhale 教室 / 实验室安装检查清单

为在运行 Windows 的实验室或教室机器上部署 CodeWhale 的 IT 管理员提供的分步检查清单。

> **受众**：IT 人员、助教、实验室管理员。
> **前提**：每台目标机器运行 Windows 10（1809+）或 Windows 11。

---

## 安装前检查清单（每台机器运行一次）

| # | 任务 | 完成？ |
|---|------|-------|
| 1 | 确认 Windows 版本：`winver` → 10 版本 17763+ 或 11 | ☐ |
| 2 | 确保用户账户是**标准用户**（非本地管理员）。安装程序不需要提权。 | ☐ |
| 3 | 验证到 `api.openai.com`（或课程使用的任何 LLM provider）的出站 HTTPS（端口 443）已开放。 | ☐ |
| 4 | 获取安装程序：从 v0.8.50+ [发布页](https://github.com/Hmbown/CodeWhale/releases/latest)或你的部门镜像下载 `CodeWhaleSetup.exe`。 | ☐ |
| 5 | 在部署之前对照 `codewhale-artifacts-sha256.txt` 验证 SHA-256 哈希。 | ☐ |
| 6 | 注意：公共安装程序目前未签名，可能会触发 Windows SmartScreen，除非你的组织在部署前对其进行签名。 | ☐ |

---

## 安装

### 选项 A — 静默安装（推荐用于镜像制作 / SCCM / Intune）

```powershell
# 以目标用户身份运行或通过每用户部署工具运行
CodeWhaleSetup.exe /S
```

静默安装程序会：
- 安装到 `%LOCALAPPDATA%\Programs\CodeWhale\bin`
- 将 bin 目录添加到**当前用户**的 PATH
- 在 Windows"应用和功能"中注册以便卸载

### 选项 B — 交互式安装

1. 双击 `CodeWhaleSetup.exe`。
2. 接受许可协议。
3. 选择安装目录（默认值适用于大多数设置）。
4. 点击**安装**。

### 选项 C — 手动回退（无安装程序）

如果 NSIS 安装程序被组策略阻止，请手动安装：

```powershell
# 1. 创建目录
$binDir = "$env:LOCALAPPDATA\Programs\CodeWhale\bin"
New-Item -ItemType Directory -Force -Path $binDir

# 2. 下载二进制文件（将 URL 调整为你的镜像或发布标签）
$tag = (Invoke-RestMethod -Uri "https://api.github.com/repos/Hmbown/CodeWhale/releases/latest").tag_name
Invoke-WebRequest -Uri "https://github.com/Hmbown/CodeWhale/releases/download/$tag/codewhale-windows-x64.exe"     -OutFile "$binDir\codewhale.exe"
Invoke-WebRequest -Uri "https://github.com/Hmbown/CodeWhale/releases/download/$tag/codewhale-tui-windows-x64.exe" -OutFile "$binDir\codewhale-tui.exe"

# 3. 添加到用户 PATH（持久化）
$currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
$pathParts = @($currentPath -split ";" | Where-Object { $_ })
if ($pathParts -notcontains $binDir) {
    $newPath = (@($pathParts) + $binDir) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
}

# 4. 刷新当前会话 PATH
$env:Path = [Environment]::GetEnvironmentVariable("Path", "User") + ";" + [Environment]::GetEnvironmentVariable("Path", "Machine")
```

---

## 安装后验证

在**每台机器**上运行（或抽样检查）：

| # | 命令 | 预期输出 | 完成？ |
|---|---------|-----------------|-------|
| 1 | `codewhale --version` | 输出版本字符串 | ☐ |
| 2 | `codewhale doctor` | 所有检查通过 | ☐ |
| 3 | `codewhale-tui --version` | 输出版本字符串 | ☐ |

如果找不到 `codewhale`，用户可能需要打开一个**新**终端窗口以使 PATH 更改生效。

## 实验室验证检查清单

在干净的实验室机器上运行一次，然后在已安装旧版 CodeWhale 的机器上再运行一次：

| # | 场景 | 预期结果 | 完成？ |
|---|----------|-----------------|-------|
| 1 | 在没有现有 CodeWhale PATH 条目的情况下安装 | 精确添加 `%LOCALAPPDATA%\Programs\CodeWhale\bin` | ☐ |
| 2 | 安装两次 | PATH 不重复 | ☐ |
| 3 | 在存在相邻 PATH 条目（如 `C:\Tools\CodeWhale\bin-extra`）的情况下安装 | 相邻条目被保留 | ☐ |
| 4 | 通过在旧版本上安装更新的 `CodeWhaleSetup.exe` 进行升级 | 应用和功能中的版本以及两个 `--version` 输出都匹配新版本 | ☐ |
| 5 | 使用 `Uninstall.exe /S` 静默卸载 | 文件、卸载注册表条目以及仅精确匹配安装程序的 PATH 条目被移除 | ☐ |

---

## API 密钥配置

每个学生需要一个 API 密钥。选项：

| 方法 | 优点 | 缺点 |
|--------|------|------|
| **每学生密钥** | 单独使用跟踪 | 更多密钥管理 |
| **共享实验室密钥** | 部署简单 | 更难审计；速率限制共享 |

### 通过环境变量部署共享密钥

```powershell
# 为当前用户设置（重启后持久化）
[Environment]::SetEnvironmentVariable("OPENAI_API_KEY", "sk-...", "User")
```

或在 `%APPDATA%\codewhale\` 中创建 `config.toml`：

```toml
[provider]
api_key = "sk-..."
base_url = "https://api.openai.com/v1"
```

### 使用 Intune / GPO 部署每学生密钥

使用组策略首选项或 Intune PowerShell 脚本为每个用户设置 `OPENAI_API_KEY` 环境变量。变量名取决于你的 LLM provider — 参见 [CONFIGURATION.md](CONFIGURATION.md)。

---

## 卸载

### 静默卸载

```powershell
& "$env:LOCALAPPDATA\Programs\CodeWhale\Uninstall.exe" /S
```

### 手动卸载（如果未使用安装程序）

```powershell
$binDir = "$env:LOCALAPPDATA\Programs\CodeWhale\bin"
Remove-Item -Recurse -Force (Split-Path $binDir)

# 从 PATH 中移除
$currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
$newPath = ($currentPath -split ";" | Where-Object { $_ -and ($_ -ne $binDir) }) -join ";"
[Environment]::SetEnvironmentVariable("Path", $newPath, "User")
```

---

## 故障排除

| 症状 | 修复 |
|---------|-----|
| 安装后找不到 `codewhale` | 打开一个**新**终端。如果仍然缺失，检查 PATH：`echo $env:Path` |
| `MISSING_COMPANION_BINARY` | 确保 `codewhale.exe` 和 `codewhale-tui.exe` 在同一目录中 |
| `TLS handshake` 错误 | 检查代理设置或使用 CNB 镜像（参见 [INSTALL.md](INSTALL.md)） |
| 防病毒软件隔离二进制文件 | 将安装目录添加到 AV 排除项 |
| `codewhale doctor` API 检查失败 | 验证 `OPENAI_API_KEY` 已设置或 `config.toml` 存在 |

---

## 镜像制作 / 黄金镜像说明

如果正在构建黄金镜像（WIM/FFU）：

1. 使用选项 A（静默）或选项 C（手动）安装 CodeWhale。
2. **不要**在镜像中设置 API 密钥 — 这些是每用户/每学生的。
3. 安装目录（`%LOCALAPPDATA%\Programs\CodeWhale\bin`）是按用户的，因此它只会对安装它的用户存在。对于同一台机器上的其他用户，请重新运行安装程序或使用选项 C。
4. 或者，安装到共享位置如 `C:\Tools\CodeWhale\bin` 并将其添加到**机器** PATH：
   ```powershell
   [Environment]::SetEnvironmentVariable("Path", "$env:Path;C:\Tools\CodeWhale\bin", "Machine")
   ```

---

## 快速参考：所有文件路径

| 项目 | 默认位置 |
|------|-----------------|
| 二进制文件 | `%LOCALAPPDATA%\Programs\CodeWhale\bin\` |
| 用户配置 | `%APPDATA%\codewhale\config.toml` |
| 卸载程序 | `%LOCALAPPDATA%\Programs\CodeWhale\Uninstall.exe` |
| PATH 条目 | `HKCU\Environment\Path`（当前用户） |

---

*最后更新：2026-06-02*
