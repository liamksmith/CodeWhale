# `codewhale remote-setup` - Tailscale 优先设计

状态：**设计 / 修订**。本 RFC 修订了早期云优先的 `remote-setup` 计划。保留已存在的准确实现工作：`codewhale remote-setup` 今天作为仅生成的捆绑向导存在，用于云加聊天桥接部署，而 `--apply` 仍未实现。

## 目标

为用户提供一种引导式、教育先行的方法，从另一个界面访问本地优先的 CodeWhale 运行时，而不会意外发布其代理。

默认姿态：

1. **默认本地优先。**
2. **远程时 Tailnet 私有。**
3. **仅在显式选择时公开。**

向导应询问：

> How do you want to reach CodeWhale?

并按以下顺序提供这些路径：

1. 仅本机（localhost）
2. 使用 Tailscale 的私有设备（**推荐**）
3. Telegram 机器人
4. 飞书/Lark 机器人
5. 微信个人桥接
6. 公共 webhook / Funnel（**高级**）

推荐的远程答案是 Tailscale Serve，后端仍然绑定到 `127.0.0.1`。Tailscale 提供设备身份和加密传输。Tailscale Funnel 是公共互联网暴露，必须保持为高级选项。

## 当前实现检查点

针对代码库进行了验证：

- `codewhale app-server --http` 是规范的 HTTP/SSE 运行时 API 入口点。它委托给成熟的 `serve --http` 实现。
- `codewhale app-server --mobile` 是真实的，并在 `/mobile` 提供手机控制页面。
- `--host`、`--port`、`--workers`、`--auth-token`、`--insecure-no-auth` 和可重复的 `--cors-origin` 在 `app-server --http` / `--mobile` 上存在。
- 不带 `--host` 的 `--mobile` 设计上绑定到 `0.0.0.0`。在 Tailscale 放在运行时前面时使用 `--host 127.0.0.1`。
- `/health` 和 `/v1/runtime/info` 是公共引导/监督端点。`/v1/*` 控制路由需要运行时 bearer token，除非在受信任的 loopback 绑定上显式禁用认证。
- `codewhale doctor --json` 作为机器可读的本地诊断存在。
- `codewhale remote-setup` 存在，但今天它仅是生成模式。其当前矩阵为云目标（`lighthouse`、`azure`、`digitalocean`）x 桥接（`feishu`、`telegram`）x provider 注册表。它**不**将 localhost、Tailscale、微信或 Funnel 建模为一等选择。
- Telegram 和飞书桥接验证器作为 `npm run validate:config` 存在。微信当前有 `npm run check`，但没有 validate-config 脚本。

Tailscale 推荐的准确性说明：请求的设置使用 `app-server --http`，但当前运行时仅在移动模式下提供 `/mobile`。本 RFC 保留推荐 loopback 运行时的目标命令形态，并在需要移动页面时记录已验证的当前二进制变体：

```bash
# 仅 Runtime API，已验证：
codewhale app-server --http --host 127.0.0.1 --port 7878 --auth-token "$CODEWHALE_RUNTIME_TOKEN"

# Runtime API 加 /mobile，已验证：
codewhale app-server --mobile --host 127.0.0.1 --port 7878 --auth-token "$CODEWHALE_RUNTIME_TOKEN"
```

## 通用运行时基础

每条路径从相同的本地运行时信任边界开始。

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --http \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"
```

对于当前二进制文件，如果路径需要内置的 `/mobile` 页面，使用 `--mobile --host 127.0.0.1` 代替 `--http`。

Doctor 风格本地验证：

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
curl -fsS \
  -H "Authorization: Bearer $CODEWHALE_RUNTIME_TOKEN" \
  http://127.0.0.1:7878/v1/runtime/info
```

运行时心智模型：

- 由 CodeWhale 暴露：仅其绑定的地址。推荐的绑定是 `127.0.0.1:7878`。
- 认证 token：`CODEWHALE_RUNTIME_TOKEN`，由客户端和桥接器作为 `Authorization: Bearer ...` 传递。旧版 `DEEPSEEK_RUNTIME_TOKEN` 保留为回退。
- Provider 密钥：保留在运行时配置中，而非桥接环境文件中。
- 桥接密钥：保留在传输特定的环境文件中。

## 引导流程

### 1. 仅本机（localhost）

当 TUI、SDK、浏览器或本地脚本与 CodeWhale 在同一台机器上运行时使用此选项。

设置：

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --http \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"
```

环境模板：

```env
CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878
CODEWHALE_RUNTIME_TOKEN=<用于启动 app-server 的相同值>
```

验证：

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
curl -fsS \
  -H "Authorization: Bearer $CODEWHALE_RUNTIME_TOKEN" \
  http://127.0.0.1:7878/v1/runtime/info
```

信任边界：

- 暴露：仅 loopback。
- 不暴露：LAN、tailnet 或公共互联网。
- 使用的 Token：`CODEWHALE_RUNTIME_TOKEN` 用于控制路由；本地 `/health` 和 `/v1/runtime/info` 是公共引导端点。

### 2. 使用 Tailscale 的私有设备（推荐）

使用此选项从手机或笔记本电脑访问 CodeWhale，而无需打开 LAN 或公共端口。Tailscale 在您的 tailnet 中认证设备；CodeWhale 仍然绑定到 localhost。

向导中要展示的目标设置：

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --http \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"

tailscale serve --bg --https=443 localhost:7878
```

然后从同一 tailnet 中的手机或笔记本电脑打开 Tailscale Serve URL。对于当前二进制文件的移动页面，使用已验证的移动变体启动 CodeWhale：

```bash
codewhale app-server --mobile \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"
```

然后打开（将 token 放在 URL **片段**中，而非查询参数 — `/mobile` 页面从 `location.hash` 读取它，片段永远不会被发送到 Tailscale 服务层或任何代理日志）：

```text
https://<machine>.<tailnet>.ts.net/mobile#token=<CODEWHALE_RUNTIME_TOKEN>
```

环境模板：

```env
CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878
CODEWHALE_RUNTIME_TOKEN=<openssl-rand-hex-32>
TAILSCALE_SERVE_TARGET=localhost:7878
TAILSCALE_SERVE_URL=https://<machine>.<tailnet>.ts.net
```

验证：

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
curl -fsS https://<machine>.<tailnet>.ts.net/health
curl -fsS \
  -H "Authorization: Bearer $CODEWHALE_RUNTIME_TOKEN" \
  https://<machine>.<tailnet>.ts.net/v1/runtime/info
tailscale serve status
```

信任边界：

- 暴露：一个 HTTPS 端点，可由您的 tailnet 中授权的设备访问。
- 不暴露：原始 CodeWhale 监听器；它保持在 `127.0.0.1`。
- 使用的 Token：Tailscale 身份门控网络可达性；CodeWhale 仍使用 `CODEWHALE_RUNTIME_TOKEN` 进行运行时控制。
- 注意事项：Tailscale Serve 对 tailnet 是私有的。Tailscale Funnel 是公共互联网暴露，仅属于下面的高级路径。

### 3. Telegram 机器人

当 Telegram 私信应控制本地 CodeWhale 运行时使用此选项。桥接使用 Telegram Bot API 长轮询，因此不需要公共 webhook URL 或入站端口。

设置：

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --http \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"

cd integrations/telegram-bridge
npm install --omit=dev
cp .env.example .env
$EDITOR .env
npm run validate:config -- \
  --env .env \
  --workspace-root "$PWD/../.." \
  --check-filesystem
npm start
```

环境模板：

```env
TELEGRAM_BOT_TOKEN=replace-with-botfather-token

CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878
CODEWHALE_RUNTIME_TOKEN=<用于启动 app-server 的相同值>
CODEWHALE_WORKSPACE=/path/to/workspace
# 可选覆盖；留空以继承运行时的已配置 provider/模型。
CODEWHALE_MODEL=
CODEWHALE_MODE=agent
CODEWHALE_ALLOW_SHELL=true     # 授予从桥接执行 shell 的权限；设置为 false 仅允许文本聊天
CODEWHALE_TRUST_MODE=false
CODEWHALE_AUTO_APPROVE=false

TELEGRAM_CHAT_ALLOWLIST=
TELEGRAM_ALLOW_UNLISTED=false
TELEGRAM_ALLOW_GROUPS=false
```

首次配对：

```bash
# 在 .env 中临时设置：
TELEGRAM_ALLOW_UNLISTED=true
```

私信机器人 `/status`，将返回的 `chat_id` 或 `user_id` 复制到 `TELEGRAM_CHAT_ALLOWLIST`，然后设置 `TELEGRAM_ALLOW_UNLISTED=false` 并重启桥接。

验证：

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
npm run validate:config -- \
  --env .env \
  --workspace-root "$PWD/../.." \
  --check-filesystem
```

信任边界：

- 暴露：无入站 CodeWhale 端口。Telegram 看到发送给机器人的消息。
- 不暴露：CodeWhale 保持在 `127.0.0.1`；provider 密钥保留在运行时环境变量中，而非 Telegram 环境变量中。
- 使用的 Token：`TELEGRAM_BOT_TOKEN` 用于 Telegram，`CODEWHALE_RUNTIME_TOKEN` 用于桥接到运行时的调用，`TELEGRAM_CHAT_ALLOWLIST` 用于用户/聊天门控。
- 注意事项：私信是预期的 MVP 控制界面。群组控制关闭，除非 `TELEGRAM_ALLOW_GROUPS=true`。

### 4. 飞书/Lark 机器人

当飞书或 Lark 聊天应控制本地运行时使用此选项。桥接使用 Lark/飞书长连接 SDK，因此第一个版本不需要公共 webhook URL。

设置：

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --http \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"

cd integrations/feishu-bridge
npm install --omit=dev
cp .env.example .env
$EDITOR .env
npm run validate:config -- \
  --env .env \
  --workspace-root "$PWD/../.." \
  --check-filesystem
npm start
```

环境模板：

```env
FEISHU_APP_ID=cli_xxxxxxxxxxxxxxxx
FEISHU_APP_SECRET=replace-with-app-secret
FEISHU_DOMAIN=feishu               # 国际 Lark 用户：设置为 "lark"

CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878
CODEWHALE_RUNTIME_TOKEN=<用于启动 app-server 的相同值>
CODEWHALE_WORKSPACE=/path/to/workspace
# 可选覆盖；留空以继承运行时的已配置 provider/模型。
CODEWHALE_MODEL=
CODEWHALE_MODE=agent
CODEWHALE_ALLOW_SHELL=true     # 授予从桥接执行 shell 的权限；设置为 false 仅允许文本聊天
CODEWHALE_TRUST_MODE=false
CODEWHALE_AUTO_APPROVE=false

CODEWHALE_CHAT_ALLOWLIST=
CODEWHALE_ALLOW_UNLISTED=false
FEISHU_ALLOW_GROUPS=false
```

首次配对：

临时设置 `CODEWHALE_ALLOW_UNLISTED=true`，向应用发送一次消息，将记录的 open id 复制到 `CODEWHALE_CHAT_ALLOWLIST`，然后设置 `CODEWHALE_ALLOW_UNLISTED=false` 并重启桥接。

验证：

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
npm run validate:config -- \
  --env .env \
  --workspace-root "$PWD/../.." \
  --check-filesystem
```

信任边界：

- 暴露：无入站 CodeWhale 端口。飞书/Lark 看到发送给应用的消息。
- 不暴露：CodeWhale 保持在 `127.0.0.1`；provider 密钥保留在运行时配置中。
- 使用的 Token：`FEISHU_APP_ID` / `FEISHU_APP_SECRET` 用于平台，`CODEWHALE_RUNTIME_TOKEN` 用于桥接到运行时的调用，`CODEWHALE_CHAT_ALLOWLIST` 用于聊天门控。
- 注意事项：群组控制关闭，除非显式启用。

### 5. 微信个人桥接

当个人微信账户应通过二维码登录控制本地运行时使用此选项。这不是公共账户 webhook。桥接发起长轮询，不需要公共端口。

设置：

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --http \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"

cd integrations/weixin-bridge
npm install --omit=dev
cp .env.example .env
$EDITOR .env
npm run check
npm start
```

环境模板：

```env
CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878
CODEWHALE_RUNTIME_TOKEN=<用于启动 app-server 的相同值>
CODEWHALE_WORKSPACE=/path/to/workspace
# 可选覆盖；留空以继承运行时的已配置 provider/模型。
CODEWHALE_MODEL=
CODEWHALE_MODE=agent
CODEWHALE_ALLOW_SHELL=true     # 授予从桥接执行 shell 的权限；设置为 false 仅允许文本聊天
CODEWHALE_TRUST_MODE=false
CODEWHALE_AUTO_APPROVE=false

WEXIN_CHAT_ALLOWLIST=
WEXIN_ALLOW_UNLISTED=false
WEXIN_STATE_DIR=/var/lib/codewhale-weixin-bot-bridge
```

首次配对：

设置 `WEXIN_ALLOW_UNLISTED=true`，启动桥接，扫描二维码，发送 `/status`，将返回的 `user_id` 复制到 `WEXIN_CHAT_ALLOWLIST`，然后设置 `WEXIN_ALLOW_UNLISTED=false` 并重启桥接。

验证：

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
npm run check
```

信任边界：

- 暴露：无入站 CodeWhale 端口。个人微信会话和桥接状态目录成为敏感的本地状态。
- 不暴露：CodeWhale 保持在 `127.0.0.1`；provider 密钥保留在运行时配置中。
- 使用的 Token：扫描的微信登录/会话状态用于平台访问，`CODEWHALE_RUNTIME_TOKEN` 用于桥接到运行时的调用，`WEXIN_CHAT_ALLOWLIST` 用于用户门控。
- 注意事项：这是一个个人账户桥接。将主机和状态目录视为已登录的手机会话。

### 6. 公共 webhook / Funnel（高级）

仅当用户显式选择公共互联网可达性、理解 URL 可以在 tailnet 之外访问，并且有 Tailscale Serve 或长轮询无法满足的理由时使用。

首选高级模式：

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --mobile \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"

tailscale funnel --bg --https=443 localhost:7878
```

环境模板：

```env
CODEWHALE_RUNTIME_URL=https://<public-name>
CODEWHALE_RUNTIME_TOKEN=<openssl-rand-hex-32>
PUBLIC_EXPOSURE_ACK=true
```

验证：

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
curl -fsS https://<public-name>/health
curl -fsS \
  -H "Authorization: Bearer $CODEWHALE_RUNTIME_TOKEN" \
  https://<public-name>/v1/runtime/info
tailscale funnel status
```

信任边界：

- 暴露：一个公共 HTTPS 端点，而不仅仅是您的 tailnet。
- CodeWhale 不直接暴露：后端仍绑定到 `127.0.0.1`，但前置层使选定路由可从互联网访问。
- 使用的 Token：`CODEWHALE_RUNTIME_TOKEN` 对于控制路由保持强制。
- 注意事项：公开并不意味着安全。不要使用 `--insecure-no-auth`，不要将 CodeWhale 绑定到 `0.0.0.0`，不要将此称为默认值。

## 云/VPS 姿态

云/VPS 是位置选择，而非信任模型。旧 RFC 的云工作仍然有用，但它应位于相同的可达性选择之后：

- VPS 可以运行绑定到 `127.0.0.1` 的运行时。
- 推荐的从个人设备远程访问仍然是 Tailscale Serve。
- 机器人桥接应在可用时使用长轮询/长连接，将运行时保持在主机上的 localhost-only。
- SSH 隧道对于临时验证仍然可接受：

```bash
ssh -L 7878:127.0.0.1:7878 <host>
```

公共入站监听器、公共 webhook 和 Tailscale Funnel 是高级选择，而非默认云路径。

## 先前艺术：Hermes Agent（仅供参考 - 不要复制）

Nous Research 的 Hermes Agent 验证了此设计的表驱动部分。用它获取想法；保持 CodeWhale 的风格：Rust 核心、本地运行时、在可能时零依赖 Node 桥接，以及纯文本回复。

- `gateway/platform_registry.py` 映射到我们的 `BridgeSpec` / 访问路径注册表：每平台一行，包含设置提示、所需环境变量、验证和适配器工厂。
- `gateway/pairing.py` 映射到我们的允许列表 / 首次配对流程。

从原始 RFC 继承的 Telegram 加固：

| 边界情况 | Hermes 中 | 我们的 Telegram 桥接中 |
|---|---|---|
| 409 轮询冲突 | `_looks_like_polling_conflict` | 已完成 - 轮询循环退避并警告 |
| 429 `retry_after` | 速率限制处理 | 已完成 - `telegramApi` 遵循 `parameters.retry_after` |
| 论坛 General 主题 id 处理 | 发送/输入分离 | 已完成 - 当 id 为 1 时在发送中省略 `message_thread_id` |
| 重启后过时的回复锚点 | 不带锚点重试 | 已回避 - 无 `reply_to_message_id` |
| 网络/连接超时重试 | 网络错误检测 | 部分 - 通用轮询循环退避 |
| 文本批处理 / 进度编辑 | 进度编辑测试 | 推迟 - 纯文本定期分块 |
| MarkdownV2 转义 | 转义辅助工具 | 推迟 - 纯文本 |
| Webhook 模式 | webhook 适配器 | 超出默认范围 - 长轮询优先 |

## 设计原则：表驱动，如 `ProviderSpec`

provider 注册表是要保留的模型：添加一个 provider 就是一行。将相同的想法应用于访问路径、桥接和云位置，使矩阵通过数据增长。

```text
AccessPath x Placement x BridgeSpec + ProviderSpec
----------   ---------   ----------   ------------
localhost    local       none         deepseek / openai / ...
tailscale    local/vps   none         provider 驻留在 runtime.env 中
telegram     local/vps   telegram     桥接是纯传输
feishu       local/vps   feishu       桥接是纯传输
weixin       local/vps   weixin       桥接是纯传输
funnel       local/vps   optional     显式公共暴露
```

清晰分离：

- **Provider = 运行时环境。** 运行时从 `CODEWHALE_PROVIDER`、provider 密钥变量和 provider 注册表解析 provider/模型/API 密钥。桥接不需要 provider 密钥。
- **访问路径 = 可达性。** Localhost、Tailscale Serve、聊天长轮询和 Funnel 是具有不同信任边界的独立选择。
- **桥接 = 传输。** 聊天桥接将允许的聊天消息转发到带 `CODEWHALE_RUNTIME_TOKEN` 的 `http://127.0.0.1:7878`。
- **云 = 运行位置和密钥存储位置。** 它不是打开端口 7878 的权限。

## 提案的命令界面

已验证的当前标志用于仅生成的云/桥接向导：

| 标志 | 当前状态 |
|---|---|
| `--cloud <lighthouse\|azure\|digitalocean>` | 已验证 |
| `--bridge <telegram\|feishu>` | 已验证 |
| `--provider <slug>` | 已验证，provider 注册表支持 |
| `--out <dir>` | 已验证 |
| `--generate-only` | 已验证 |
| `--apply` | 已验证标志，但未实现 |
| `--yes` | 已验证标志 |
| `--non-interactive` | 已验证标志 |

提案的 Tailscale 优先修订：

| 标志 | 含义 |
|---|---|
| `--access <localhost\|tailscale\|telegram\|feishu\|weixin\|funnel>` | 跳过可达性提示。 |
| `--placement <local\|vps\|lighthouse\|azure\|digitalocean>` | 运行时运行的位置；默认 local。 |
| `--bridge <telegram\|feishu\|weixin>` | 当 `--access` 暗示桥接时可选的。 |
| `--provider <slug>` | Provider slug；根据现有 provider 注册表验证。 |
| `--out <dir>` | 捆绑输出目录。 |
| `--generate-only` | 发出命令/环境/运行手册，不进行供应。默认。 |
| `--apply` | 未来的云 CLI 供应，需确认。仍未实现。 |
| `--yes` | 在 CI/非交互使用安全的情况下跳过最终确认门禁。 |
| `--non-interactive` | 在缺少所需值时失败，而不是提示。 |

第一个提示应该是可达性问题，而非云问题。Tailscale 应在视觉上标记为推荐。

## 生成的捆绑包

当前的捆绑模型仍然有用。扩展它，使生成的运行手册是访问路径优先的。

文件：

- `runtime.env` - provider 和运行时配置：

  ```env
  CODEWHALE_PROVIDER=openai
  OPENAI_API_KEY=replace-with-provider-key
  # 可选覆盖；留空以继承运行时的已配置 provider/模型。
CODEWHALE_MODEL=
  CODEWHALE_RUNTIME_TOKEN=<random>
  CODEWHALE_RUNTIME_PORT=7878
  CODEWHALE_RUNTIME_WORKERS=2
  RUST_LOG=info
  ```

- `<bridge>.env` - 仅在选择了桥接时的传输配置：

  ```env
  CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878
  CODEWHALE_RUNTIME_TOKEN=<相同的随机 token>
  CODEWHALE_WORKSPACE=/opt/whalebro
  # 可选覆盖；留空以继承运行时的已配置 provider/模型。
CODEWHALE_MODEL=
  CODEWHALE_MODE=agent
  CODEWHALE_ALLOW_SHELL=true     # 授予从桥接执行 shell 的权限；设置为 false 仅允许文本聊天
  CODEWHALE_TRUST_MODE=false
  CODEWHALE_AUTO_APPROVE=false
  ```

- `codewhale-runtime.service`
- 可选的 `codewhale-<bridge>.service`
- 可选的云制品：`cloud-init.yaml`、`provision.sh`、`cnb.yml` 或云特定的运行手册步骤
- `RUNBOOK.md` 包含：
  - 精确的设置命令
  - 环境模板
  - doctor 风格验证
  - 桥接的首次配对步骤
  - 信任边界摘要
  - Funnel/webhook 模式的显式"已确认公共暴露"部分

## 自动供应

保留原始安全模型：

- `--generate-only` 是默认值。
- `--apply` 是显式的，今天未实现。
- 每个命令在执行前渲染。
- 密钥不通过 shell 历史或 argv 传递。
- 云 CLI 是位置辅助工具，而非打开运行时端口的权限。

现有云目标设计保持准确：

- Tencent Lighthouse：原生加 systemd，env-file 密钥，CNB 导向计划。
- Azure VM：Docker 镜像加 Key Vault，启动时托管身份。
- DigitalOcean Droplet：原生加 systemd，env-file 密钥，`doctl` 计划。

所有云计划应将 CodeWhale 绑定到 `127.0.0.1`，然后在上面分层上述可达性路径之一。

## 命名空间迁移：`DEEPSEEK_*` 到 `CODEWHALE_*`

沿用代码中已使用的约定：优先读取 `CODEWHALE_X`，在需要兼容性时回退到 `DEEPSEEK_X`。

来自原始 RFC 的触碰列表仍然有效：

1. 桥接：对于运行时 URL/token、工作区、模型、模式、shell/trust/approval 标志、允许列表和超时，读取 `CODEWHALE_X ?? DEEPSEEK_X`。模板应发出 `CODEWHALE_*`。
2. 部署单元：首选 `/etc/codewhale/*.env`；仅在需要兼容性时保留旧路径读取。
3. `.env.example` 文件和 `config.example.toml`：以 `CODEWHALE_*` 开头，记录旧版别名。
4. 在桥接模板中删除 DeepSeek 形态的默认值，除非 DeepSeek 是显式选择的 provider。Provider 选择属于 `runtime.env`。

## 测试

现有捆绑测试应保留：

- 每个云 / 桥接 / provider 三元组均渲染。
- 运行时和桥接环境文件共享相同的 `CODEWHALE_RUNTIME_TOKEN`。
- 环境文件以 `CODEWHALE_*` 开头。
- 生成的运行手册非空并列出供应计划。
- 供应计划是命令数据，不在测试中执行。

本修订的新测试：

- 每个 `AccessPath` 行都有设置命令、环境模板、验证命令和信任边界文案。
- Tailscale 是提示排序中推荐的远程路径。
- Funnel/webhook 模式需要显式的高级/公共确认。
- `/mobile` 文档对当前二进制行为使用 `app-server --mobile --host 127.0.0.1`，或清楚地将任何 `--http` 加 `/mobile` 路径标记为提案。
- 微信可以在其进入 `remote-setup` 注册表之前被文档化，但向导必须将其标记为提案，直到 `BridgeSpec` 行和验证方案存在。

## 建议的排序

1. 修订 RFC 和运行手册文案为 Tailscale 优先。
2. 在现有云/桥接/provider 表之上添加访问路径注册表。
3. 添加 localhost 和 Tailscale 仅生成捆绑包。
4. 添加微信作为 `BridgeSpec` 行，或在注册表和验证支持落地之前将其明确隐藏在向导的"提案"后面。
5. 重新设计云捆绑，使位置为第二，可达性为第一。
6. 仅作为具有显式公共暴露确认的高级路径添加 Funnel/webhook。
7. 在仅生成输出经过审查后最后实现 `--apply`。

## 命令验证账本

针对此工作树中的 CodeWhale 代码/文档进行了验证：

- `codewhale app-server --http --host 127.0.0.1 --port 7878 --auth-token TOKEN`
- `codewhale app-server --mobile --host 127.0.0.1 --port 7878 --auth-token TOKEN`
- `codewhale doctor --json`
- `curl /health` 和已认证的 `curl /v1/runtime/info`
- Telegram 和飞书桥接的 `npm run validate:config`
- 微信桥接的 `npm run check`
- 上述现有 `remote-setup` 仅生成标志

标记为提案或外部：

- `codewhale remote-setup --access ...` 和访问路径注册表
- 向导中的一等 Tailscale、localhost、微信和 Funnel 选择
- `--apply` 执行
- Tailscale CLI 命令（`tailscale serve ...`、`tailscale funnel ...`）是外部 Tailscale 命令。它们是预期的 RFC 示例，但不是 CodeWhale CLI 标志。
