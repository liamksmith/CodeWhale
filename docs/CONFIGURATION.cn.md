# 配置

codewhale 从 TOML 文件和环境变量中读取配置。
进程启动时，如果存在工作区本地的 `.env` 文件也会加载。
使用受跟踪的 `.env.example` 作为模板；将其复制为 `.env`，然后只编辑
您需要的提供者和安全选项。

## 宪法、项目指令和仓库权威

CodeWhale 有多个指令层。它们被有意分开，以便
个人宪法、仓库策略、项目指令和运行时安全
控制不会混在一起。

- **捆绑的全局宪法** — 二进制文件中编译的基本法则。它是
  每个会话的默认底线。
- **用户全局宪法** — 正常的引导设置输出。通过
  `/constitution` 或 `/setup` 管理；CodeWhale 在
  `$CODEWHALE_HOME/constitution.json`（默认 `~/.codewhale/constitution.json`）中存储结构化数据，
  并将其渲染到单独的 `<codewhale_user_constitution>` 散文块中。
  这可以表达偏好和停止条件，但不会更改
  运行时批准策略、沙箱、shell、网络、信任或 MCP 权限。
- **仓库本地宪法** — `.codewhale/constitution.json` 中的可选项目策略，
  如下所述。
- **`AGENTS.md`** — 跨代理的**项目指令**（散文）。这是
  "代理应如何在此仓库中工作"的权威文件。运行 `/init` 搭建一个。
  `CLAUDE.md` 和 `.claude/instructions.md` 作为兼容回退被读取。
- **记忆和交接** — 召回的状态。有用，但权威性低于
  宪法和项目指令。

这些层面的发布验证位于
[`docs/evidence/v0867-constitution-setup-qa-matrix.md`](evidence/v0867-constitution-setup-qa-matrix.md)。
在检查 `/setup`、`/constitution`、doctor、上下文报告和
更新检查点是否一致时使用它。

### 管理用户全局宪法（`/setup` 和 `/constitution`）

首次启动时，CodeWhale 运行一个简短的**宪法优先**设置路径：
语言 → 提供者/模型就绪 → 运行时姿态 → 创建或确认您的
宪法。捆绑/默认宪法始终有效，因此您可以
推迟；随时通过 `/setup` 重新打开中心。

在**宪法**步骤中：

- **`1`**–**`6`** 调整引导草案。**`G`** 预览它，再次按 **`G`** 
  批准并保存新的结构化 `constitution.json`。
- **`A`**（仅在已配置提供者时显示）要求您的第一个已配置
  模型起草宪法。起草**不是**保存：草案通过
  相同的预览渲染，您仍需按 **`G`** 批准
  才能持久化。
- **`K`** 保持现有已加载的宪法不变（仅在已有
  有效文件时显示）。
- **`U`**（或 `/constitution bundled`）记录捆绑/默认法则。

`/constitution`（别名 `/law`）是设置后的主要管理界面。
子命令：`status`（默认）、`preview`、`review`、`repo`（
仓库本地法则块）、`explain`、`edit`/`guided`、`repair`、`posture` 和
`bundled`。管理宪法永远不会更改运行时批准、沙箱、
shell、网络、信任、默认模式或 MCP 权威 — 这些保留在运行时
姿态/配置中。

每个仓库可以携带两个不同但互补的文件：

- **`AGENTS.md`** — 普通项目工作指令。
- **`.codewhale/constitution.json`** — CodeWhale 特定的**仓库权威 /
  优先级策略**：当本地来源冲突时，CodeWhale 应首先信任哪个，
  以及在声称任务完成前要验证什么。`.codewhale/`
  位于仓库内（像 `.github/`）。示例：

  ```json
  {
    "schema_version": 1,
    "authority": [
      "current user request",
      "live code and tests",
      "GitHub issue/PR details",
      "AGENTS.md",
      "memory",
      "old handoffs"
    ],
    "protected_invariants": [
      "do not break old-session transcript replay"
    ],
    "branch_policy": "PRs target the integration branch, not main",
    "verification_policy": {
      "before_claiming_done": ["run focused tests", "read changed files back"]
    },
    "escalate_when": [
      "a destructive action was not explicitly authorized"
    ]
  }
  ```

  所有字段都是可选的。当存在时，该文件以更高的权威块
  渲染为系统提示中的简洁散文。旧版 `WHALE.md` 文件
  被忽略并报告为仅用于迁移的诊断。

  每个 `protected_invariants` 条目可以是纯字符串（建议性
  散文，历史形状）或携带路径通配符的对象，后者
  还会在工具门控中**机械地强制执行**。参见
  下方的[强制执行的仓库法则不变量](#强制执行的仓库法则不变量)。

  这是 CodeWhale 层级中的**仓库本地法则**层：*捆绑全局
  宪法* → *用户全局宪法*（`$CODEWHALE_HOME/constitution.json`，
  渲染为散文）→ *仓库宪法*（`.codewhale/constitution.json`，此
  文件）→ *AGENTS/项目指令* → *记忆和交接* → *当前
  请求和活跃轮次的实时证据*。运行时策略
  （权限/沙箱/成本限制在代码中强制执行）与所有这些
  提示层是分开的。仓库宪法提供项目决策规则；它
  不替代捆绑宪法、用户全局宪法或
  当前用户请求。

> **`WHALE.md` 已弃用。** 它与 `AGENTS.md` 有令人困惑的重叠。
> CodeWhale 不再将 `WHALE.md` 读取为项目或全局上下文。如果存在，
> 设置/上下文诊断会报告它被忽略，以便您可以迁移它。
> 将普通指令移到 `AGENTS.md`，将 CodeWhale 特定的权威
> 策略移到 `.codewhale/constitution.json`。个人持久指导属于
> `/constitution` / `$CODEWHALE_HOME/constitution.json`。（在模型提示中
> 附带的全局 CodeWhale 宪法是一个独立的东西，
> 不受影响。）

### 强制执行的仓库法则不变量

默认情况下，`protected_invariants` 条目是建议性散文：它被渲染到
提示中作为代理应遵循的指导，但没有东西阻止写入。一个
写成**带有 `paths` 的对象**的条目则不同 — 它编译成
引擎工具门控在写入运行前评估的机械写入阻止。法则变为
机制，而不仅仅是请求。

强制执行的条目具有此形状：

```json
{
  "schema_version": 1,
  "protected_invariants": [
    "Keep DeepSeek support first-class.",
    {
      "text": "The wire format is frozen; protocol changes need a human.",
      "paths": ["crates/protocol/**"],
      "action": "block"
    },
    {
      "text": "Release notes need human review.",
      "paths": ["CHANGELOG.md"],
      "action": "ask"
    }
  ]
}
```

- `text` — 必填。在阻止时显示的原因。空的 `text` 被跳过。
- `paths` — 工作区相对通配符（globset 语法，例如 `crates/protocol/**`、
  `**/secrets.toml`、`CHANGELOG.md`）。没有可用 `paths` 的对象尽管是对象形状，
  仍保持仅建议性。
- `action` — 可选，默认 `ask`。`ask` **强制提示**批准；
  `block` **直接拒绝写入**。

语义：

- **仅收紧。** 模式没有允许/放宽的形状，因此法则只能*添加*
  阻止 — 精心制作的宪法永远不能授予权限或削弱其上方的门控。
- **不可通过模式绕过。** 像内置安全底线一样，`ask` 阻止
  在每个模式（包括 YOLO）中强制提示；`block` 始终拒绝。模式
  不能关闭阻止。
- **仅仓库本地。** 只有仓库的 `.codewhale/constitution.json`
  参与。用户全局宪法保持建议性散文，永远不会
  到达此机制。
- **安全失败。** 缺失文件、解析错误或无效通配符会降级为
  更少或零规则 — 永远不会阻止未受保护的路径，永远不会毒化
  门控。跨匹配最强的操作胜出，因此 `block` 优先于 `ask`。
- **留下收据。** 每个阻止发出一个 `tool.repo_law_decision` 工具审计
  事件，命名不变量、匹配的路径和源文件；
  批准/拒绝原因也命名不变量。

**覆盖范围有意受限。** 阻止仅对写入
工具 `write_file`、`edit_file`、`apply_patch` 和 `fim_edit` 进行评估，并且仅
针对其输入中命名的文件系统目标（`path`/`target`/
`destination`/`file_path`、`changes[].path` 和 unified-diff /
`apply_patch` 信封头）。写入受保护路径的 shell 命令**不**被仓库法则阻止 —
 这些写入仍受普通批准、沙箱
 和 shell 写入门控的约束，而不受此机制约束。

### 专家完整基础提示覆盖（#3638）

全局宪法（基础系统提示，通常从
`prompts/constitution.md` 编译进来）可以在不重新构建的情况下按用户替换。这是
一个专家级逃生舱口，不是正常的 `/constitution` 引导设置输出。
因为这是一个提示信任边界，需要**两个有意步骤** — 仅有
文件是不够的：

1. 将替换内容放在 `~/.codewhale/prompts/constitution.md`（当设置
   `$CODEWHALE_HOME` 时在其下）。
2. 设置显式选择加入标志 `CODEWHALE_ALLOW_BASE_PROMPT_OVERRIDE=1`
   （`true`/`on`/`yes` 也被接受）。

如果文件存在但标志未设置，覆盖被**忽略**（带有
指向标志的日志行），捆绑宪法保持原位。
这旨在用于将 TUI 重新用于软件工程之外的用途 — 例如
长篇写作或文档审查 — 其中面向工程的基础
提示不适合。它在启动时加载一次；**缺失或空文件
是无操作的**，因此现有安装保持捆绑提示。

范围有意狭窄：只有字节稳定的**基础提示段**是
可覆盖的。模式增量、批准策略、工具分类、上下文
管理和压缩中继仍由 CodeWhale 的运行时
组装拥有，因此覆盖**不能移除安全相关指导**（沙箱、
批准）— 它只交换任务/声音框架。要自定义普通
个人行为，首选 `/constitution`；要自定义每个仓库的行为，
首选上面的 `AGENTS.md` + `.codewhale/constitution.json`。

## 查找位置

默认配置路径：

- `~/.codewhale/config.toml`
- 旧版回退：`~/.deepseek/config.toml`

覆盖：

- CLI：`codewhale --config /path/to/config.toml`
- 环境变量：`CODEWHALE_CONFIG_PATH=/path/to/config.toml`
- 旧版环境变量别名：`DEEPSEEK_CONFIG_PATH=/path/to/config.toml`

如果两者都设置，`--config` 优先。环境变量覆盖在文件加载后应用。

### TUI 可编辑性审计

在 TUI 中，运行 `/config audit` 查看哪些文档化的键可以从
当前会话更改，哪些也可以持久化，哪些保持
仅文件或仅重启。审计包括高影响
运行时控制的当前值，例如 `approval_policy`、`allow_shell`、
`stream_chunk_timeout_secs`、`base_url`、`mcp_config_path` 和
`[subagents]` 并发/深度/超时键。

在手动编辑之前，使用命令的"Command / reason"列作为事实来源。
例如，`/config approval_mode on-request --save` 写入
顶级 `approval_policy = "on-request"`，而提供者基础 URL 被保存
但仍需要重启模型客户端。

### 用户工作区条目

交互式 Agent 会话默认暴露带批准门控的 shell 工具，
除非您显式禁用它们。对于应存在于
用户全局配置中的 shell 选择加入（用于非交互或持久任务配置文件），
添加工作区作用域条目：

```toml
[workspace.'/absolute/path/to/project']
allow_shell = true
```

该条目仅在启动的工作区路径匹配表键时应用。
旧版 `[projects."/absolute/path/to/project"]` 表也被接受用于
此用户拥有的覆盖。

在交互模式下，每个项目的覆盖
`<workspace>/.codewhale/config.toml` 在此用户条目之后应用。项目级别的
`allow_shell = false` 仍可收紧会话；项目级别的
`allow_shell = true` 被忽略。

### 每个项目的覆盖（#485）

当 TUI 在包含常规文件
`<workspace>/.codewhale/config.toml` 的工作区中启动时，该文件中声明的安全值
会合并在全局配置之上。当 CodeWhale 路径
不存在时，旧版 `<workspace>/.deepseek/config.toml` 文件仍被读取。符号链接的项目配置文件被拒绝。这让仓库可以建议
模型或收紧本地安全姿态，而无需触及用户的
`~/.codewhale/config.toml`。传递 `--no-project-config` 跳过一次启动的覆盖。

项目覆盖中支持的键（仅顶级字段）：

| 键 | 效果 |
|---|---|
| `model` | 覆盖 `default_text_model` |
| `reasoning_effort` | 为复杂仓库强制 `"high"` / `"max"` |
| `approval_policy` | 仅收紧用户当前批准姿态的值 |
| `sandbox_mode` | 仅收紧用户当前沙箱姿态的值 |
| `notes_path` | 将笔记保留在仓库中 |
| `max_subagents` | 为受限仓库限制子代理并发（限制为 1..=20） |
| `allow_shell` | `false` 可禁用 shell 访问；`true` 被忽略 |

覆盖有意狭窄 — 它涵盖仓库
维护者最可能希望跨贡献者标准化的字段。
凭据、端点、提供者选择、MCP 配置、钩子、技能、容量、
重试、hotbar 绑定和 `instructions = [...]` 设置保持用户全局。
如果仓库本地配置声明了 `api_key`、`base_url`、`provider`、
`mcp_config_path`、`hotbar`、`allow_shell = true` 或 `instructions`，
CodeWhale 忽略该键并保留用户的全局设置。

`codewhale` 外观和 `codewhale-tui` 二进制文件共享相同的配置文件用于
DeepSeek 认证和模型默认值。`codewhale auth set --provider deepseek`（以及
旧版 `codewhale login --api-key ...` 别名）将密钥保存到
`~/.codewhale/config.toml`（在需要时首次启动迁移旧版 `~/.deepseek/config.toml`），
`codewhale --model deepseek-v4-flash` 作为 `DEEPSEEK_MODEL` 转发到 TUI。

凭据查找在显式 CLI `--api-key` 之后使用 `config -> keyring -> env`。运行 `codewhale auth status` 检查活跃提供者的配置文件、
操作系统密钥环后端、环境变量、获胜来源和最后四位
标签，而不打印密钥本身。该命令仅探测活跃
提供者的密钥环条目。

对于托管、通用 OpenAI 兼容、自托管、OpenAI Responses 或原生
Anthropic 提供者，设置 `provider = "<id>"` 或传递
`codewhale --provider <id>`。权威提供者 ID 为 `deepseek`、
`nvidia-nim`、`openai`、`atlascloud`、`wanjie-ark`、`volcengine`、
`openrouter`、`xiaomi-mimo`、`novita`、`fireworks`、`siliconflow`、`arcee`、
`siliconflow-CN`、`moonshot`、`sglang`、`vllm`、`ollama`、`huggingface`、
`together`、`qianfan`、`openai-codex`、`anthropic`、`openmodel`、`zai`、
`stepfun`、`minimax` 和 `deepinfra`。
对于每个提供者的注册表，包括线路协议、认证变量、
默认基础 URL、模型 ID 和能力元数据，参见
[PROVIDERS.md](PROVIDERS.md)。
外观将提供者凭据保存到共享用户配置，并将
解析后的密钥、基础 URL、提供者和模型转发到 TUI 进程。使用
`codewhale auth set --provider nvidia-nim --api-key "YOUR_NVIDIA_API_KEY"` 或
`codewhale auth set --provider openai --api-key "YOUR_OPENAI_COMPATIBLE_API_KEY"` 或
`codewhale auth set --provider atlascloud --api-key "YOUR_ATLASCLOUD_API_KEY"` 或
`codewhale auth set --provider wanjie-ark --api-key "YOUR_WANJIE_API_KEY"` 或
`codewhale auth set --provider xiaomi-mimo --api-key "YOUR_XIAOMI_KEY"` 或
`codewhale auth set --provider fireworks --api-key "YOUR_FIREWORKS_API_KEY"` 或
`codewhale auth set --provider siliconflow --api-key "YOUR_SILICONFLOW_API_KEY"` 或
`codewhale auth set --provider arcee --api-key "YOUR_ARCEE_API_KEY"` 或
[PROVIDERS.md](PROVIDERS.md) 中匹配的提供者 ID 来通过外观保存提供者密钥。通用 `openai` 提供者默认
为 `https://api.openai.com/v1`，接受 `OPENAI_BASE_URL`，并为 OpenAI 兼容网关默认
使用 `deepseek-v4-pro`。`atlascloud` 默认为
`https://api.atlascloud.ai/v1`，接受 `ATLASCLOUD_BASE_URL`，并使用
`deepseek-ai/deepseek-v4-flash` 作为其默认模型。`wanjie-ark` 目标为
万界 Ark 在 `https://maas-openapi.wanjiedata.com/api/v1` 的 OpenAI 兼容端点，
默认为 `deepseek-reasoner`，并按原样传递模型 ID，因为万界模型访问是
账户作用域的。SGLang、vLLM 和 Ollama 是
自托管的，默认可以在没有 API 密钥的情况下运行。Ollama 默认为
`http://localhost:11434/v1`，并按原样发送模型标签，如 `codewhale-coder:1.3b`
或 `qwen2.5-coder:7b`。自托管提供者和本地回环自定义
URL（`localhost`、`127.0.0.1`、`[::1]`、`0.0.0.0`）不读取密钥存储，
除非显式请求 API 密钥认证；当本地服务器确实需要 bearer 认证时，
使用环境变量或配置文件密钥。
SiliconFlow 默认为 `https://api.siliconflow.com/v1`，接受
`SILICONFLOW_BASE_URL`，默认使用 `deepseek-ai/DeepSeek-V4-Pro`。
`provider = "siliconflow-CN"` 选择中国区域默认
`https://api.siliconflow.cn/v1` 与 `[providers.siliconflow_cn]` 表和
`SILICONFLOW_API_KEY` 凭据槽。
Arcee AI 默认为 `https://api.arcee.ai/api/v1`，接受 `ARCEE_BASE_URL`，
并为 CodeWhale 代理工作默认使用 `trinity-large-thinking`。
`trinity-large-preview` 也被列为直接 Arcee API 模型；OpenRouter 的
`arcee-ai/trinity-large-thinking` 保持为 OpenRouter 命名空间形式，而
直接 Arcee 提供者使用裸 `trinity-large-thinking` ID。直接
Arcee 大模型 API 调用被跟踪为 256K 上下文 BF16 服务；Thinking
具有推理能力，而 Preview 未被标记为思考模型。

### 自定义 OpenAI 兼容网关

对于实现 OpenAI Chat Completions
API 的单个第三方服务，最简单的设置是将内置的 `openai` 提供者名称指向
网关：

```toml
provider = "openai"
default_text_model = "your-model-id"

[providers.openai]
api_key = "YOUR_OPENAI_COMPATIBLE_API_KEY"
base_url = "https://your-gateway.example/v1"
```

将端点放在 `[providers.openai]` 下，而不是旧版顶级
`base_url`，以便 OpenAI 兼容提供者接收它。`default_text_model`
是发送到网关的模型 ID；`[providers.openai].model` 可用作
OpenAI 提供者特定的覆盖。

如果您保留多个 OpenAI 兼容网关，或需要一个稳定的名称用于
AgentProfile 提供者固定，定义一个用户命名的自定义提供者表：

```toml
provider = "lm-studio"

[providers.lm-studio]
kind = "openai-compatible"
base_url = "http://127.0.0.1:1234/v1"
api_key = "lm-studio"
model = "qwen-2.5-7b"
```

自定义提供者名称可以使用 `provider = "<name>"`、
`--provider <name>` 或当匹配的 `[providers.<name>]` 表存在时
通过 AgentProfile `provider = "<name>"` 选择。

StepFun 有第一类提供者条目，因此保持 Coding Plan 凭据和
基础 URL 作用域为 `[providers.stepfun]`：

```toml
provider = "stepfun"

[providers.stepfun]
api_key = "YOUR_STEPFUN_API_KEY"
base_url = "https://api.stepfun.com/step_plan/v1"
model = "step-3.7-flash"
```

阿里云百炼 / Model Studio DashScope Qwen 路由使用相同的 OpenAI
提供者形状：

```toml
provider = "openai"

[providers.openai]
api_key = "YOUR_DASHSCOPE_API_KEY"
base_url = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
model = "qwen-plus"
context_window = 1000000
```

使用与您的 API 密钥区域匹配的区域 DashScope `compatible-mode/v1` 基础 URL。
CodeWhale 将 `qwen-plus` 保持在 `openai`
提供者路由作用域内，并且不从模型前缀推断不同的提供者。
相同的规则适用于所有提供者前缀的模型字符串：诸如
`deepseek-ai/...` 或 `deepseek/...` 的前缀是选定提供者下的提供者拥有的线路 ID，
而不是自动切换到 DeepSeek 提供者。
当网关/模型的实际总上下文窗口与 CodeWhale 的静态模型元数据不同时，
设置 `context_window`。

如果网关接受 `POST /chat/completions` 但拒绝
`/v1/chat/completions`，设置提供者本地的 `path_suffix`：

```toml
[providers.openai]
base_url = "https://your-gateway.example/v1"
path_suffix = "/chat/completions"
```

后缀仅适用于聊天补全请求。模型列表和
DeepSeek beta 路径保留其内置路由，以便通用网关覆盖
不会意外改写 `/models` 或 `/beta/completions`。

对于具有损坏或被拦截证书的私有网关，使用
`SSL_CERT_FILE` 与受信任的 CA 捆绑包。旧版提供者表键
`insecure_skip_tls_verify = true` 仍被解析以便 `codewhale doctor` 可以
报告过时的配置，但提供者客户端拒绝它而不是禁用 TLS
证书验证。

本地 HTTP 端点（如 Ollama、SGLang 和 vLLM）在使用
localhost 或回环地址时默认被允许。对于非本地 `http://`
网关，仅在受信任网络上使用 `DEEPSEEK_ALLOW_INSECURE_HTTP=1` 启动：

```bash
DEEPSEEK_ALLOW_INSECURE_HTTP=1 codewhale
```

需要额外请求头的第三方 OpenAI 兼容网关可以在顶级
或提供者表（如 `[providers.deepseek]`）下设置
`http_headers = { "X-Model-Provider-Id" = "your-model-provider" }`。配置后，
codewhale 在模型 API 请求上发送这些自定义头。等效的
环境变量覆盖是 `DEEPSEEK_HTTP_HEADERS`，使用逗号分隔的
`name=value` 对，如
`X-Model-Provider-Id=your-model-provider,X-Gateway-Route=dev`。`Authorization`
和 `Content-Type` 由客户端管理，不被此
设置覆盖。

### 视觉模型

CodeWhale 的聊天提供者和 `image_analyze` 工具是分开配置的。
主聊天路径保持选定的文本/工具提供者；当 `vision_model` 功能启用时，
图像分析通过 `[vision_model]` 运行。

小米当前的图像理解文档包括 `mimo-v2.5` 用于图像输入。
要使用 MiMo 进行 `image_analyze`，显式配置视觉模型：

```toml
[features]
vision_model = true

[vision_model]
model = "mimo-v2.5"
api_key = "YOUR_XIAOMI_KEY"
base_url = "https://api.xiaomimimo.com/v1"
```

上面的示例使用小米 MiMo 的按量付费 OpenAI 兼容端点。
MiMo Token Plan 用户应使用 Token Plan 基础 URL 并将模型设置为
计划特定的聊天模型。`image_analyze` 调用使用 `[vision_model]`
凭据/基础 URL，将图像作为 OpenAI 兼容的
`vision_url` 块发送，并要求视觉模型支持最大 5 MB 的 PNG/JPEG
图像。当功能启用且视觉模型配置时，`image_analyze` 和
`image_query` 工具在工具目录中可用。

## 环境变量参考

### 运行时 / TUI 配置

- `CODEWHALE_HOME` — CodeWhale 数据和配置目录（默认 `~/.codewhale`）。
  设置此项后，所有 CodeWhale 管理的数据都迁移到此目录。
- `DEEPSEEK_CONFIG_PATH` / `CODEWHALE_CONFIG_PATH`
- `DEEPSEEK_MCP_CONFIG`
- `DEEPSEEK_SKILLS_DIR`
- `DEEPSEEK_NOTES_PATH`
- `DEEPSEEK_MEMORY`（`1|on|true|yes|y|enabled`）
- `DEEPSEEK_MEMORY_PATH`
- `DEEPSEEK_LOG_LEVEL` 或 `RUST_LOG`（`info`/`debug`/`trace` 启用轻量级详细日志）
- `DEEPSEEK_MODEL`
- `DEEPSEEK_ALLOW_INSECURE_HTTP`（`1|on|true|yes|y`）
- `DEEPSEEK_HTTP_HEADERS`
- `DEEPSEEK_MODE`（`agent`/`ask`/`plan`/`yolo`）

### 提供者凭据和端点

- `DEEPSEEK_API_KEY`
- `DEEPSEEK_BASE_URL`
- `NVIDIA_API_KEY` 或 `NVIDIA_NIM_API_KEY`（当提供者为 `nvidia-nim` 时优先；回退到 `DEEPSEEK_API_KEY`）
- `NVIDIA_NIM_BASE_URL`、`NIM_BASE_URL` 或 `NVIDIA_BASE_URL`
- `NVIDIA_NIM_MODEL`
- `OPENAI_API_KEY`
- `OPENAI_BASE_URL`
- `OPENAI_MODEL`
- `ATLASCLOUD_API_KEY`
- `ATLASCLOUD_BASE_URL`
- `ATLASCLOUD_MODEL`
- `WANJIE_ARK_API_KEY`、`WANJIE_API_KEY` 或 `WANJIE_MAAS_API_KEY`
- `WANJIE_ARK_BASE_URL`、`WANJIE_BASE_URL` 或 `WANJIE_MAAS_BASE_URL`
- `WANJIE_ARK_MODEL`、`WANJIE_MODEL` 或 `WANJIE_MAAS_MODEL`
- `VOLCENGINE_API_KEY`、`VOLCENGINE_ARK_API_KEY` 或 `ARK_API_KEY`
- `VOLCENGINE_BASE_URL`、`VOLCENGINE_ARK_BASE_URL` 或 `ARK_BASE_URL`
- `VOLCENGINE_MODEL` 或 `VOLCENGINE_ARK_MODEL`
- `OPENROUTER_API_KEY`
- `OPENROUTER_BASE_URL`
- `OPENROUTER_MODEL`
- `XIAOMI_MIMO_TOKEN_PLAN_API_KEY`、`MIMO_TOKEN_PLAN_API_KEY`、`XIAOMI_MIMO_API_KEY`、`XIAOMI_API_KEY` 或 `MIMO_API_KEY`
- `XIAOMI_MIMO_BASE_URL` 或 `MIMO_BASE_URL`
- `XIAOMI_MIMO_MODEL` 或 `MIMO_MODEL`
- `XIAOMI_MIMO_MODE` 或 `MIMO_MODE`（`token-plan-sgp`、`token-plan-cn`、
  `token-plan-ams` 或 `pay-as-you-go`）
- `NOVITA_API_KEY`
- `NOVITA_BASE_URL`
- `NOVITA_MODEL`
- `FIREWORKS_API_KEY`
- `FIREWORKS_BASE_URL`
- `FIREWORKS_MODEL`
- `HUGGINGFACE_API_KEY` 或 `HF_TOKEN`（当提供者为 `huggingface` 时，`HF_TOKEN` 是接受的回退别名）
- `HUGGINGFACE_BASE_URL` 或 `HF_BASE_URL`
- `HUGGINGFACE_MODEL` 或 `HF_MODEL`
- `SILICONFLOW_API_KEY`
- `SILICONFLOW_BASE_URL`
- `SILICONFLOW_MODEL`
- `ARCEE_API_KEY`
- `ARCEE_BASE_URL`
- `ARCEE_MODEL`
- `TOGETHER_API_KEY`
- `TOGETHER_BASE_URL`
- `TOGETHER_MODEL`
- `QIANFAN_API_KEY` 或 `BAIDU_QIANFAN_API_KEY`
- `QIANFAN_BASE_URL` 或 `BAIDU_QIANFAN_BASE_URL`
- `QIANFAN_MODEL` 或 `BAIDU_QIANFAN_MODEL`
- `OPENAI_CODEX_ACCESS_TOKEN` 或 `CODEX_ACCESS_TOKEN`
- `OPENAI_CODEX_BASE_URL` 或 `CODEX_BASE_URL`
- `OPENAI_CODEX_MODEL` 或 `CODEX_MODEL`
- `OPENAI_CODEX_ACCOUNT_ID` 或 `CODEX_ACCOUNT_ID`
- `ANTHROPIC_API_KEY`
- `ANTHROPIC_BASE_URL`
- `ANTHROPIC_MODEL`
- `ZAI_API_KEY` 或 `Z_AI_API_KEY`
- `ZAI_BASE_URL` 或 `Z_AI_BASE_URL`
- `ZAI_MODEL` 或 `Z_AI_MODEL`
- `STEPFUN_API_KEY` 或 `STEP_API_KEY`
- `STEPFUN_BASE_URL` 或 `STEP_BASE_URL`
- `STEPFUN_MODEL` 或 `STEP_MODEL`
- `MINIMAX_API_KEY`
- `MINIMAX_BASE_URL`
- `MINIMAX_MODEL`
- `DEEPINFRA_API_KEY` 或 `DEEPINFRA_TOKEN`
- `DEEPINFRA_BASE_URL`
- `DEEPINFRA_MODEL`
- `MOONSHOT_API_KEY` 或 `KIMI_API_KEY`
- `MOONSHOT_BASE_URL` 或 `KIMI_BASE_URL`
- `MOONSHOT_MODEL`、`KIMI_MODEL_NAME` 或 `KIMI_MODEL`
- `SGLANG_BASE_URL`
- `SGLANG_MODEL`
- `SGLANG_API_KEY`（可选；许多本地主机 SGLang 服务器不需要认证）
- `VLLM_BASE_URL`
- `VLLM_MODEL`
- `VLLM_API_KEY`（可选；许多本地主机 vLLM 服务器不需要认证）
- `OLLAMA_BASE_URL`
- `OLLAMA_MODEL`
- `OLLAMA_API_KEY`（可选；许多本地主机 Ollama 服务器不需要认证）
- `DEEPSEEK_LOG_LEVEL` 或 `RUST_LOG`（`info`/`debug`/`trace` 启用轻量级详细日志）
- `DEEPSEEK_SKILLS_DIR`
- `DEEPSEEK_MCP_CONFIG`
- `DEEPSEEK_NOTES_PATH`
- `DEEPSEEK_MEMORY`（`1|on|true|yes
