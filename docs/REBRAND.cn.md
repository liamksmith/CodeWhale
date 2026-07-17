# 品牌重塑：DeepSeek TUI → CodeWhale

从 **v0.8.41** 开始，本项目以新名称 `codewhale` 发布。

本文档说明哪些变了、哪些没变以及如何迁移。DeepSeek 提供商集成的任何部分都没有改变——只有本地 CLI / TUI 的品牌变了。

## 太长不看

```bash
# 1. 卸载旧的封装器或二进制文件。
npm uninstall -g deepseek-tui      # 或:
cargo uninstall deepseek-tui-cli 2>/dev/null || true
cargo uninstall deepseek-tui 2>/dev/null || true
                                    # 旧版 Homebrew 安装可使用：
                                    # brew upgrade deepseek-tui

# 2. 以新名称安装。
npm install -g codewhale            # 或:
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
                                    # 旧版 Homebrew 安装可继续使用
                                    # brew install deepseek-tui，直至 tap
                                    # formula 重命名。

# 3. 使用新命令运行。
codewhale doctor
codewhale
```

你现有的 `~/.deepseek/config.toml`、`~/.deepseek/sessions/`、`~/.deepseek/skills/`、`~/.deepseek/tasks/` 和 `~/.deepseek/mcp.json` 不会被删除。新的 CodeWhale 安装优先使用 `~/.codewhale/`，旧的 `~/.deepseek/` 状态在迁移期间保留为读取回退。现有的 `DEEPSEEK_*` 环境变量继续有效。

## 重命名的内容

| 表面 | 之前 | 之后 |
|---|---|---|
| CLI 调度器二进制 | `deepseek` | `codewhale` |
| TUI 运行时二进制 | `deepseek-tui` | `codewhale-tui` |
| npm 封装包 | `deepseek-tui` | `codewhale` |
| Crates.io 包 | `deepseek-tui-cli` / `deepseek-tui` / `deepseek-*` | `codewhale-cli` / `codewhale-tui` / `codewhale-*` |
| 发布资产 | `deepseek-<platform>` / `deepseek-tui-<platform>` | `codewhale-<platform>` / `codewhale-tui-<platform>` |
| 校验和清单 | `deepseek-artifacts-sha256.txt` | `codewhale-artifacts-sha256.txt` |

## 本地状态的变化

新安装将产品自有状态写入 `~/.codewhale/` 下。现有的 `~/.deepseek/` 配置、会话、技能、任务、MCP 配置、记忆和笔记在迁移期间作为旧版回退保持可读。CodeWhale 永远不会自动删除旧目录。

## 未改变的内容

任何针对 DeepSeek 提供商 API 的内容完全保持不变：

- **环境变量**：`DEEPSEEK_API_KEY`、`DEEPSEEK_BASE_URL`、`DEEPSEEK_MODEL`、`DEEPSEEK_PROVIDER`、`DEEPSEEK_PROFILE`、`DEEPSEEK_YOLO`、`DEEPSEEK_LOG_LEVEL`，以及现有的 `DEEPSEEK_TUI_*` 运行时开关（`DEEPSEEK_TUI_BIN`、`DEEPSEEK_TUI_RELEASE_BASE_URL` 等）。出于向后兼容保留它们；重命名会破坏全球每个 shell rc。
- **模型 ID**：`deepseek-v4-pro`、`deepseek-v4-flash`，以及旧版别名 `deepseek-chat` 和 `deepseek-reasoner`。
- **主机**：`api.deepseek.com`（全球）和 `api.deepseeki.com`（中国回退）。
- **GitHub 仓库 URL**：`https://github.com/Hmbown/CodeWhale`。旧的 `Hmbown/DeepSeek-TUI` URL 在过渡期间重定向到此处。
- **Homebrew tap 和 formula**（`Hmbown/homebrew-deepseek-tui`）：现有安装仍使用旧版 formula 名称。在 tap 重命名之前将其视为仅兼容性用途；新安装文档优先使用 `codewhale` npm、Cargo、Docker 或直接下载。
- **Docker 镜像**：`ghcr.io/hmbown/codewhale`。

## 弃用适配层（v0.9.0 中移除）

为在重命名期间保持现有 shell 别名、脚本和 CI 正常工作，v0.8.41 及后续 v0.8.x 版本发布了**弃用适配层**：

- 一个 `deepseek` 二进制文件，向 stderr 打印一行警告并将 argv 转发给 `codewhale`。
- 一个 `deepseek-tui` 二进制文件，对 `codewhale-tui` 执行相同操作。
- 旧版 `deepseek-tui` npm 包已弃用，不再接收新版本发布。请安装 `codewhale` npm 包。

这些二进制适配层在 **v0.9.0** 中移除。DeepSeek 提供商支持、模型 ID、`DEEPSEEK_*` 环境变量和旧版 `~/.deepseek/` 状态回退继续得到支持。

## 实际迁移

### npm

```bash
npm uninstall -g deepseek-tui
npm install -g codewhale
```

### Cargo

```bash
cargo uninstall deepseek-tui-cli 2>/dev/null || true
cargo uninstall deepseek-tui 2>/dev/null || true
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

或在检出中：

```bash
cargo install --path crates/cli --locked --force
cargo install --path crates/tui --locked --force
```

### 旧版 `deepseek update`

当前 v0.8.x 兼容性二进制文件在检测到自身以旧版 `deepseek` 或 `deepseek-tui` 文件名运行时，`deepseek update` 或 `deepseek-tui update` 会下载规范的 CodeWhale 发布资产，并在安装目录可写时将其作为 `codewhale` 和 `codewhale-tui` 安装到旧版二进制文件旁边。

如果该更新路径无法写入安装目录，请使用上述 npm、Cargo、Homebrew 或手动重新安装命令。旧版 npm 包 `deepseek-tui` 保持弃用且不会重新发布；npm 用户应迁移到 `npm install -g codewhale`。

### Homebrew

**当前状态（v0.8.x）：** tap formula 出于兼容性仍使用旧版 `deepseek-tui` 名称。现有用户继续运行 `brew upgrade deepseek-tui`。该 formula 安装相同的当前版本 `codewhale` / `codewhale-tui` 二进制文件。

**目标状态：** 在重命名的 tap（`Hmbown/codewhale` 或现有的 `Hmbown/deepseek-tui` tap 中添加 `codewhale` formula 别名）中提供 `codewhale` formula。旧版 `deepseek-tui` formula 作为仅兼容性别名保持可安装。

**推行步骤：**

1. **审计 formula Ruby 文件** — 确认它已安装 `codewhale` / `codewhale-tui` 二进制文件，只是 formula *名称*是旧版。
2. **添加 `codewhale` formula** 到 tap，与现有的 `deepseek-tui` formula 相同或别名。
3. **更新网站和文档** — 将 `brew install codewhale` 作为主要 Homebrew 路径展示，将 `brew install deepseek-tui` 标记为旧版兼容性。
4. **一个版本的重叠期** — 至少发布一个同时提供 `codewhale` 和 `deepseek-tui` formula 的版本，以便现有的 crontab/脚本可以迁移。
5. **弃用通知** — 在旧版 formula 中添加 `caveat`，引导用户执行 `brew uninstall deepseek-tui && brew install codewhale`。
6. **最终移除** `deepseek-tui` formula，在弃用窗口期（例如两个小版本）之后。

在 formula 重命名发布之前，新安装应优先使用 npm、Cargo、Docker 或直接下载。

### 手动 / GitHub Releases

`v0.8.41` 到 `v0.8.x` 的发布同时附加了规范的 `codewhale-*` / `codewhale-tui-*` 资产和仅兼容性的 `deepseek-*` / `deepseek-tui-*` 适配资产。从 v0.9.0 开始，发布仅附加规范的 `codewhale-*` / `codewhale-tui-*` 资产和规范的 `codewhale-artifacts-sha256.txt` 校验和清单。在迁移到 v0.9.0 之前，通过 `codewhale` 安装或更新。

### 会话、技能和手动工作区

重命名二进制文件不需要从头开始：

- **配置**：首次启动时，如果 CodeWhale 文件尚不存在，CodeWhale 会将 `~/.deepseek/config.toml` 复制到 `~/.codewhale/config.toml`。它永远不会覆盖更新的 CodeWhale 配置。你可以通过 `codewhale doctor` 检查活动路径。
- **会话和任务**：当 `~/.codewhale/...` 存在时，从中读取托管状态；当仅存在旧目录时，使用 `~/.deepseek/...` 作为旧版回退。现有保存的会话仍会出现在 `codewhale sessions` 和 TUI 恢复选择器中。
- **技能**：CodeWhale 先发现工作区技能，再发现全局技能，包括 `~/.codewhale/skills` 和旧版 `~/.deepseek/skills`。包含 `SKILL.md` 的现有技能目录无需重写。
- **MCP 配置**：默认路径为 `~/.codewhale/mcp.json`。如果该文件不存在，CodeWhale 仍会读取旧版 `~/.deepseek/mcp.json`。要使用自定义 MCP 配置文件，请在 `config.toml` 中设置 `mcp_config_path` 或 `DEEPSEEK_MCP_CONFIG`。
- **手动二进制安装**：将调度器和 TUI 二进制文件作为同级文件保留在 `PATH` 上：`codewhale` 加 `codewhale-tui`。在 Windows 上，推荐的用户本地位置是 `%LOCALAPPDATA%\Programs\CodeWhale\bin`。在类 Unix 系统上，只要两个二进制文件都存在，任何用户可写的 `PATH` 目录都可以。
- **指定工作目录**：从项目目录运行 `codewhale` 或以特定工作区路径启动它不会移动项目文件。CodeWhale 首先读取 `<workspace>/.codewhale/config.toml`，当新路径不存在时回退到旧版 `<workspace>/.deepseek/config.toml`。

如果 `~/.codewhale/...` 和 `~/.deepseek/...` 副本同时存在，CodeWhale 路径优先。在确认 `codewhale doctor`、`codewhale sessions` 和你的预期技能都显示相同状态之前，保留旧目录。

## 为什么改名

CodeWhale 是对同一个终端编程智能体及其长期产品方向的更短、更终端友好的名称：一个以 DeepSeek 为先的、面向开源和开放权重编程模型的智能终端。项目名称、命令名称、包名称、发布资产、Docker 镜像和 CNB 镜像迁移到 CodeWhale；官方 DeepSeek 提供商、模型 ID、环境变量和 `~/.deepseek/` 配置表面保持首要地位。

## 报告重命名相关问题

如果你的安装在迁移过程中出现问题，请在 <https://github.com/Hmbown/CodeWhale/issues> 创建 issue，并包含：

- `codewhale --version` 的输出（如果仍在使用适配层，则为 `deepseek --version`）。
- 你使用的安装路径（npm、cargo、brew、手动）。
- 你运行的确切命令和完整的错误输出。

我们将优先处理迁移回归问题。
