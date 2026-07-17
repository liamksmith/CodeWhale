# 仓库代理指南

## 当前工作位置（请先阅读此项）

- **仓库：** `Hmbown/CodeWhale`。此仓库存在于多台设备上，请在你拥有的本地检出中工作——保持路径与设备无关，并始终**在编辑前通过 `git branch --show-current` 确认分支。**
- **活跃分支：** 从实际状态出发。根据最新的交接文档和目标文件以及 `git branch --show-current` 确认当前的修复/集成分支；近期工作通过小型 PR 落地在 `main`，而非长期存活的 `codex/...` 集成分支，因此依赖之前先确认命名集成分支仍然存在。
- **工作区版本：** 从 `Cargo.toml`（`[workspace.package] version`）读取；版本随发布线推进，因此将该文件视为唯一真相来源，而非任何记忆中的版本号。谨慎地升级版本，每次升级独立成一次提交。
- **里程碑路标：** 使用活跃交接文档中命名的当前发布里程碑，并实时列出，例如：`gh issue list --repo Hmbown/CodeWhale --milestone "<当前里程碑>" --state open`。
- **默认分支为 `main`。** 对于发布线工作，直接提交到 `main` 是允许的——保持每次提交关注一个可审查的关注点，附带真实的提交说明。对于孤立或有风险的变更，新建 `codex/...` 分支或工作树仍然是正确的选择，以 PR 形式提交到适合审查的位置。
- **推送变更前始终执行：** `cargo fmt`，然后针对相应区域的目标测试（`cargo test -p codewhale-tui --locked <过滤器>`、`cargo test -p codewhale-config`、`cargo test -p codewhale-protocol` 等）。全量门控：`cargo test --workspace`。发布构建：`cargo build --release -p codewhale-cli -p codewhale-tui`。
- **已知测试套件瑕疵（预先存在，非本次回归）：**
  `run_verifiers_background_*` 在全量套件并行执行时存在不稳定现象，但在隔离状态下正常通过。将其归类为已知不稳定现象，而非你的变更所致。（旧有的 `config_command_allow_shell_*` 失败问题出现在 `default_mode = "yolo"` 的机器上，已通过将命令测试应用锁定为 Agent 模式解决。）

## 构建 / 测试 / 代码检查

```bash
# 默认构建（cli + app-server + tui）
cargo build

# 构建所有工作区成员
cargo build --workspace

# 发布构建（交付的二进制文件）
cargo build --release -p codewhale-cli -p codewhale-tui

# 运行所有测试
cargo test --workspace --all-features --locked

# 仅运行单个 crate 的测试
cargo test -p codewhale-config
cargo test -p codewhale-protocol
cargo test -p codewhale-state
cargo test -p codewhale-tui --locked

# 格式化（每次推送前必须执行）
cargo fmt --all

# 代码检查（工作区范围，使用项目级允许列表）
cargo clippy --workspace --all-features --locked -- -D warnings \
  -A clippy::uninlined_format_args \
  -A clippy::too_many_arguments \
  -A clippy::unnecessary_map_or \
  -A clippy::collapsible_if \
  -A clippy::assertions_on_constants

# 运行评估测试框架（提示词/组合冒烟测试）
cargo run -p codewhale-tui --all-features -- eval

# 直接运行 TUI 二进制文件
cargo run --bin codewhale-tui
```

**CI 跳过纯文档 / Markdown 变更的昂贵工作。** `changes` 作业将每个 PR 分类为重型或轻型；测试/代码检查/静态分析门控仅在可执行内容（Rust、JS 或 CI 执行的脚本）发生变更时触发。轻型变更 PR 仍然需要通过 `Version drift` 和 `check-coauthor-trailers.py` 检查。

**Linux 工作区测试在 CNB 镜像上运行**，而非 GitHub Actions。GitHub 仅保留 macOS + Windows 覆盖率。

**rust-toolchain：** `stable`，edition `2024`，最低 rust 版本 `1.88`。

## 架构

CodeWhale 是一个终端编码代理——一个 TUI 和一个 CLI，指向一个模型和一个项目。`tui` crate 仍然是活跃的最终用户运行时（引擎、工具、提示词路由、会话管理）；其他工作区 crate 正在逐步拆分，但尚未成为唯一的运行时真相来源。

### Crate 地图（工作区成员）

| Crate | 用途 |
|-------|------|
| `cli` | `codewhale` CLI 调度器（认证、检测、配置、模型、服务）。薄层：将交互式运行委托给 TUI 二进制文件。 |
| `tui` | 活跃运行时——TUI（ratatui）、代理引擎循环、工具、LLM 客户端、MCP 生命周期、会话管理器、任务管理器、运行时 API、LSP 集成。main.rs 约 11.5 kLOC。 |
| `config` | 配置加载、供应商、模型目录、定价、用户宪章、设置状态。 |
| `core` | 无头运行时，用于线程、目标、作业。通过 `ToolRegistry` 包装 `tui` 工具表面。 |
| `agent` | `ModelRegistry`——包含供应商路由和别名的内置模型规范列表。 |
| `tools` | `ToolRegistry`、`ToolResult`、`ToolError`、输入验证辅助函数。 |
| `protocol` | 跨 crate 的共享类型：`Thread`、`ThreadStatus`、`SessionSource`、`EventFrame`、`AppRequest`/`AppResponse`。 |
| `execpolicy` | Shell 执行策略引擎（受信任/拒绝前缀、类型化 `ToolAskRule`、`AskForApproval`）。 |
| `hooks` | Hook 事件分发系统（`ResponseStart`、`ToolLifecycle` 等），支持 stdout/jsonl/webhook 接收端。 |
| `mcp` | Model Context Protocol 客户端：服务器管理、工具/资源描述符、`McpManager`。 |
| `state` | SQLite 持久化：线程、消息、检查点、作业、目标。 |
| `workflow` | 类型化 Workflow IR 及验证（声明式 JSON/YAML 计划、模型策略、门控、舰队形态）。 |
| `workflow-js` | 沙箱化 QuickJS（rquickjs）运行时，执行模型编写的 JS，支持 `task()`/`parallel()`/`pipeline()`。 |
| `lane` | Lane 注册表及运行时后端（inline、tmux）。持久化 lane 记录。 |
| `app-server` | HTTP（Axum）和 stdio JSON-RPC 服务器，为无头/API/SSE 使用场景包装 `core::Runtime`。 |
| `secrets` | 密钥存储（供应商 API 密钥），与系统密钥链集成。 |
| `build-support` | 构建时辅助（版本嵌入、构建信息注入）。 |
| `release` | 平台 HTTP 客户端构建器，发布辅助函数。 |

### 其他顶级目录

| 目录 | 内容 |
|-----|------|
| `web/` | Next.js 市场/社区网站（codewhale.net）。Cloudflare Pages。 |
| `integrations/` | 聊天桥接适配器（飞书、Telegram、企业微信、微信）。Node.js。 |
| `extensions/vscode/` | VS Code 扩展。 |
| `npm/` | npm 包：`codewhale` 包装器、遗留 `deepseek-tui`、运行时 SDK 类型。 |
| `scripts/` | 发布自动化、冒烟测试、QA 设置、目录脚本。 |
| `workflows/` | 树内 `.workflow.js` 文件——由 `workflow-js` 执行的声明式 Workflow IR。 |
| `fleets/` | 舰队名册定义（TOML）。 |
| `docs/` | 面向用户的文档、架构、RFC、证据、技能。 |
| `deploy/` | 部署配置（腾讯轻量服务器、systemd）。 |

### 数据流（交互式会话）

1. TUI 接收用户输入 → `core/engine.rs` 代理循环
2. 提示词由 `prompts.rs` 模板 + 配置指令 + 用户记忆组装
3. 请求通过 `llm_client.rs` 分发到供应商
4. 响应流式返回，解析工具调用
5. 工具调用通过 `tools/` 层执行（shell、文件、git、MCP、RLM 等）
6. Hook 在工具执行前后触发
7. 结果反馈给 LLM 进行下一轮
8. 最终响应在 TUI 中渲染

## 关键文件与目录

- `Cargo.toml` — 工作区根：版本、成员、共享依赖、发布配置
- `config.example.toml` — 完整配置参考（约 1200 行）
- `crates/tui/src/main.rs` — 主 TUI 二进制入口（约 11.5 kLOC）
- `crates/cli/src/lib.rs` — `codewhale` CLI 调度器（约 5.5 kLOC）
- `crates/core/src/lib.rs` — 无头 `Runtime`、`Thread`、`JobManager`
- `crates/agent/src/lib.rs` — `ModelRegistry`，包含所有内置模型
- `crates/config/src/lib.rs` — `ConfigToml`、`ProviderKind`、供应商定义
- `crates/tools/src/lib.rs` — `ToolRegistry`、`ToolResult`、输入辅助函数
- `crates/protocol/src/lib.rs` — 共享协议类型
- `crates/state/src/lib.rs` — `StateStore`（SQLite 持久化）
- `.github/workflows/ci.yml` — CI 流水线，含变更检测门控
- `AGENTS.md` — 本文件（代理指南）
- `CLAUDE.md` — Claude 兼容适配层（委托给 AGENTS.md）

## 编码规范

- **Edition 2024**，仅限 Rust 1.88+ stable。不使用 nightly 特性。
- **错误处理：** 应用代码使用 `anyhow::Result`，库错误枚举使用 `thiserror`（参见 `ToolError`、`WorkflowJsError`）。
- **命名：** 标准 Rust 规范（函数/变量用 snake_case，类型用 CamelCase）。配置 serde 大量使用 `#[serde(alias)]` 以保持向后兼容。
- **模块组织：** 每个 crate 内部扁平化组织；TUI crate 是例外，`src/` 下有约 100+ 个顶级模块。
- **测试：** 单元测试与代码同文件放置（`#[cfg(test)] mod tests`）；集成测试放在 crate 级别的 `tests/` 目录中。不使用仓库根级别的 `tests/` 目录。
- **异步运行时：** `tokio`（多线程）。`rquickjs` 用于 JS 沙箱（`futures` 特性，单线程——Workflow VM 通过通道桥接到多线程引擎）。
- **序列化：** 统一使用 `serde`/`serde_json`。协议类型对标记枚举使用 `#[serde(tag = "type", rename_all = "snake_case")]`。
- **内存分配器：** `cli` 和 `tui` 二进制文件中均使用 `mimalloc` 作为全局分配器。
- **TLS：** 使用 `rustls` + `ring` 加密供应商（无 OpenSSL 依赖）。
- **持久化：** 状态使用 SQLite（`rusqlite`，bundled）；会话日志和 hook 事件使用 JSONL。

## Git 工作流

- **约定式提交：** `feat:`、`fix:`、`docs:`、`refactor:`、`test:`、`chore:`。
- **一次提交一个关注点**，附带真实的提交说明。
- **分支命名：** 特性/集成工作用 `codex/...`，修复用 `fix/...`，发布分类临时分支用 `scratch/vX.Y.Z-pr-train-YYYYMMDD`。
- **收割流程：** 社区 PR 可能被收割到维护者提交中，提交说明中包含 `Harvested from PR #N by @handle` 以及 `Co-authored-by` 尾部信息。
- **禁止强制推送到 main 或共享分支。**

## CI/CD

- **CI 触发条件：** 推送到 `main`、PR 到 `main`、每周定时任务。
- **变更检测：** `changes` 作业通过文件分类步骤对重型 Rust 工作（测试、clippy、npm 冒烟）进行门控。仅文档/Markdown 变更的 PR 跳过昂贵 CI。
- **必需上下文：** `Lint`、`Version drift`、`Test (ubuntu-latest / macos-latest / windows-latest)`、`npm wrapper smoke (ubuntu-latest)`。
- **Linux 测试在 CNB 镜像上运行**，而非 GitHub Actions。
- **供应商注册表漂移检测**和**共同作者尾部合规检查**在 `Lint` 作业中执行。
- **离线评估框架**在 `Test` 作业的 macOS 上运行。
- **移动端运行时冒烟测试**仅对相关文件变更进行门控。
- **其他工作流：** `pr-gate.yml`、`release.yml`、`nightly.yml`、`web.yml`、`auto-close-harvested.yml`。

## 持续代理工作规范

- 一次提交一个关注点；写真实的提交说明。将不相干的变更分开提交。
- 除非你实际验证了行为（构建了二进制文件、运行了测试、复现了修复），否则提交为 **WIP**。声称"已修复"而没有证据，比诚实的 WIP 更糟糕。
- 只构建当前存在的表面（已移除的机制不可再用）：面向模型的子代理表面**仅为 `agent`**——`agent_open`/`agent_eval`/`agent_close`/`delegate_to_agent` 变体、容量/一致性/运行时标签系统、生命周期工具以及运行时提示词/标签注入均已移除。`constitution.md` 是唯一的基础提示词。
- 可配置的子代理深度保持不变。仅在明确需要时才添加新限制，并解释原因。
- 较早交接文档中报告的子代理 **TUI 冻结问题已由** v0.8.61 切换（cap-20、持久化防抖、AgentProgress 重绘节流、ListSubAgents 合并、input-pump-off-render-thread）解决。主导的"阻塞 I/O 耗尽工作池"理论已被测量并**证伪**（`git rev-parse` ~10ms，18 核机器）。将冻结问题视为已关闭，将精力投入其他方向，而非对推测性的 `spawn_blocking` 修复。

## CodeWhale 管理规范

- 将社区贡献者视为合作伙伴。善意的 PR、问题报告、复现步骤、日志、审查和验证评论是维护者证据，而非队列噪音。
- 保持门控处于预热和预演状态，除非 Hunter 明确批准强制执行。门控文案应清晰且尊重地引导贡献者。
- 对每一个对本修复产生实质影响的收割 PR、问题报告或评论给予署名。尽可能保留作者身份；否则使用来自 `.github/AUTHOR_MAP` 的可映射 GitHub noreply `Co-authored-by` 尾部信息。
- CodeWhale 最初是 DeepSeek 专用工具集；现在的目标是在开源社区的帮助下，构建最优秀的编码工具集。保持 CodeWhale 品牌以及每个模型/供应商的一等公民地位——无特权。当退役 `deepseek-tui` 等遗留名称时，明确每个模型和供应商都保持完全支持。
- 从代码、测试、关联问题、评论和检查结果来审查 PR——让这些因素，而非仅标题或标签，驱动社区工作的每一次合并、关闭、收割或推迟决策。
- 尊重树中的并发工作——保持他人或其他代理的不相关编辑原封不动。

## 发布 PR 集成

- 在分类拥挤的发布队列时，使用临时集成分支。像 `scratch/vX.Y.Z-pr-train-YYYYMMDD` 这样的分支可以合并或 cherry-pick 多个 PR 头部，以快速暴露冲突、缺失的测试、重复工作和隐藏的耦合。
- 将临时分支视为证据，而非要交付的产物。通过将安全解决的块或提交以窄范围、可审查的提交收割回发布分支——保持标签、发布和快进操作远离临时分支。
- 仅在 PR 对实际落地分支干净、具有可接受的检查结果且不跨越信任边界表面时，才优先直接 GitHub 合并。一个对 `main` 干净的 PR 仍可能与发布分支冲突；在宣称可合并之前，先对实际发布头部进行测试。
- 对于已获批准的 PR，先从发布分支进行临时合并，然后决定直接合并、带冲突解决的 cherry-pick 还是署名收割。维护者批准是优先级信号，而非跳过审查或测试的许可。
- 收割时，保留或添加机器可读署名：尽可能保留原始作者，使用 `.github/AUTHOR_MAP` 或 GitHub 数字 noreply 身份添加 `Co-authored-by`，并在提交说明中包含 `Harvested from PR #N by @handle`，以便自动关闭工作流在到达 `main` 后能带署名关闭 PR。携带有该行的提交的 PR 应通过 rebase 或 merge commit 合并，以确保提交说明完整保留——squash 可能会重写它，丢失 `Harvested from PR` 行，从而悄无声息地丢失机器可读署名和自动关闭功能。
- 保持 `Co-authored-by` 尾部信息仅限于人类贡献者——`scripts/check-coauthor-trailers.py` 在收割提交中拒绝 bot/工具类（Claude、codex、cursor、`noreply@anthropic.com`）。同时刷新不会从尾部信息自动填充的手动署名表面：`docs/CONTRIBUTORS.md` 和 `CHANGELOG.md`。
- 仅在验证落地提交已在相关分支上后，才关闭或更新问题和 PR。如果发布分支已经包含等价行为，留下清晰的备注，链接提交并描述任何剩余的差异。
- 对于活跃发布队列，从活跃交接文档中命名的当前 GitHub 发布里程碑开始（`gh issue list --repo Hmbown/CodeWhale --milestone "<当前里程碑>"`），并在操作前刷新状态。`docs/` 下的旧版本分类文档仅供参考历史。

## 给 AI 代理的提示

- **TUI crate 就是运行时。** 大多数行为变更（工具、提示词、引擎逻辑、LLM 客户端、会话管理）位于 `crates/tui/src/`。`core` crate 是围绕它的无头包装器。不要在未先理解 `tui` 表面的情况下在 `core` 中添加并行实现。
- **模型注册表**位于 `crates/agent/src/lib.rs`（`ModelRegistry::default()`）。要添加新的供应商模型，在那里添加 `ModelInfo` 条目，并在 `crates/config/` 中添加相应的 `ProviderKind` 变体。
- **供应商配置**在 `crates/config/src/lib.rs`（`ProvidersToml`）中是强类型的。新供应商需要在那里添加字段、在 `crates/cli/src/lib.rs`（`ProviderArg`）中添加 CLI 变体，以及在 `crates/secrets/` 中添加认证管道。
- **工具**通过 `crates/tools/` 中的 `ToolRegistry` 注册。TUI crate 负责连接实际实现。要添加工具，定义其 schema 和处理函数，注册它，并在 `execpolicy` 中添加任何审批规则。
- **LSP 集成**（`crates/tui/src/lsp/`）通过 `lsp_hooks.rs` 连接到引擎——它在每次 `edit_file`/`apply_patch`/`write_file` 后触发。
- **不要在无明显需求的情况下**向工作区 `Cargo.toml` 添加新的顶级依赖。依赖集有意保持精简。
- **`Cargo.lock` 漂移**是 CI 门控（`git diff --exit-code -- Cargo.lock`）。始终提交 lockfile 变更。
- **`config.example.toml`** 是规范配置参考。如果添加配置键，请在那里记录。
- **发布配置**使用 `lto = true`、`strip = true`、`codegen-units = 1`。无 `panic = "abort"`——TUI 的 panic 监管需要 unwind。
- **`web/` 目录**是部署到 Cloudflare Pages 的 Next.js 应用，不属于 Rust 构建的一部分。它有自己独立的 `package.json`、构建和 lint。
- **`CLAUDE.md`** 是兼容适配层——它委托给本文件。不要在那里重复指导内容；更新 AGENTS.md 并保持 CLAUDE.md 精简。
