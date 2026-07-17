# Provider 注册表

本注册表描述了已集成到当前 CodeWhale 代码库中的 provider 行为。内容有意保持保守：已发布的条目仅限于代码已知的 provider ID、配置键、认证路径、base URL、模型解析和能力元数据。

DeepSeek 仍然是默认 provider，但 `ProviderKind::ALL` 和 `PROVIDER_REGISTRY` 中的每个条目都是一等公民的可选 provider 路由。托管路由、通用 OpenAI 兼容端点、OpenAI Codex/ChatGPT 路由、原生 Anthropic 以及本地运行时都使用相同的终端框架，基于选定的 provider/模型/base URL 运行。

需要保持同步的源文件：

- `crates/config/src/lib.rs` - 共享的 provider ID、默认值、环境变量优先级。
- `crates/tui/src/config.rs` - TUI provider ID、provider 能力元数据以及 provider 特定的环境变量处理。
- `crates/agent/src/lib.rs` - `codewhale model list` 和 `codewhale model resolve` 使用的静态 `ModelRegistry`。
- `config.example.toml` 和 `docs/CONFIGURATION.md` - 面向用户的配置示例和环境变量参考。
- `scripts/check-provider-registry.py` - 对规范 provider ID、活跃 TUI provider ID、TOML 表名、静态注册表行以及文档默认值进行漂移检查。

## Provider 选择

规范 provider ID 如下：

`deepseek`、`deepseek-anthropic`、`nvidia-nim`、`openai`、`atlascloud`、
`wanjie-ark`、`volcengine`、`openrouter`、`xiaomi-mimo`、`novita`、`fireworks`、
`siliconflow`、`arcee`、`siliconflow-CN`、`moonshot`、`sglang`、`vllm`、
`ollama`、`huggingface`、`together`、`qianfan`、`openai-codex`、`anthropic`、
`openmodel`、`zai`、`stepfun`、`minimax`、`deepinfra`、`sakana`、`longcat` 和
`xai`。

使用以下任一方式选择 provider：

- CLI：`codewhale --provider <id>`
- TUI：`/provider <id>` 或 provider 选择器
- 环境变量：`CODEWHALE_PROVIDER=<id>`；`DEEPSEEK_PROVIDER=<id>` 是旧版别名
- 配置：`provider = "<id>"`

`deepseek-cn`、`deepseek_china`、`deepseekcn` 和 `deepseek-china` 被接受为 `deepseek` 的旧版别名。它们不会选择不同的官方主机；DeepSeek 在全世界使用相同的官方 API 主机。

`deepseek_anthropic`、`deepseek-claude` 和 `deepseek_claude` 选择
`deepseek-anthropic`，即可选的 DeepSeek 路由，通过 Anthropic
Messages API 在 `https://api.deepseek.com/anthropic` 进行通信。它保持正常的
DeepSeek API key 路径，但使用 `x-api-key` 加 `anthropic-version: 2023-06-01`
而不是 Bearer 认证。

`huggingface`、`hugging-face`、`hugging_face` 和 `hf` 都选择
Hugging Face Inference Providers 路由。这是用于聊天/推理的 OpenAI 兼容路由器路径，而不是 Hub 浏览、模型卡检查、上传或工件导出。

全新共享配置写入 `~/.codewhale/config.toml`。现有的
`~/.deepseek/config.toml` 文件仍然会被读取以保持兼容性。

### 通信协议兼容性

Provider 选择是显式的。诸如
`deepseek-ai/...`、`deepseek/...`、`qwen/...` 或 `arcee-ai/...` 之类的模型字符串前缀是在选定 provider 下的 provider 拥有的通信 ID 或目录命名空间提示。它不是 provider 切换，绝不能被视为证明路由是 DeepSeek、OpenRouter 或其他任何 provider 的证据。

使用 `provider = "<id>"`、`CODEWHALE_PROVIDER=<id>`、
`--provider <id>` 或 TUI `/provider <id>` 来设置路由。模型字符串前缀会原样传递到 provider 的 API，CodeWhale 不会根据它来切换 provider。

### 默认模型

CodeWhale 附带了每个 provider 的默认静态模型 ID。这些在编译时通过代码和文档表中的显式常量声明。使用以下任一方式覆盖：

- `codewhale --model <id>`
- 环境变量：`CODEWHALE_MODEL=<id>`（或 provider 特定的 `DEEPSEEK_MODEL`、`OPENAI_MODEL` 等）
- 配置：`[providers.<id>].model = "<id>"` 或全局 `model = "<id>"`
- TUI：`/model <id>` 或模型选择器

`deepseek-chat` 和 `deepseek-reasoner` 是 `deepseek-v4-pro` 和 `deepseek-v4-flash` 的受支持兼容性别名。
CodeWhale 在内部将它们规范化为 V4 ID，但两个别名都可使用，并且仍然通过 `/models` 和 picker 列出。

### 请求负载模式

大多数 provider 使用标准 OpenAI Chat Completions 负载。原生 Anthropic
provider 使用 Anthropic Messages API。DeepSeek Anthropic 路由
（`deepseek-anthropic`）同样使用 Messages。`openai-codex` provider 使用
OpenAI Responses API（`/codex/responses`）。设置
`kind = "openai-compatible"` 的自定义表使用 Chat Completions。`openmodel`
provider 默认使用 `AnthropicMessages`；可通过 `request_payload_mode` 覆盖。

## 认证与环境变量规则

对于托管 provider，`codewhale auth set --provider <id>` 会为该 provider 保存一个 API key。API key 环境变量是已保存配置和密钥环凭据之后的回退输入；显式的进程级 `--api-key` 对于该次启动仍然优先。

对于 base URL 和模型选择，优先使用：

- `CODEWHALE_BASE_URL` / `CODEWHALE_MODEL` 用于当前活跃 provider。
- 下文列出的 provider 特定 base URL/模型环境变量。
- `DEEPSEEK_BASE_URL`、`DEEPSEEK_MODEL` 和 `DEEPSEEK_DEFAULT_TEXT_MODEL` 作为旧版别名。

非本地的 `http://` base URL 将被拒绝，除非设置了
`DEEPSEEK_ALLOW_INSECURE_HTTP=1`。回环 HTTP URL 允许用于自托管运行时。

## 自定义 DeepSeek 兼容端点

大多数自定义 DeepSeek 兼容部署可以使用现有的 provider ID。
不要创建 `[providers.deepseek_custom]`；provider 表名是固定的。
而是选择最接近的内置路由并覆盖其端点/模型：

- DeepSeek 兼容托管 API：保持 `provider = "deepseek"` 并设置
  `[providers.deepseek].base_url` 加 `[providers.deepseek].model`，或使用
  `DEEPSEEK_BASE_URL` 和 `DEEPSEEK_MODEL` 启动。
- 通用 OpenAI 兼容网关：使用 `provider = "openai"` 并设置
  `[providers.openai].base_url` 加 `[providers.openai].model`，或使用
  `OPENAI_BASE_URL` 和 `OPENAI_MODEL` 启动。
- 多个命名的 OpenAI 兼容网关，或你想从 AgentProfile 固定的本地路由，可以使用自定义表，如
  `[providers.lm-studio] kind = "openai-compatible"` 并通过
  `provider = "lm-studio"` 或 profile 的 `provider = "lm-studio"` 来选择。
- 本地 OpenAI 兼容运行时：使用 `provider = "vllm"`、`"sglang"` 或
  `"ollama"` 并配合匹配的 provider 特定 base URL/模型值。

DeepSeek 兼容主机的用户配置示例：

```toml
provider = "deepseek"

[providers.deepseek]
api_key = "YOUR_API_KEY"
base_url = "https://your-provider.example/v1"
model = "deepseek-ai/DeepSeek-V4-Pro"
```

通用网关的用户配置示例：

```toml
provider = "openai"

[providers.openai]
api_key = "YOUR_GATEWAY_API_KEY"
base_url = "https://gateway.example/v1"
model = "your-deepseek-compatible-model"
```

阿里云百炼 / Model Studio DashScope 通过 OpenAI 兼容的 Chat Completions 端点暴露通义千问。将其配置为显式的
`openai` provider 路由，以便 API key、base URL 和通信模型限定在该 provider 范围内：

```toml
provider = "openai"

[providers.openai]
api_key = "YOUR_DASHSCOPE_API_KEY"
base_url = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
model = "qwen-plus"
context_window = 1000000
```

以上新加坡端点将聊天请求发送到 `/chat/completions`；
阿里云还记录了 Virginia、北京、香港和法兰克福的区域性 `compatible-mode/v1` base URL。请保持 API key 和 base URL 来自同一区域。`qwen-plus` 模型 ID 作为 OpenAI provider 范围内的通信 ID 被保留；CodeWhale 不会从 `qwen` 前缀推断切换到 OpenRouter、DeepSeek 或其他 provider。当 `context_window` 与 CodeWhale 的静态模型元数据不一致时，将其设置为网关/模型的真实总上下文窗口。

带有损坏或被拦截证书的私有网关应使用
`SSL_CERT_FILE` 配合受信任的 CA 捆绑包。旧版
`insecure_skip_tls_verify = true` 键仍然会被解析，以便 `codewhale doctor` 可以报告过时的配置，但 provider 客户端会拒绝它，而不是跳过 TLS 证书验证。

将 `provider`、`api_key` 和 `base_url` 保留在用户配置或进程环境中。项目本地配置覆盖有意不能设置这些键，因此仓库无法静默地将提示或凭据重定向到另一个端点。

## 凭据链接

Provider 设置界面应将用户链接到 provider 拥有的凭据页面，而不是让他们从缺失密钥的错误中去搜索。运行时代码在可能的情况下使用相同的链接。

| Provider ID | 凭据或控制台链接 |
| --- | --- |
| `deepseek` | [DeepSeek API keys](https://platform.deepseek.com/api_keys) |
| `nvidia-nim` | [NVIDIA NIM API keys](https://build.nvidia.com/settings/api-keys) |
| `openai` | [OpenAI API keys](https://platform.openai.com/api-keys) |
| `atlascloud` | [Atlas Cloud API keys](https://atlascloud.ai/docs/en/api-keys) |
| `wanjie-ark` | [万界 MaaS APIKEY 文档](https://docs.wanjiedata.com/maas/maas-openapi-v1.html) |
| `volcengine` | [火山方舟控制台](https://console.volcengine.com/ark) |
| `openrouter` | [OpenRouter keys](https://openrouter.ai/settings/keys) |
| `xiaomi-mimo` | [Xiaomi MiMo Token Plan](https://platform.xiaomimimo.com/token-plan) |
| `novita` | [Novita 快速入门](https://novita.ai/docs/guides/quickstart) |
| `fireworks` | [Fireworks API keys](https://fireworks.ai/account/api-keys) |
| `siliconflow`、`siliconflow-CN` | [SiliconFlow API keys](https://cloud.siliconflow.com/account/ak) |
| `arcee` | [Arcee API key 指南](https://docs.arcee.ai/other/create-your-first-api-key) |
| `moonshot` | [Kimi 开放平台](https://platform.kimi.ai/) |
| `zai` | [Z.ai model API](https://z.ai/model-api) |
| `stepfun` | [阶跃星辰开放平台](https://platform.stepfun.ai/) |
| `minimax` | [MiniMax 先决条件](https://platform.minimax.io/docs/guides/quickstart-preparation) |
| `huggingface` | [Hugging Face tokens](https://huggingface.co/settings/tokens) |
| `deepinfra` | [DeepInfra API keys](https://deepinfra.com/dash/api_keys) |
| `together` | [Together API keys](https://api.together.ai/settings/api-keys) |
| `anthropic` | [Anthropic API keys](https://console.anthropic.com/settings/keys) |
| `openmodel` | [OpenModel API key 指南](https://docs.openmodel.ai/en/docs/guides/api-key) |
| `openai-codex` | 复用 `codex login`；不存储 CodeWhale API key。 |
| `sglang`、`vllm`、`ollama` | 本地 OpenAI 兼容端点可以在 localhost 上无需 API key 运行。 |
| `sakana` | [Sakana AI API](https://api.sakana.ai/) |
| `longcat` | [美团 LongCat 平台](https://longcat.chat/platform) |
| `meta` | [Meta Model API](https://developer.meta.com/ai/) |
| `xai` | [xAI Console](https://console.x.ai/) |

## 内置 Provider

| Provider ID | TOML 表 | 认证环境变量 | Base URL 环境变量和默认值 | 默认或静态模型 | 备注 |
| --- | --- | --- | --- | --- | --- |
| `deepseek` | `[providers.deepseek]` | `DEEPSEEK_API_KEY` | `CODEWHALE_BASE_URL` / `DEEPSEEK_BASE_URL`；默认 `https://api.deepseek.com/beta` | `deepseek-v4-pro`、`deepseek-v4-flash`；兼容性别名 `deepseek-chat`、`deepseek-reasoner` | 一等默认。Beta URL 启用严格工具模式、聊天前缀补全和 FIM。非 beta DeepSeek Chat Completions 端点仅用于医疗/法律合规路由。 |
| `deepseek-anthropic` | `[providers.deepseek_anthropic]` | `DEEPSEEK_API_KEY`（与 DeepSeek 相同） | `CODEWHALE_BASE_URL` / `DEEPSEEK_ANTHROPIC_BASE_URL`；默认 `https://api.deepseek.com/anthropic` | `deepseek-v4-pro`、`deepseek-v4-flash` | 使用 `x-api-key` 和 `anthropic-version: 2023-06-01` 的 Anthropic Messages API 路由。工具调用、系统提示和消息使用 Anthropic 格式；流式 SSE 事件也使用 Anthropic 格式。 |
| `nvidia-nim` | `[providers.nvidia_nim]` | `NVIDIA_NIM_API_KEY`、`NVIDIA_API_KEY` | `NVIDIA_NIM_BASE_URL` / `NVIDIA_BASE_URL`；默认 `https://integrate.api.nvidia.com/v1` | `deepseek-ai/deepseek-v4-pro`、`deepseek-ai/deepseek-v4-flash` | 在 NVIDIA NIM 平台上运行的 DeepSeek V4 模型的 OpenAI 兼容路由。`NVIDIA_NIM_MODEL` 被接受。DeepSeek V4 别名被规范化，非 V4 自定义模型 ID 按原样传递。内部路由经过认证的 DeepSeek Chat Completions。 |
| `openai` | `[providers.openai]` | `OPENAI_API_KEY` | `CODEWHALE_BASE_URL` / `OPENAI_BASE_URL`；默认 `https://api.openai.com/v1` | (无静态默认值；用户必须设置模型) | 标准 OpenAI Chat Completions。用于通用网关或将 OpenAI 用作后端。`OPENAI_MODEL` 被接受。 |
| `atlascloud` | `[providers.atlascloud]` | `ATLASCLOUD_API_KEY` | `ATLASCLOUD_BASE_URL`；默认 `https://api.atlascloud.ai/v1` | `deepseek-v4-pro`、`deepseek-v4-flash` | Atlas Cloud 的 OpenAI 兼容 DeepSeek 路由。`ATLASCLOUD_MODEL` 被接受。 |
| `wanjie-ark` | `[providers.wanjie_ark]` | `WANJIE_ARK_API_KEY` | `WANJIE_ARK_BASE_URL`；默认 `https://ark.wanjiedata.com/v1` | `deepseek-v4-pro`、`deepseek-v4-flash` | 万界方舟 MaaS OpenAI 兼容路由。 |
| `volcengine` | `[providers.volcengine]` | `VOLCENGINE_API_KEY` | `VOLCENGINE_BASE_URL`；默认 `https://ark.cn-beijing.volces.com/api/v3` | `deepseek-v4-pro`、`deepseek-v4-flash` | 火山方舟 OpenAI 兼容路由。`VOLCENGINE_MODEL` 被接受。 |
| `openrouter` | `[providers.openrouter]` | `OPENROUTER_API_KEY` | `OPENROUTER_BASE_URL`；默认 `https://openrouter.ai/api/v1` | `deepseek/deepseek-v4-pro`、`deepseek/deepseek-v4-flash` | OpenRouter OpenAI 兼容路由。`OPENROUTER_MODEL` 被接受。模型 ID 保留命名空间前缀（`deepseek/`、`anthropic/` 等）。 |
| `xiaomi-mimo` | `[providers.xiaomi_mimo]` | `XIAOMI_MIMO_API_KEY` | `XIAOMI_MIMO_BASE_URL`；默认 `https://api.xiaomimimo.com/v1` | `mimo-v4-pro`、`mimo-v4-flash` | 小米 MiMo OpenAI 兼容路由。`XIAOMI_MIMO_MODEL` 被接受。通过 Chat Completions 与 MiMo TTS 语音 API 共存。 |
| `novita` | `[providers.novita]` | `NOVITA_API_KEY` | `NOVITA_BASE_URL`；默认 `https://api.novita.ai/v3/openai` | `deepseek/deepseek-v4-pro`、`deepseek/deepseek-v4-flash` | Novita OpenAI 兼容路由。`NOVITA_MODEL` 被接受。 |
| `fireworks` | `[providers.fireworks]` | `FIREWORKS_API_KEY` | `FIREWORKS_BASE_URL`；默认 `https://api.fireworks.ai/inference/v1` | `accounts/fireworks/models/deepseek-v4-pro`、`accounts/fireworks/models/deepseek-v4-flash` | Fireworks OpenAI 兼容路由。`FIREWORKS_MODEL` 被接受。 |
| `siliconflow` | `[providers.siliconflow]` | `SILICONFLOW_API_KEY` | `SILICONFLOW_BASE_URL`；默认 `https://api.siliconflow.com/v1` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | SiliconFlow 国际 OpenAI 兼容路由。`SILICONFLOW_MODEL` 和 `SILICONFLOW_API_MODEL` 被接受。 |
| `siliconflow-CN` | `[providers.siliconflow_cn]` | `SILICONFLOW_CN_API_KEY` | `SILICONFLOW_CN_BASE_URL`；默认 `https://api.siliconflow.cn/v1` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | SiliconFlow 中国 OpenAI 兼容路由。`SILICONFLOW_CN_MODEL` 被接受。 |
| `arcee` | `[providers.arcee]` | `ARCEE_API_KEY` | `ARCEE_BASE_URL`；默认 `https://api.arcee.ai/v1` | `trinity-large-thinking`、`trinity-large-preview`；provider 提示的自定义模型 ID 按原样传递 | Arcee OpenAI 兼容路由。`ARCEE_MODEL` 被接受。 |
| `moonshot` | `[providers.moonshot]` | `MOONSHOT_API_KEY` | `MOONSHOT_BASE_URL`；默认 `https://api.moonshot.net/v1` | `kimi-k2.7-code`、`kimi-k2.6` | 月之暗面 Kimi OpenAI 兼容路由。`MOONSHOT_MODEL` 被接受。 |
| `zai` | `[providers.zai]` | `ZAI_API_KEY` | `ZAI_BASE_URL`；默认 `https://api.z.ai/v1` | `GLM-5.2`、`GLM-5.1`、`GLM-5-Turbo`；provider 提示的自定义模型 ID 按原样传递 | Z.ai / 智谱 GLM OpenAI 兼容路由。`ZAI_MODEL` 被接受。 |
| `stepfun` | `[providers.stepfun]` | `STEPFUN_API_KEY` | `STEPFUN_BASE_URL`；默认 `https://api.stepfun.com/v1` | `step-3.7-flash`；provider 提示的自定义模型 ID 按原样传递 | 阶跃星辰 OpenAI 兼容路由。`STEPFUN_MODEL` 被接受。 |
| `minimax` | `[providers.minimax]` | `MINIMAX_API_KEY` | `MINIMAX_BASE_URL`；默认 `https://api.minimax.io/v1` | `MiniMax-M3`、`MiniMax-M2.7`、`MiniMax-M2.7-highspeed`、`MiniMax-M2.5`、`MiniMax-M2.5-highspeed`、`MiniMax-M2.1`、`MiniMax-M2.1-highspeed`、`MiniMax-M2` | MiniMax 直接 OpenAI 兼容路由。CodeWhale 发送 `reasoning_split = true`，以便 MiniMax 的思考内容与答案文本分开到达，并且直接 MiniMax ID 与 OpenRouter 命名空间 ID（如 `minimax/minimax-m3`）保持区分。 |
| `sglang` | `[providers.sglang]` | 可选 `SGLANG_API_KEY` | `SGLANG_BASE_URL`；默认 `http://localhost:30000/v1` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | 自托管 OpenAI 兼容路由。本地部署通常省略认证。`SGLANG_MODEL` 被接受。 |
| `vllm` | `[providers.vllm]` | 可选 `VLLM_API_KEY` | `VLLM_BASE_URL`；默认 `http://localhost:8000/v1` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | 自托管 vLLM OpenAI 兼容路由。本地部署通常省略认证。`VLLM_MODEL` 被接受。 |
| `ollama` | `[providers.ollama]` | 可选 `OLLAMA_API_KEY` | `OLLAMA_BASE_URL`；默认 `http://localhost:11434/v1` | `deepseek-coder:1.3b`；provider 提示的自定义标签按原样传递 | 自托管 Ollama OpenAI 兼容路由。本地部署通常省略认证。`OLLAMA_MODEL` 被接受。 |
| `huggingface` | `[providers.huggingface]` | `HUGGINGFACE_API_KEY`、`HF_TOKEN` | `HUGGINGFACE_BASE_URL`、`HF_BASE_URL`；默认 `https://router.huggingface.co/v1` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | Hugging Face Inference Providers OpenAI 兼容路由器路由。接受的别名：`huggingface`、`hugging-face`、`hugging_face`、`hf`。组织前缀的模型 ID 按原样传递。`HUGGINGFACE_MODEL` 和 `HF_MODEL` 被接受。Hub 浏览/导出是单独的未来功能。 |
| `deepinfra` | `[providers.deepinfra]` | `DEEPINFRA_API_KEY`、`DEEPINFRA_TOKEN` | `DEEPINFRA_BASE_URL`；默认 `https://api.deepinfra.com/v1/openai` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | DeepInfra OpenAI 兼容路由。OpenAI SDK 的直接替代。 |
| `together` | `[providers.together]` | `TOGETHER_API_KEY` | `TOGETHER_BASE_URL`；默认 `https://api.together.xyz/v1` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | Together AI OpenAI 兼容路由。`TOGETHER_MODEL` 被接受。模型别名 `deepseek-v4-pro` 和 `deepseek-v4-flash` 规范化为 Together 的组织前缀 ID。 |
| `qianfan` | `[providers.qianfan]` | `QIANFAN_API_KEY`、`BAIDU_QIANFAN_API_KEY` | `QIANFAN_BASE_URL`、`BAIDU_QIANFAN_BASE_URL`；默认 `https://api.baiduqianfan.ai/v1` | `ernie-4.0-turbo-8k`；provider 范围内的自定义千帆服务/模型 ID 按原样传递 | 百度千帆 OpenAI 兼容路由。请求使用 Bearer 认证和 Chat Completions 负载。`QIANFAN_MODEL` 和 `BAIDU_QIANFAN_MODEL` 被接受；别名 `baidu-qianfan`、`baidu_qianfan` 和 `baidu` 解析到此 provider。工具/函数调用在千帆文档中是模型范围的，因此 CodeWhale 保留选定的通信模型，并将实时能力证明留给后续的路由/能力工作。 |
| `openai-codex` | `[providers.openai_codex]` | 通过 `codex login` 的 OAuth（`~/.codex/auth.json`）；环境变量覆盖 `OPENAI_CODEX_ACCESS_TOKEN`、`CODEX_ACCESS_TOKEN` | `OPENAI_CODEX_BASE_URL`/`CODEX_BASE_URL`；默认 `https://chatgpt.com/backend-api` | `gpt-5.5` | **实验性。** 复用你现有的 ChatGPT/Codex CLI OAuth 登录，并通过 `/codex/responses` 与 OpenAI Responses API 通信。访问令牌从 `~/.codex/auth.json` 读取和刷新；不存储 API key。`OPENAI_CODEX_MODEL`/`CODEX_MODEL` 和 `OPENAI_CODEX_ACCOUNT_ID`/`CODEX_ACCOUNT_ID` 被接受。CodeWhale 使用 400K Codex 系列有效上下文窗口为此路由做预算，即使公共 API 模型表列出了更大的原生 `gpt-5.5` 窗口。 |
| `anthropic` | `[providers.anthropic]` | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL`；默认 `https://api.anthropic.com` | `claude-opus-4-8`、`claude-sonnet-4-6`、`claude-haiku-4-5` | 原生 Anthropic Messages API。原生 Anthropic 流式 SSE 事件。不通过 OpenAI Chat Completions 隧道传输。 |
| `openmodel` | `[providers.openmodel]` | `OPENMODEL_API_KEY` | `OPENMODEL_BASE_URL`；默认 `https://api.openmodel.ai` | `deepseek-v4-flash`；provider 范围内的自定义模型 ID 按原样传递 | OpenModel Anthropic Messages 路由。默认使用 Messages 负载模式；可通过 `request_payload_mode` 覆盖到 `AnthropicMessages`。 |
| `sakana` | `[providers.sakana]` | `SAKANA_API_KEY` | `SAKANA_BASE_URL`；默认 `https://api.sakana.ai/v1` | `fugu`、`fugu-ultra-20260615` | Sakana AI OpenAI 兼容路由。 |
| `longcat` | `[providers.longcat]` | `LONGCAT_API_KEY` | `LONGCAT_BASE_URL`；默认 `https://api.longcat.chat/v1` | `LongCat-2.0` | 美团 LongCat OpenAI 兼容路由。 |
| `meta` | `[providers.meta]` | `META_API_KEY` | `META_BASE_URL`；默认 `https://api.meta.com/v1` | `muse-spark-1.1` | Meta Model API OpenAI 兼容路由。 |
| `xai` | `[providers.xai]` | `XAI_API_KEY` | `XAI_BASE_URL`；默认 `https://api.x.ai/v1` | `grok-4.5`、`grok-4.3`、`grok-build`、`grok-composer-2.5-fast`、`grok-4.20-0309-reasoning`、`grok-4.20-0309-non-reasoning` | xAI Grok OpenAI 兼容路由。 |

## 模型注册表

每个 provider 的已知静态模型：

| Provider ID | 静态模型 | 工具/函数调用 | 推理/思考支持 |
| --- | --- | --- | --- |
| `deepseek` | `deepseek-v4-pro`、`deepseek-v4-flash`；兼容性别名 `deepseek-chat`、`deepseek-reasoner` | yes | yes |
| `nvidia-nim` | `deepseek-ai/deepseek-v4-pro`、`deepseek-ai/deepseek-v4-flash` | yes | yes |
| `openai` | (无静态默认值；用户必须设置模型) | yes | 模型相关 |
| `atlascloud` | `deepseek-v4-pro`、`deepseek-v4-flash` | yes | yes |
| `wanjie-ark` | `deepseek-v4-pro`、`deepseek-v4-flash` | yes | 模型相关 |
| `volcengine` | `deepseek-v4-pro`、`deepseek-v4-flash` | yes | yes |
| `openrouter` | `deepseek/deepseek-v4-pro`、`deepseek/deepseek-v4-flash` | yes | yes |
| `xiaomi-mimo` | `mimo-v4-pro`、`mimo-v4-flash` | yes | yes |
| `novita` | `deepseek/deepseek-v4-pro`、`deepseek/deepseek-v4-flash` | yes | yes |
| `fireworks` | `accounts/fireworks/models/deepseek-v4-pro`、`accounts/fireworks/models/deepseek-v4-flash` | yes | yes |
| `siliconflow`、`siliconflow-CN` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `arcee` | `trinity-large-thinking`、`trinity-large-preview`；provider 提示的自定义模型 ID 按原样传递 | yes | `trinity-large-thinking` 支持 yes；`trinity-large-preview` 不支持 |
| `moonshot` | `kimi-k2.7-code`、`kimi-k2.6` | yes | yes |
| `zai` | `GLM-5.2`、`GLM-5.1`、`GLM-5-Turbo`；provider 提示的自定义模型 ID 按原样传递 | yes | yes |
| `stepfun` | `step-3.7-flash` | yes | no |
| `minimax` | `MiniMax-M3`、`MiniMax-M2.7`、`MiniMax-M2.7-highspeed`、`MiniMax-M2.5`、`MiniMax-M2.5-highspeed`、`MiniMax-M2.1`、`MiniMax-M2.1-highspeed`、`MiniMax-M2` | yes | yes |
| `sglang` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `vllm` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `ollama` | `deepseek-coder:1.3b`；当 provider 提示为 `ollama` 时自定义标签按原样传递 | yes | no |
| `huggingface` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | yes | no |
| `deepinfra` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `together` | `deepseek-ai/DeepSeek-V4-Pro`、`deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `openai-codex` | `gpt-5.5` | yes | yes |
| `anthropic` | `claude-opus-4-8`、`claude-sonnet-4-6`、`claude-haiku-4-5` | yes | `claude-opus-4-8` 和 `claude-sonnet-4-6` 支持 yes；`claude-haiku-4-5` 不支持 |
| `openmodel` | `deepseek-v4-flash`；provider 范围内的自定义模型 ID 按原样传递 | yes | 模型相关 |
| `sakana` | `fugu`、`fugu-ultra-20260615` | yes | `fugu-ultra-20260615` 支持 yes |
| `longcat` | `LongCat-2.0` | yes | yes |
| `meta` | `muse-spark-1.1` | yes | yes |
| `xai` | `grok-4.5`、`grok-4.3`、`grok-build`、`grok-composer-2.5-fast`、`grok-4.20-0309-reasoning`、`grok-4.20-0309-non-reasoning` | yes | `grok-4.5`、`grok-4.3`、`grok-build` 和 `grok-4.20-0309-reasoning` 支持 yes |

AtlasCloud 保持与配置层相同的默认模型，并为 Pro 和 Flash 行添加 provider 范围内的别名。其他 AtlasCloud 模型 ID 仍应通过 `ATLASCLOUD_MODEL`、配置或在可用时通过实时模型列表来选择。

## 能力元数据

`codewhale-tui doctor --json` 暴露 `capability` 对象。它是静态元数据，而非实时 API 探测。当前字段包括：

`resolved_provider`、`resolved_model`、`context_window`、`max_output`、
`thinking_supported`、`cache_telemetry_supported` 和 `request_payload_mode`。

大多数内置 provider 使用 Chat Completions 请求负载模式。原生
Anthropic 和 OpenModel 使用 Messages，`openai-codex` 使用 Responses。

对于实际窗口与静态表不同的 OpenAI 兼容网关或自托管运行时，设置 `[providers.<name>] context_window = N`。
配置的值成为提示、上下文压力检查、压缩和输出容量预算的路由有效上下文窗口。

| Provider/模型类别 | 上下文窗口 | 最大输出元数据 | 推理支持 | 缓存遥测 | FIM 端点 |
| --- | --- | --- | --- | --- | --- |
| DeepSeek V4（`deepseek-v4-pro`、`deepseek-v4-flash`） | 1,000,000 | 384,000 | yes | yes | 仅 DeepSeek beta |
| DeepSeek 兼容性别名（`deepseek-chat`、`deepseek-reasoner`） | 1,000,000 | 384,000 | yes | yes | 仅 DeepSeek beta |
| NVIDIA NIM V4 注册表模型 | 1,000,000 | 384,000 | yes | yes | 代码中未记录 |
| 火山方舟 V4 模型 ID | 1,000,000 | 384,000 | yes | yes | 代码中未记录 |
| OpenRouter、Novita、Fireworks、SiliconFlow、SGLang 和 vLLM V4 模型 ID | 1,000,000 | 384,000 | yes | no | 代码中未记录 |
| Moonshot Kimi 模型 | 256,000 | 16,384 | yes | no | 不可用 |
| Z.ai GLM 模型 | 1,000,000 | 384,000 | yes | no | 不可用 |
| 阶跃星辰 step-3.7-flash | 256,000 | 32,768 | no | no | 不可用 |
| MiniMax M 系列 | 1,000,000 | 384,000 | yes | no | 不可用 |
| Arcee Trinity 模型 | 256,000 | 32,768 | 模型相关 | no | 不可用 |
| Ollama 模型 | 131,072 | 131,072 | no | no | 不可用 |
| HuggingFace 路由器模型 | 1,000,000 | 384,000 | no | no | 不可用 |
| DeepInfra DeepSeek 模型 | 1,000,000 | 384,000 | yes | no | 不可用 |
| Together AI DeepSeek 模型 | 1,000,000 | 384,000 | yes | no | 不可用 |
| 千帆 ERNIE 模型 | 8,000 | 2,048 | no | no | 不可用 |
| OpenAI Codex `gpt-5.5` | 400,000（Codex 系列有效窗口） | 128,000 | yes | no | 不可用 |
| Anthropic Claude 模型 | 200,000 | 32,768 | yes | no | 不可用 |
| OpenModel | 1,000,000 | 384,000 | 模型相关 | no | 不可用 |
| Sakana AI | 262,144 | 65,536 | 模型相关 | no | 不可用 |
| LongCat | 1,000,000 | 384,000 | yes | no | 不可用 |
| Meta muse-spark-1.1 | 256,000 | 32,768 | yes | no | 不可用 |
| xAI Grok 模型 | 1,000,000 | 128,000 | 模型相关 | no | 不可用 |
| 小米 MiMo V4 模型 | 262,144 | 65,536 | yes | no | 不可用 |
| 万界方舟 V4 模型 | 1,000,000 | 384,000 | no | no | 代码中未记录 |
| Atlas Cloud V4 模型 | 1,000,000 | 384,000 | yes | yes | 代码中未记录 |

## 推理/思考配置

推理模式与 provider 的能力交叉。当模型被设置为 `auto` 并且目标模型支持推理时，TUI 默认关闭推理。`codewhale --reasoning-effort high` 或 `/reasoning high` 在支持推理的模型上启用它。

下表显示了每个推理 effort 等级下发送到 provider 的字段。标记为 `omitted` 的单元格表示 CodeWhale 完全不发送推理字段——对于不支持推理的 provider，没有推理配置的模型会获得干净的请求。对于按模型区分推理支持的 provider（例如 `arcee`），如果所选模型不支持推理，请求中会省略 `reasoning_effort` 和 `thinking` 对象。

| Provider | `off` | `low`/`medium`/`high` | `max`/`xhigh` |
| --- | --- | --- | --- |
| `deepseek`、`deepseek-cn`、`siliconflow`、`siliconflow-CN`、`sglang`、`volcengine`、`atlascloud` | `thinking: {type: disabled}` | `reasoning_effort: "high"` + `thinking: {type: enabled}` | `reasoning_effort: "max"` + `thinking: {type: enabled}` |
| `openrouter`、`novita`、`together` | `thinking: {type: disabled}` | `reasoning_effort` 透传 + `thinking: {type: enabled}` | `reasoning_effort: "xhigh"` + `thinking: {type: enabled}` |
| `moonshot` | `thinking: {type: disabled}` | `thinking: {type: enabled}` | `thinking: {type: enabled}` |
| `ollama` | `think: false` | `think: true` | `think: true` |
| `xiaomi-mimo` | `thinking: {type: disabled}` | `thinking: {type: enabled}` | `thinking: {type: enabled}` |
| `minimax` | `reasoning_split: true` + `thinking: {type: disabled}` | `reasoning_split: true` + `thinking: {type: adaptive}` | `reasoning_split: true` + `thinking: {type: adaptive}` |
| `nvidia-nim` | `chat_template_kwargs.thinking: false` | `chat_template_kwargs`：`thinking: true` + `reasoning_effort: "high"` | `chat_template_kwargs`：`thinking: true` + `reasoning_effort: "max"` |
| `vllm` | `chat_template_kwargs.enable_thinking: false` | `chat_template_kwargs.enable_thinking: true` + `reasoning_effort` low/medium/high | `chat_template_kwargs.enable_thinking: true` + `reasoning_effort: "high"`（vLLM 没有 max 等级） |
| `arcee`、`huggingface` | omitted | `reasoning_effort` 透传 | `reasoning_effort: "high"` |
| `fireworks` | omitted | `reasoning_effort: "high"` | `reasoning_effort: "max"` |
| `openai`、`wanjie-ark` | omitted | omitted | omitted |
| `openmodel` | Anthropic Messages 适配器处理推理/输出配置 | Anthropic Messages 适配器处理推理/输出配置 | Anthropic Messages 适配器处理推理/输出配置 |
| `openai-codex` | Responses API `reasoning` 字段（由 Responses 桥处理） | Responses API `reasoning` 字段 | Responses API `reasoning` 字段 |

AtlasCloud 提供 DeepSeek 模型服务，因此它使用 DeepSeek 推理方言，包括 `max` 等级（#3024）。

## 漂移检查

在更改 provider ID、provider TOML 表、静态模型注册表行或 provider 默认字符串之前运行：

```bash
python3 scripts/check-provider-registry.py
```

检查在以下情况失败：

- `docs/PROVIDERS.md` 遗漏了规范的 `ProviderKind::as_str()` ID。
- `crates/tui/src/config.rs` 的 `ApiProvider::as_str()` 与
  `ProviderKind::as_str()` 偏离，除非是显式的 `deepseek-cn` 旧版别名。
- 内置 provider 表遗漏或添加了 `[providers.*]` TOML 表。
- 静态模型注册表表与 `crates/agent/src/lib.rs` 使用的 provider 偏离。
- `crates/tui/src/config.rs` 中的 provider 默认模型或 base URL 常量在此处不再被提及。

## 已规划，尚未发布

以下项目属于 v0.8.48+ provider 抽象里程碑或相关的 provider 文档工作，但它们不是此检出的原生内置行为：

- `codewhale-agent` 中的统一 `Provider` trait，负责环境变量优先级、
  密钥解析、base URL 规范化、认证头构建和
  provider 元数据。这些职责目前仍分散在
  `crates/config`、`crates/secrets` 和 `crates/tui/src/client.rs` 中。
- 选择器中的 Hugging Face 模型护照元数据，包括许可证、基础
  模型、上下文长度、聊天模板、工具调用支持、推理支持
  和门控/私有状态。
