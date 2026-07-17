# RFC：MCP 模块化

**Issue：** #2190
**状态：** 草案
**日期：** 2026-05-26

## 1. 当前状态

### 1.1 `codewhale-mcp` crate（`crates/mcp/`）

当前的 MCP 实现位于单个 crate 中，具有两个职责：

- **MCP 客户端** — 通过 stdio 连接到 MCP 服务器，管理协议握手、工具发现和工具调用。由 TUI 使用，将 MCP 工具作为 `mcp_<server>_<tool>` 条目放入工具注册表。
- **MCP stdio 服务器** — 一个最小的 MCP 服务器，通过 stdio 为外部 MCP 客户端暴露 CodeWhale 自身的工具。由 `codewhale mcp` CLI 子命令使用。

客户端和服务器共享协议类型（JSON-RPC 消息、工具模式），但具有不同的生命周期关注点和不同的调用方。

### 1.2 集成点

- `crates/tui/src/mcp.rs` — MCP 客户端集成：服务器生命周期、工具发现、工具执行转发
- `crates/tui/src/mcp_server.rs` — MCP stdio 服务器：通过 stdio MCP 协议暴露 TUI 工具
- `docs/MCP.md` — 面向用户的文档

## 2. 动机

### 2.1 关注点分离

客户端和服务器共享一个 crate，但在运行时没有共享代码路径。它们导入相同的协议类型，但服务于不同的角色：
- 客户端是**出站**的 — 它连接到外部服务器
- 服务器是**入站**的 — 它接受来自外部客户端的连接

将它们混合在一个 crate 中会造成不必要的耦合：对服务器 API 的更改会重新编译客户端，反之亦然。

### 2.2 OAuth 支持

当前的 MCP 客户端没有 OAuth 支持。需要 OAuth 的 MCP 服务器（例如 GitHub、Google）无法使用。向客户端添加 OAuth 需要：
- Token 存储（钥匙串、基于环境变量或基于配置）
- OAuth 流程（设备码、PKCE 或客户端凭证）
- Token 刷新和过期处理

这些关注点仅属于客户端，不应影响服务器 crate。

### 2.3 在 TUI 之外重用

MCP 客户端当前嵌入在 TUI 二进制文件中。如果我们想从以下位置使用 MCP 工具：
- `app-server`（HTTP/SSE 运行时 API）
- `codewhale` CLI（非交互模式）
- 外部消费者（库使用）

……客户端需要成为一个具有清晰公共 API 的独立 crate。

## 3. 提案的 crate 拆分

```
crates/mcp/           →  crates/mcp-protocol/   （共享类型，无 I/O）
                           crates/mcp-client/     （客户端实现）
                           crates/mcp-server/     （服务器实现）
```

### 3.1 `codewhale-mcp-protocol`

**内容：** JSON-RPC 消息类型、工具模式类型、协议常量、握手类型、错误类型。无 I/O，无异步运行时依赖。

**依赖项：** `serde`、`serde_json`、`codewhale-protocol`（用于工具模式）

**公共 API：**
```rust
pub mod messages;     // JSON-RPC 请求/响应/通知类型
pub mod tools;        // MCP 工具模式类型
pub mod errors;       // MCP 错误码
pub mod version;      // 协议版本常量
```

### 3.2 `codewhale-mcp-client`

**内容：** MCP 客户端：stdio 传输、进程管理、握手、工具发现、工具调用、OAuth 支持。

**依赖项：** `codewhale-mcp-protocol`、`tokio`、`serde_json`、`tracing`、`oauth2`（新增，用于 OAuth）、`keyring`（可选，用于 token 存储）

**公共 API：**
```rust
pub struct McpClient {
    // 配置
}

impl McpClient {
    pub async fn connect(config: McpClientConfig) -> Result<Self>;
    pub async fn list_tools(&self) -> Result<Vec<ToolSchema>>;
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value>;
    pub async fn disconnect(self);
}

pub struct McpClientConfig {
    pub command: String,           // 例如："npx"、"python"
    pub args: Vec<String>,         // 例如：["-y", "@modelcontextprotocol/server-github"]
    pub env: HashMap<String, String>,
    pub oauth: Option<OAuthConfig>,
    pub timeout: Duration,
}

pub struct OAuthConfig {
    pub provider: OAuthProvider,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub token_storage: TokenStorage,
}

pub enum OAuthProvider {
    Github,
    Google,
    Custom { auth_url: String, token_url: String },
}
```

### 3.3 `codewhale-mcp-server`

**内容：** MCP stdio 服务器：接受连接、暴露工具列表、处理工具调用、管理 stdio 传输。

**依赖项：** `codewhale-mcp-protocol`、`codewhale-tools`、`tokio`、`serde_json`、`tracing`

**公共 API：**
```rust
pub struct McpServer {
    // 工具注册表
}

impl McpServer {
    pub fn new(tools: Vec<Arc<dyn ToolSpec>>) -> Self;
    pub async fn serve_stdio(self) -> Result<()>;
    pub async fn serve_sse(self, addr: SocketAddr) -> Result<()>;
}
```

## 4. 迁移路径

### 阶段 1：提取协议 crate（非破坏性）

1. 将共享类型从 `crates/mcp/src/` 移动到 `crates/mcp-protocol/src/`
2. 从 `codewhale-mcp` 重新导出以实现向后兼容
3. 更新 `codewhale-mcp` 中的 `Cargo.toml` 以依赖 `codewhale-mcp-protocol`

### 阶段 2：拆分客户端和服务器（对直接导入具有破坏性）

1. 创建 `crates/mcp-client/` 并包含客户端代码
2. 创建 `crates/mcp-server/` 并包含服务器代码
3. 更新 `codewhale-tui` 以依赖 `codewhale-mcp-client`
4. 更新 `codewhale-cli` 以依赖 `codewhale-mcp-server`
5. 弃用 `codewhale-mcp` crate（从新 crate 重新导出）

### 阶段 3：移除旧版 crate

1. 在弃用周期后移除 `crates/mcp/`（一个发布版本）

## 5. OAuth 集成

### 5.1 Token 存储

Token 应安全存储。选项（按优先级排序）：
1. 通过 `keyring` crate 使用操作系统钥匙串（macOS Keychain、Windows Credential Manager、Linux Secret Service）
2. `~/.codewhale/mcp-credentials/` 中的加密文件（回退）
3. 环境变量 `MCP_OAUTH_TOKEN_<PROVIDER>`

### 5.2 OAuth 流程

初始实现支持：
- **设备码流程**（GitHub）— 用户打开 URL，输入代码
- **客户端凭证** — 用于服务到服务的 MCP 服务器

未来（推迟）：
- **PKCE** — 用于面向用户的 OAuth，带重定向
- **Token 刷新** — 使用 refresh_token 自动刷新

### 5.3 配置

```toml
# ~/.codewhale/config.toml
[mcp.servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[mcp.servers.github.oauth]
provider = "github"
client_id = "your-client-id"
scopes = ["repo", "read:org"]
```

## 6. 风险和未知因素

| 风险 | 缓解措施 |
|---|---|
| Crate 泛滥 | 3 个小 crate vs 1 个中等 crate；每个都有明确的用途 |
| 破坏内部导入 | 阶段 2 携带 `codewhale-mcp` 弃用兼容层一个发布版本 |
| OAuth token 安全 | OS 钥匙串优先；具有文件权限的加密回退 |
| 测试复杂性 | 每个 crate 有其自己的测试套件；集成测试保留在 `crates/tui/tests/` 中 |
| 依赖膨胀 | `oauth2` 和 `keyring` 是可选功能；消费者选择加入 |

## 7. 超出范围（未来 RFC）

- 通过 HTTP/SSE 传输的 MCP（当前仅 stdio）
- MCP 服务器发现（当前仅显式配置）
- MCP 工具结果流式传输（当前仅请求-响应）
- MCP 服务器端工具审批流程

## 相关

- `crates/mcp/src/` — 当前实现
- `crates/tui/src/mcp.rs` — TUI MCP 集成
- `crates/tui/src/mcp_server.rs` — MCP stdio 服务器
- `docs/MCP.md` — 面向用户的文档
- Issue #2190 — 本 RFC
