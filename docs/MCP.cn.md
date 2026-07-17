# MCP（外部工具服务器）

codewhale 可以通过 MCP（模型上下文协议）加载额外的工具。MCP 服务器可以是 TUI 启动的本地 stdio 进程，也可以是使用流式 HTTP 及旧版 SSE 回退的远程 URL 服务器。

浏览说明：
- `web.run` 是权威的内置浏览工具。
- `web_search` 仍然作为旧提示和集成的兼容别名可用。

服务器模式说明：
- `codewhale-tui serve --mcp` 运行 MCP stdio 服务器。
- `codewhale-tui serve --http` 运行运行时 HTTP/SSE API（独立模式）。
- `codewhale` 调度器暴露 `codewhale mcp-server` 作为等效的 stdio
  入口点，供分离式 CLI 使用。

## 设置向导 vs 手动 MCP 设置（#3407）

宪法优先的 `/setup` 向导包含一个可选的 **工具与 MCP**
步骤。该步骤仅用于发现/就绪检查：

| 向导可以做到 | 仍需手动/显式操作 |
| --- | --- |
| 将已配置的服务器显示为 `healthy` / `needs_config` / `off` | 启动或连接 MCP 服务器 |
| 报告配置路径存在情况（全局 + 项目） | 编写或编辑 `mcp.json` 内容 |
| 安全的静态健康探测（缺失命令/URL、损坏的绝对路径、缺失 bearer 环境变量） | `codewhale mcp validate`、实时连接、OAuth 登录 |
| 指向安全的上手路径（`/mcp`、`codewhale mcp init`、`codewhale doctor`） | 安装社区技能、信任技能、启用插件 |
| 从相同的技能/MCP 适配器共享 Hotbar 来源计数（#3399） | 绑定 Hotbar 槽位（Hotbar 步骤 / `H`） |
| 记录 optional/`needs_action` setup_state 而不阻塞首次运行 | 任何启动进程或安装软件包的操作 |

空的清单**不是**错误：首次运行用户会看到"尚未配置任何内容，这没问题"。
失败或不完整的已配置服务器会以 `needs_config` 形式显示，并附有可操作的提示，且永远不会阻塞设置完成。
枚举永远不会执行超出静态探测范围的 MCP/插件命令。
摘要会隐藏命令、参数、环境变量、头部和令牌。

`codewhale doctor` 以相同的可选界面意图报告 MCP/技能/工具/插件健康状态
（路径、计数、静态检查），使向导和医生保持一致。

## 引导 MCP 配置

在已解析的 MCP 路径处创建入门 MCP 配置：

```bash
codewhale-tui mcp init
```

`codewhale-tui setup --mcp` 在技能设置的同时执行相同的 MCP 引导。

常用管理命令：

```bash
codewhale-tui mcp list
codewhale-tui mcp tools [server]
codewhale-tui mcp add <name> --command "<cmd>" --arg "<arg>"
codewhale-tui mcp add <name> --url "http://localhost:3000/mcp"
codewhale-tui mcp add <name> --url "https://example.com/mcp" --bearer-token-env-var MCP_TOKEN
codewhale-tui mcp login <name>
codewhale-tui mcp logout <name>
codewhale-tui mcp enable <name>
codewhale-tui mcp disable <name>
codewhale-tui mcp remove <name>
codewhale-tui mcp validate
```

## TUI 内管理器

在交互式 TUI 中，`/mcp` 会打开一个针对已解析
MCP 配置路径的紧凑管理器。它显示每个已配置的服务器、是否启用或
禁用、传输方式、命令或 URL、超时值、连接错误，
以及在运行发现后发现的工具/资源/提示。

支持的 TUI 内操作：

```text
/mcp init
/mcp init --force
/mcp add stdio <name> <command> [args...]
/mcp add http <name> <url>
/mcp login <name> [--scope scope]
/mcp logout <name>
/mcp enable <name>
/mcp disable <name>
/mcp remove <name>
/mcp validate
/mcp reload
```

`/mcp validate` 和 `/mcp reload` 重新连接进行 UI 发现并刷新
管理器快照。从 TUI 进行的配置编辑会立即写入，但
模型可见的 MCP 工具池不会热重载；管理器将其标记为
需要重启，直到 TUI 重新启动。

## 远程 HTTP 认证

基于 URL 的 MCP 服务器可以使用静态头部、环境变量派生的头部、bearer-token
环境变量或 OAuth。授权优先级是保守的：

1. `headers` 和 `env_headers` 首先应用。
2. 当尚未设置 Authorization 头部时，`bearer_token_env_var` 添加 `Authorization: Bearer <env value>`。
3. 存储的 OAuth 凭据仅在不存在 Authorization 头部时使用。

对于 bearer-token 认证，推荐使用环境变量支持的配置：

```json
{
  "servers": {
    "remote": {
      "url": "https://example.com/mcp",
      "bearer_token_env_var": "EXAMPLE_MCP_TOKEN"
    }
  }
}
```

对于通用远程 MCP OAuth，添加 URL 服务器并运行登录：

```bash
codewhale-tui mcp add remote --url "https://example.com/mcp"
codewhale-tui mcp login remote
```

CodeWhale 发现服务器的 OAuth 元数据，在浏览器中打开授权 URL，
监听本地回调，交换授权码，并通过 CodeWhale 密钥后端存储
令牌响应。存储的 OAuth 令牌按服务器名称加 URL 查找，
并在可能的情况下在请求前刷新。在登录期间，CLI 在
本地回调监听器活跃时打印授权 URL 和等待状态。如果基于 URL 的服务器
在连接/发现期间返回 401 或 Unauthorized，`codewhale mcp connect <name>` 会报告
需要 OAuth 认证并指向
`codewhale mcp login <name>`。资源助手列表也会对认证形状的失败
显示 `authentication_required` 条目，而不是静默地显示为空。

可选的 OAuth 字段：

```json
{
  "servers": {
    "remote": {
      "url": "https://example.com/mcp",
      "scopes": ["tools/read"],
      "oauth": {
        "client_id": "public-client-id"
      },
      "oauth_resource": "https://example.com"
    }
  }
}
```

用户级配置可以在提供者需要固定重定向时设置回调行为：

```toml
mcp_oauth_callback_port = 1455
mcp_oauth_callback_url = "http://127.0.0.1:1455/callback"
```

这些回调字段在项目作用域配置覆盖中被忽略。

## Hugging Face MCP

Hugging Face 为 Hub 资源、文档、
数据集、Spaces 和社区工具提供了一个托管的 MCP 服务器。CodeWhale 不会通过 `/hf` 调用 Hugging Face 的
Hub HTTP API；它只帮助您检查和设置常规 MCP 管理器
将加载的 MCP 配置。

推荐的设置路径是 Hugging Face 的设置生成的配置：

1. 登录后访问 <https://huggingface.co/settings/mcp>。
2. 选择最接近您 CodeWhale 配置形状的 MCP 客户端，并复制
   生成的服务器片段。
3. 将 Hugging Face 服务器条目粘贴到您已解析的 MCP 配置文件中。
4. 重启 CodeWhale，或运行 `/mcp reload` 获取管理器快照，
   如果模型可见的工具池仍需重建则重启。

CodeWhale 同时读取 `servers` 和 `mcpServers`，因此可以在
不更改 MCP 文件其余部分的情况下适配设置生成的片段。一个仅占位的
形状如下所示：

```json
{
  "servers": {
    "huggingface": {
      "url": "https://huggingface.co/mcp",
      "headers": {
        "Authorization": "Bearer ${HF_TOKEN}"
      }
    }
  }
}
```

上面的占位符不是可运行的密钥。在您的私有 MCP 配置中使用设置生成的值，
永远不要提交真实的 Hugging Face 令牌。

交互式助手：

```text
/hf mcp status
/hf mcp setup
/hf concepts
```

`/hf mcp status` 检查已配置的 MCP 文件中常见的 Hugging Face 服务器
名称或 Hugging Face MCP URL。`/hf concepts` 解释了
Hugging Face 提供者路由、Hugging Face MCP 和显式 Hub 工作流之间的区别。

官方文档：<https://huggingface.co/docs/hub/hf-mcp-server>

## 配置文件位置

默认路径：

- `~/.codewhale/mcp.json`（当 CodeWhale 文件不存在时，仍会读取 `~/.deepseek/mcp.json`）

覆盖：

- 配置：`mcp_config_path = "/path/to/mcp.json"`
- 环境变量：`DEEPSEEK_MCP_CONFIG=/path/to/mcp.json`

`codewhale-tui mcp init`（以及 `codewhale-tui setup --mcp`）写入到此解析路径。

交互式 `/config` 编辑器也暴露 `mcp_config_path`。在
TUI 中更改它会更新 `/mcp` 使用的路径，并且需要重启才能
重建模型可见的 MCP 工具池。

编辑文件或更改 `mcp_config_path` 后，重启 TUI。

## 工具命名

发现的 MCP 工具以以下形式暴露给模型：

- `mcp_<server>_<tool>`

示例：名为 `git` 的服务器有一个名为 `status` 的工具，则变为 `mcp_git_status`。

命令面板包含按服务器分组的 MCP 条目。它显示禁用
和失败的服务器，而不是隐藏它们，并使用与模型显示的相同的运行时工具名称。

## 资源和提示助手

当 MCP 启用时，CLI 还暴露辅助工具：

- `list_mcp_resources`（可选的 `server` 过滤器）
- `list_mcp_resource_templates`（可选的 `server` 过滤器）
- `mcp_read_resource` / `read_mcp_resource`（别名）
- `mcp_get_prompt`

## 最小示例

```json
{
  "timeouts": {
    "connect_timeout": 10,
    "execute_timeout": 60,
    "read_timeout": 120
  },
  "servers": {
    "example": {
      "command": "node",
      "args": ["./path/to/your-mcp-server.js"],
      "env": {},
      "disabled": false
    }
  }
}
```

您也可以使用 `mcpServers` 替代 `servers` 以与其他客户端兼容。

## 将 DeepSeek 作为 MCP 服务器运行

您可以将本地 DeepSeek 二进制文件注册为 MCP 服务器，以便其他 DeepSeek 会话（或任何 MCP 客户端）可以调用其工具。

### 快速设置

```bash
codewhale-tui mcp add-self
```

这会解析当前二进制文件路径，生成一个运行 `codewhale-tui serve --mcp` 的配置条目，并将其写入您的 MCP 配置文件。默认服务器名称为 `codewhale`。

选项：

- `--name <NAME>` — 自定义服务器名称（默认：`codewhale`）
- `--workspace <PATH>` — 服务器的工作区目录

### 手动配置

`~/.codewhale/mcp.json` 中的等效手动条目：

```json
{
  "servers": {
    "codewhale": {
      "command": "/path/to/codewhale",
      "args": ["serve", "--mcp"],
      "env": {}
    }
  }
}
```

`codewhale-tui` 二进制文件直接支持 `serve --mcp`。`codewhale`
调度器提供等效的 `codewhale mcp-server` stdio 入口点。使用
您 `PATH` 中可用的那个（运行 `which codewhale` 或 `which codewhale-tui` 来
查找完整路径）。`mcp add-self` 命令会自动解析
正确的二进制文件。

### 前提条件

- `command` 中引用的二进制文件必须存在且可执行。
- MCP 服务器通过 stdio 作为子进程运行——不需要网络端口。
- 每个 MCP 客户端会话都会启动自己的服务器进程。

### 工具命名

来自自托管 DeepSeek 服务器的工具遵循标准命名约定：

- `mcp_deepseek_<tool>`（如果服务器命名为 `codewhale`）

例如，`shell` 工具变为 `mcp_deepseek_shell`。

### MCP 服务器 vs HTTP/SSE API vs ACP

| | `codewhale-tui serve --mcp` | `codewhale-tui serve --http` | `codewhale-tui serve --acp` |
|---|---|---|---|
| **协议** | MCP stdio | HTTP/SSE JSON-RPC | ACP stdio |
| **用例** | MCP 客户端的工具服务器 | 应用程序的运行时 API | Zed/自定义 ACP 客户端的编辑器代理 |
| **配置** | `~/.codewhale/mcp.json` 条目 | 直接 URL 连接 | 编辑器 `agent_servers` 自定义命令 |
| **生命周期** | 每个客户端会话启动 | 长期运行的守护进程 | 每个编辑器代理会话启动 |

当您希望 DeepSeek 工具对其他 MCP 客户端可用时使用 `mcp add-self`。
当构建直接消费 API 的应用程序时使用 `serve --http`。
当编辑器希望将 DeepSeek 作为 ACP 代理通信时使用 `serve --acp`。

### 验证

添加后，测试连接：

```bash
codewhale-tui mcp validate
codewhale-tui mcp tools codewhale
```

## 服务器字段

每个服务器的设置：

- `command`（字符串，必填）
- `args`（字符串数组，可选）
- `env`（对象，可选）
- `connect_timeout`、`execute_timeout`、`read_timeout`（秒，可选）
- `disabled`（布尔值，可选）
- `enabled`（布尔值，可选，默认 `true`）
- `required`（布尔值，可选）：如果此服务器无法初始化，启动/连接验证将失败。
- `enabled_tools`（数组，可选）：此服务器的工具允许列表。
- `disabled_tools`（数组，可选）：在 `enabled_tools` 之后应用的拒绝列表。
- `url`（字符串，可选）：远程 MCP 服务器的流式 HTTP 端点。
- `transport`（字符串，可选）：设置为 `"sse"` 用于旧版 SSE 端点。
- `headers`（对象，可选）：基于 URL 的服务器的字面 HTTP 头部。
- `env_headers` 或 `env_http_headers`（对象，可选）：映射到环境变量名称的头部名称。
- `bearer_token_env_var`（字符串，可选）：包含 bearer 令牌的环境变量。
- `scopes`（数组，可选）：`mcp login` 的默认 OAuth 作用域。
- `oauth.client_id`（字符串，可选）：预注册的 OAuth 客户端 ID。
- `oauth_resource`（字符串，可选）：附加到授权 URL 的资源参数。

## 安全说明

MCP 工具现在遵循与内置工具相同的工具批准框架。只读 MCP 助手
（资源/提示列出和读取）可以在建议批准模式下无需提示运行，
而有副作用的 MCP 工具需要批准。

您仍然应该只配置您信任的 MCP 服务器，并将 MCP 服务器配置
视为等同于在您的机器上运行代码。避免提交字面的 `Authorization` 头部。
优先使用 `env_headers`、`bearer_token_env_var` 或 OAuth 登录，
以便密钥保持在 MCP 文件之外。

## 故障排除

- 运行 `codewhale-tui doctor` 确认它解析的 MCP 配置路径以及是否存在。
- 在 TUI 中，运行 `/mcp validate` 刷新可见的服务器/工具快照。
- 如果 MCP 配置缺失，运行 `codewhale-tui mcp init --force` 重新生成。
- 如果工具不出现，验证服务器命令可以从您的 shell 运行，并且服务器支持 MCP `tools/list`。
