# ACP 注册表提交准备

为 #3192 准备。外部注册表提交现已开放为 `agentclientprotocol/registry#411`。

## 上游注册表要求

于 2026-06-27 对照 `agentclientprotocol/registry` 检查：

- 新条目存放在名称与 `id` 字段匹配的目录中。
- 每个条目需要 `agent.json` 加上必需的 `icon.svg`。
- `agent.json` 需要 `id`、`name`、`version`、`description`，以及至少一种 `distribution` 方法。
- 支持的发布方法为 `binary`、`npx` 和 `uvx`。
- 包版本和二进制版本必须与条目版本匹配，不允许使用 `latest`。
- 二进制平台 ID 为 `darwin-aarch64`、`darwin-x86_64`、`linux-aarch64`、`linux-x86_64`、`windows-aarch64` 和 `windows-x86_64`。
- 图标必须为 16x16 SVG，方形，单色，并使用 `currentColor`。
- 注册表 CI 运行认证检查：`initialize` 必须返回至少一个 `authMethods` 条目，类型为 `"agent"` 或 `"terminal"`。

外部 PR 作者的参考来源：

- https://github.com/agentclientprotocol/registry
- https://github.com/agentclientprotocol/registry/blob/main/FORMAT.md
- https://github.com/agentclientprotocol/registry/blob/main/CONTRIBUTING.md
- https://github.com/agentclientprotocol/registry/blob/main/AUTHENTICATION.md
- https://github.com/agentclientprotocol/registry/blob/main/agent.schema.json

## 本地 ACP 就绪审计

CodeWhale 已通过 `codewhale serve --acp` 暴露 ACP。

本地已实现：

- `crates/tui/src/main.rs` 接受 `serve --acp` 并调度到 ACP 服务器。
- `crates/tui/src/acp_server.rs` 通过换行分隔的 stdio 实现 JSON-RPC 2.0。
- `initialize` 声明：
  - `agentInfo.name = "codewhale"`
  - `agentInfo.title = "codewhale"`
  - `agentInfo.version = env!("CARGO_PKG_VERSION")`
  - `promptCapabilities.embeddedContext = true`
  - `loadSession = false`
  - `mcpCapabilities.http = false`
  - `mcpCapabilities.sse = false`
  - `authMethods` 包含终端认证：`auth set --provider <provider>`
- `session/new` 创建带有 cwd 的内存会话。
- `session/prompt` 接受字符串提示以及 text/resource/resource_link 块，并通过配置的 CodeWhale 客户端路由。
- `session/prompt` **流式输出**：每个 provider 文本增量作为 `session/update` agent_message_chunk 在到达时发出，然后提示返回 `stopReason: "end_turn"`（而不是缓冲整个对话并在最后发送一个大块）。
- 流与输入读取器并发消费，因此同一会话的 `session/cancel` 会在流中途中断对话，提示返回 `stopReason: "cancelled"`；丢弃流会中止底层 provider 连接。无提示的 `session/cancel` 保持为幂等的 `null` 空操作。对话为单飞行模式：在对话中途到达的另一个请求会收到明确的"提示进行中"错误，而不是被静默丢弃。

需要明确说明的已知限制：

- 适配器为基础 ACP，而非完整的交互式 TUI/运行时界面。
- 流式输出仅覆盖文本增量；thinking/tool/server-tool 增量不会通过 ACP 呈现（ACP 基线在此处为纯文本，`tools: None`）。
- ACP 不暴露 shell 工具、文件写入工具、检查点回放、会话加载或 HTTP/SSE 运行时 API。
- 注册表提交应在打开外部 PR 之前，通过本地运行上游注册表认证检查来把关。该检查在 `agentclientprotocol/registry#411` 打开之前已在本地通过。

提交的注册表 PR 使用 `npx` 发布方式，因为 `codewhale@0.8.65` 已发布，且 npm 包装器处理平台选择、校验和、镜像和 glibc 预检。

## 外部注册表文件

在 `agentclientprotocol/registry` 中创建以下目录：

```text
codewhale/
  agent.json
  icon.svg
```

使用具体的已发布版本。不要使用 `@latest`。

### `codewhale/agent.json`

```json
{
  "id": "codewhale",
  "name": "CodeWhale",
  "version": "0.8.65",
  "description": "Provider-agnostic terminal coding agent with first-class DeepSeek support.",
  "repository": "https://github.com/Hmbown/CodeWhale",
  "website": "https://github.com/Hmbown/CodeWhale/blob/main/docs/RUNTIME_API.md#acp-stdio-adapter-codewhale-serve---acp",
  "authors": ["Hunter Bown"],
  "license": "MIT",
  "distribution": {
    "npx": {
      "package": "codewhale@0.8.65",
      "args": ["serve", "--acp"]
    }
  }
}
```

### `codewhale/icon.svg`

```svg
<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16" fill="none">
  <path d="M2 9.5c0-3.3 2.7-6 6-6h4.5v2H8a4 4 0 0 0-4 4v.5h7.5a2.5 2.5 0 0 0 2.4-1.8l.6-2.2H16l-.7 2.7A4 4 0 0 1 11.5 12H4.2A3 3 0 0 1 2 9.5Z" fill="currentColor"/>
  <path d="M5 7h1.5v1.5H5V7Zm3 0h1.5v1.5H8V7Z" fill="currentColor"/>
</svg>
```

## 外部 PR 草稿

标题：

```text
Add CodeWhale ACP agent
```

正文：

```text
Adds CodeWhale to the ACP registry.

CodeWhale is a provider-agnostic terminal coding agent with first-class
DeepSeek support. The submitted distribution uses the published npm package and
runs `codewhale serve --acp`.

Local readiness checked in Hmbown/CodeWhale:
- ACP stdio adapter exists at `codewhale serve --acp`.
- `initialize` returns terminal auth via `auth set --provider <provider>`.
- `session/new`, `session/prompt`, and `session/cancel` are implemented.
- `session/prompt` streams provider text deltas as `session/update` chunks.
- The adapter is intentionally baseline: no ACP shell/file tools, no session
  load, and no full runtime API through ACP.

Version: 0.8.65
```

## 提交前检查清单

- 确认 `codewhale@0.8.65` 已发布到 npm：已于 2026-06-27 完成。
- 运行上游注册表验证器：已于 2026-06-27 使用 `python3 .github/workflows/verify_agents.py --auth-check --agent codewhale --verbose` 完成；结果为 `Auth OK: codewhale-terminal-auth(terminal)`。
- 验证 `npx -y codewhale@0.8.65 serve --acp` 从 `initialize` 返回 `authMethods`：已于 2026-06-27 完成。
- 保持外部 PR 正文明确说明 ACP 支持为基础级别，并不意味着完整的 TUI/运行时 API 在 ACP 内可用：已在 `agentclientprotocol/registry#411` 中完成。
