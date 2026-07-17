# Agent Fleet

Agent Fleet 是用于持久多 worker 运行的本地优先控制平面。它**不是**一个独立的执行引擎：fleet worker 是由 fleet 启动并持久跟踪的无头 `codewhale exec` 运行。关于子代理、`exec` 和 fleet 如何收敛于一个持久运行时的详细信息，请参见 [AGENT_RUNTIME.md](AGENT_RUNTIME.md)。在产品语言中，用户仍然可以"打开一个子代理"；在架构语言中，持久嵌套工作应该是一个带有角色的 fleet 支持的 worker。

当工作涉及重试、休眠/重启后存续、远程执行、收据或账本审计跟踪时，应使用 Fleet 而非短生命周期的 `agent` fanout。初始 CLI 界面如下：

如需一个结合 Fleet 任务规范和 Workflow 编写的指导性从入门到监控的教程，请参见 [Fleet + Workflow 教程](FLEET_WORKFLOW_TUTORIAL.md)。

```sh
codewhale fleet init
codewhale fleet run tasks.json --max-workers 4
codewhale fleet status
codewhale fleet inspect <worker-id>
codewhale fleet logs <worker-id>
codewhale fleet artifacts <worker-id>
codewhale fleet interrupt <worker-id>
codewhale fleet restart <worker-id>
codewhale fleet resume <run-id>
codewhale fleet stop --all
```

`codewhale fleet resume <run-id>` 是重启恢复动词：它重放账本，协调任何处于飞行中且其 worker 停止心跳的租约（在任务预算内重试，否则根据告警策略失败并上报），并打印恢复后的状态。它不会启动新工作，并且是幂等的，因此在管理器退出、笔记本休眠或运行时重启后可以安全运行。

Fleet 状态存储在工作区的 `.codewhale/fleet.jsonl` 下。Worker 日志和适配器日志存储在 `.codewhale/fleet/` 和 `.codewhale/fleet-host/` 下。

## 编写代理配置文件 (`/fleet setup`)

`/fleet setup`（以及 `/fleet setup edit` / `new`）打开一个 TUI 内向导，用于编写可复用的代理团队配置文件。裸 `/fleet` 以及 `roster`/`roles`/`profiles`/`party` 别名会打开 roster（已保存的配置文件）。`/fleet status` 打开 worker 状态视图；`/subagents` 是该状态视图的兼容性快捷方式。

该向导是渐进式的：你每次只做一项专注的选择 — 一个**角色**，然后一个**模型**（`inherit`，或来自*任何已配置提供商*的具体模型，而不仅仅是父会话当前使用的那个），然后一个**思考等级**（`inherit`、`off`、`low`、`medium`、`high`、`max` 或 `auto`）— 然后在执行任何操作之前审查完整姿态（路由、思考、权限、工具、工作区/组织范围和审查策略）。选择一个具体模型会显式固定其提供商：已保存的配置文件同时记录 `model` 和 `provider` 字段，因此它命名的路由不依赖于配置文件稍后加载时碰巧激活的提供商。在审查步骤按 **Enter**（"开始"）会在同一屏幕上内联预览确切的起始配置文件 TOML；在你批准之前不会写入任何内容。`provider` 字段可以是一个内置提供商 ID，如 `openrouter`，也可以是一个用户命名的 OpenAI 兼容提供商，配置在 `[providers.<name>]` 下，如 `lm-studio`；启动路径会保留该 ID，并在提供商未配置时安全失败关闭。

当提供商已配置时，审查步骤还会在批准门控后提供模型辅助起草：

- 按 **`m`** 让你第一个配置的模型起草配置文件。草案会经过清理和限制 — 权限保持在 **fleet 下限**（无 shell、无 trust、需要审批），无论模型提出什么。
- **起草不等于批准。** 确切的渲染 TOML 预览内联显示在审查步骤上（不是在单独的可滚动查看器中），因此在你按 **`g`** 或 **Enter** 批准之前（或再次按 `m` 重新起草）不会保存任何内容。批准会将配置文件写入 `.codewhale/agents/<role>`。

## 命名：模式、Workflow 和 Fleet

这些名称描述的是不同的层次，而不是竞争的系统。Agent、Plan 和 YOLO 仍然是权限/工作模式。Workflow 是一个编排覆盖层，当任务需要持续的工作流时，可以在这些模式之上运行。

- **Workflow** 是可重复的计划和面向用户的编排覆盖层：一个脚本/IR，决定接下来运行哪些阶段和代理，将中间结果排除在主对话之外，并且可以被检查或重新运行。Workflow 运行应该有一个可见的进度视图和一个清晰的活动标题状态，而不是感觉像一个隐藏的后台任务。
- **Fleet** 是持久子代理配置和执行底层：槽位、配置文件、按槽模型、工具姿态、本地/SSH 主机、信任策略、租约、心跳、日志、收据和状态 API。
- **高 fan-out** 是 Workflow 运行的一种行为，而不是一个独立的系统：当一个阶段需要同时使用许多 worker 时，Workflow 将它们作为 Fleet 支持的运行来调度（持久 worker、收据、目标重新调度），而不是恢复仅提示词的子代理 fanout。
- **Fan-in 是强制性的：** 没有拥有者的 fan-out 是不允许的，该拥有者负责等待、聚合、验证和综合一个结果。操作者应该依赖一个管理器或 workflow 收据，而不是散布在转录中的 N 个松散的 `agent` 子进程。

UI 指导：保持主转录区简洁。Workflow 运行应显示为一个紧凑的进度卡片，加上工作/代理侧边栏行，显示阶段名称、worker 计数、收据以及子 worker 的嵌套缩进。谨慎使用鲸鱼标记作为活动标题/状态信号；避免为每个 worker 重复使用充满 emoji 的行。

## 管理器拥有的操作

当并行工作必须返回一个合并的答案时，使用管理器拥有的操作而不是扁平的 `agent` fan-out：

1. **指定一个管理器**（操作者或 workflow 编排器）。
2. **Fan out** 子任务，通过 `workflow`（`task()`、`parallel()`、`pipeline()`、`phase()`）或一个拥有子进程的单一管理器会话。
3. **等待**子收据或完成事件。
4. **聚合和验证**关键声明，然后将其视为事实。
5. **综合**一个操作者可以依赖的结果。

原始的 `agent` fan-out 仅适用于独立的、发射后不管的工作，其中不需要单一的 fan-in 结果。如果结果必须被合并、比较或验证，请通过 `workflow` 路由，以便管理器拥有 fan-in。

## Workflow on Fleet

预期的高能力路径是代理编写的。当主代理决定某个任务需要的协调比逐轮子代理调用更持久时，它会起草一个 Workflow 脚本/IR，根据活动权限模式呈现运行计划，然后运行时将其编译为类型化的 Fleet 工作。

Fleet 仍然是子代理配置界面。它拥有槽位数量、角色配置文件、已保存的路由指定或继承、工具姿态、启动并发和账本。Workflow 仅拥有编排计划：分支、序列、循环、展开、审查和归约决策。workflow 脚本不得获得直接的 shell、文件系统、网络、提供商密钥、取消或 TUI 权限；worker 作为 `codewhale exec` 进程执行实际工作。

默认的 Workflow-to-Fleet 验证是有意限制的：

- 每个 workflow 运行最多 100 个 worker 代理；
- 最多 5 层递归 Fleet 环；
- 仅限有界循环（需要 `max_iterations`）；
- 仅限有界动态展开（需要 `max_children` 加模板）。

这些是人口限制，而不是要求一次启动所有内容。一个 100 代理的 workflow 仍应通过配置的 Fleet worker 池进行排泄。推荐的模型布局，例如 DeepSeek Pro 编排器搭配 Flash worker 在第一环，更便宜的 worker 在更外层，仅是预设。每个槽位可以继承活动模型或携带显式模型覆盖。继承是字面意义的：你在 `/model`中选择的模型是**操作者**（`/fleet roster` 中的固定第一行），任何任务规范和 roster 配置文件未指定模型的 worker 会在该会话模型上运行。任务级别的 `model` 和配置文件 `model` 覆盖仍然优先；路由收据记录应用了哪个来源（`task.model`、`agent_profile.model` 或 `run.model`）。

设置 UI 应将其渲染为可展开的网格：一个编排器加上少量可见的子代理槽位，通过 Right/Enter 深入槽位的下一递归环，而不是试图一次显示整个树。

## 任务规范

`codewhale fleet run` 接受 JSON 或 TOML。一个最小的 JSON 规范：

```json
{
  "name": "local smoke",
  "tasks": [
    {
      "id": "lint",
      "name": "Lint",
      "instructions": "Run the lint check and report failures.",
      "expected_artifacts": ["log"]
    }
  ]
}
```

Worker 是可选的。如果省略，CodeWhale 会根据 `--max-workers` 创建本地 worker 槽位。

任务规范在 Rust 中是类型化的，并将验证数据与 worker 转录分开。一个任务可以声明：

- `id`、`name`、`description`、`objective` 和 `instructions`
- `worker` 角色、工具配置文件、工具和所需能力
- `workspace` 根目录、必需文件、可写路径和环境允许列表
- `input_files`、额外的 `context`、`budget`、`timeout_seconds` 和 `retry_policy`
- `expected_artifacts`、`scorer`、`tags` 和自由形式的 `metadata`

Worker 将有界工件文件写入 `.codewhale/fleet/` 下，账本仅记录工件引用：类型、路径、校验和、MIME 类型和大小。收据记录 `pass`、`fail`、`partial`、`skip` 或 `timeout`；失败的收据还可能将来源标记为 `transport`、`task` 或 `verifier`。`codewhale fleet status` 分别显示这些失败来源的计数。

内置的确定性评分器包括 `exit_code`、`file_exists`、`regex_match` 和 `json_path`。规范还可以声明 `command`、`code_whale_verifier_prompt` 或 `manual`；这些会记录部分收据，直到显式的验证者通过完成。

### 使用角色预设

任务可以引用角色名称，fleet 管理器会从角色注册表中填充默认值。内置角色（`smoke-runner`、`reviewer`、`builder`、`read-only`）始终可用；在 `[fleet.roles]` 中定义你自己的角色。

```json
{
  "name": "smoke check",
  "tasks": [
    {
      "id": "lint",
      "name": "Lint check",
      "instructions": "Run lint and report failures.",
      "worker": { "role": "smoke-runner" },
      "expected_artifacts": ["log"]
    }
  ]
}
```

任务会继承角色的工具配置文件、预算和超时。你可以在任务规范中覆盖任何字段：

```json
{
  "id": "deep-review",
  "name": "Deep review",
  "instructions": "Review the entire crate for soundness issues.",
  "worker": {
    "role": "reviewer",
    "tools": ["cargo", "rg", "git"],
    "capabilities": ["rust"]
  },
  "input_files": ["crates/**/*.rs"],
  "budget": { "max_tokens": 32000 },
  "expected_artifacts": ["log", "report"],
  "scorer": { "kind": "regex_match", "path": ".codewhale/fleet/report.md", "pattern": "finding|all clear" }
}
```

### 多任务运行示例

单次 fleet 运行可以并行调度多个独立任务：

```json
{
  "name": "CI gate",
  "tasks": [
    {
      "id": "check",
      "name": "Compile check",
      "instructions": "Run cargo check --workspace and report errors.",
      "worker": { "role": "builder" },
      "expected_artifacts": ["log"],
      "scorer": { "kind": "exit_code" }
    },
    {
      "id": "clippy",
      "name": "Clippy lint",
      "instructions": "Run cargo clippy --workspace and report warnings.",
      "worker": { "role": "reviewer", "tools": ["cargo", "cargo-clippy"] },
      "expected_artifacts": ["log"],
      "scorer": { "kind": "exit_code" }
    },
    {
      "id": "security",
      "name": "Secret audit",
      "instructions": "Search for plaintext secrets and report any matches.",
      "worker": { "role": "read-only", "tools": ["rg"] },
      "input_files": ["crates/**/*.rs"],
      "expected_artifacts": ["log", "report"],
      "retry_policy": { "max_attempts": 1 }
    }
  ]
}
```

## 告警

Fleet 告警默认是禁用的。调用者必须提供启用的告警配置，才会发送任何内容。路由匹配类型化的 fleet 事件类，而不是日志字符串：

- `stale`
- `restart_exhausted`
- `needs_human`
- `budget_exceeded`
- `verifier_failed`
- `run_completed`

适配器配置存储环境变量名称，而不是密钥值。发送时代码从环境或未来的密钥提供者解析这些名称。账本记录仅存储审计标签，如 `slack`、`webhook` 或 `pagerduty`；持久化在账本中的任务规范会脱敏 webhook URL 和路由密钥。

告警配置形状示例：

```json
{
  "enabled": true,
  "dry_run": true,
  "routes": [
    {
      "events": ["stale", "restart_exhausted", "verifier_failed"],
      "adapter": "ops-slack"
    },
    {
      "events": ["restart_exhausted"],
      "adapter": "pager"
    }
  ],
  "adapters": {
    "ops-slack": {
      "kind": "slack",
      "webhook_env": "CODEWHALE_FLEET_SLACK_WEBHOOK",
      "channel": "#codewhale-fleet"
    },
    "pager": {
      "kind": "pager_duty",
      "routing_key_env": "CODEWHALE_FLEET_PAGERDUTY_ROUTING_KEY",
      "severity": "critical"
    }
  }
}
```

使用试运行在不发送的情况下检查脱敏的适配器负载：

```sh
codewhale fleet alert-dry-run \
  --event stale \
  --run-id fleet-demo \
  --worker-id fleet-demo-local-1 \
  --task-id release-triage \
  --reason "worker heartbeat stale since 2026-06-13T02:00:00Z" \
  --adapter slack
```

负载包括运行 ID、worker ID、任务 ID、状态、简短原因以及安全检测命令，如 `codewhale fleet status` 和 `codewhale fleet inspect <worker-id>`。端点、webhook 密钥和 PagerDuty 路由密钥显示为 `<redacted:env:...>`。

## 状态界面

`codewhale fleet status` 显示排队、运行中、完成、部分、失败、重启、上报、取消、过时以及验证者/传输失败来源的紧凑计数。`inspect` 显示 worker 状态加上当前任务目标、角色、主机、心跳、最新事件、工件引用、最新错误和告警状态。`logs` 打印有界日志工件内容，`artifacts` 列出工件引用而不嵌入大负载。

Runtime API 在现有运行时认证中间件后面暴露相同的账本支持投影：

```text
GET  /v1/fleet/runs
GET  /v1/fleet/runs/{run_id}
GET  /v1/fleet/runs/{run_id}/workers
GET  /v1/fleet/workers/{worker_id}
POST /v1/fleet/workers/{worker_id}/interrupt
POST /v1/fleet/workers/{worker_id}/restart
POST /v1/fleet/runs/{run_id}/stop
```

操作端点调用与 CLI 相同的管理器控制，并在 fleet 账本中记录其决策。

## 管理器代理操作手册

管理器代理应将 Fleet 操作视为类型化的、账本记录的控制平面工作。从 `codewhale fleet status` 开始，然后使用 `codewhale fleet inspect <worker-id>`、`logs` 和 `artifacts` 检查一个运行或 worker。仅在类型化的 CLI/API 界面无法提供所需证据时，才使用 `.codewhale/fleet.jsonl`、主机日志或远程文件的直接读取。

在采取行动之前对 worker 进行分类：

- `transient failure`：过时心跳、主机超时、中断的传输、可重试的提供商/网络错误，或可以合理恢复而不改变任务的适配器状态。
- `task failure`：worker 完成但产生了不正确的结果、领域失败、缺少必需的工件或显式的任务级别错误。
- `verifier failure`：worker 结果存在，但评分器/验证者失败、超时或与收据不一致。
- `needs-human`：缺少权限、密钥请求、破坏性操作、重复重启耗尽、模糊的产品决策，或管理器无法从类型化工件中解决的冲突证据。

选择一个类型化操作：

- 仅在失败是瞬态的、重试预算仍然存在、任务是幂等或重试安全的，且不涉及权限或密钥边界时，重新启动 worker：`codewhale fleet restart <worker-id>`。
- 仅当当前任务继续运行不安全或操作者明确要求取消时，中断或停止：`codewhale fleet interrupt <worker-id>` 或 `codewhale fleet stop --all`。
- 默认情况下不要重启纯任务失败；保留工件并将收据交给任务拥有者，除非任务规范说明重试可以产生新的证据。
- 对于验证者失败，首先检查评分器输入和工件引用。如果验证者无法通过类型化 fleet 操作纠正，上报人工审查。
- 对于 `needs-human`，起草上报而不是发送，除非告警配置明确授权发送。

安全的 Slack 或 PagerDuty 草稿：

```text
CodeWhale fleet needs attention
Run: <run-id>
Worker: <worker-id>
Task: <task-id or unknown>
Classification: <transient failure | task failure | verifier failure | needs-human>
Reason: <one sentence, no secrets>
Latest typed evidence: codewhale fleet inspect <worker-id>; codewhale fleet artifacts <worker-id>
Safe log excerpt: <3 lines max or "see artifact <ref>">
Requested decision: <restart approval | verifier review | task owner review | permission decision>
```

运行后摘要应包括运行 ID、已检查的 worker、分类、采取或起草的类型化操作、预期的账本效果、审查的工件引用以及下一个拥有者。保持摘要有界；链接工件引用而不是复制完整日志或转录。

捆绑的 `fleet-manager` 技能为管理器代理反映了此操作手册。它是一个第一方系统技能，在系统技能安装或刷新后应通过正常的技能注册表被发现。

## 主机适配器

主机适配器边界支持本地子进程和显式 SSH worker。适配器暴露相同的操作：启动、读取状态、读取有界日志、中断、重启、停止和清理。

本地 worker 作为子进程运行，stdin 关闭，stdout/stderr 写入有界 fleet 主机日志。它们仅继承一个小的安全基础环境，如 `PATH` 和显式允许列表中的变量。

SSH worker 通过系统 `ssh` 客户端运行，使用 `BatchMode=yes` 和有界连接超时。远程环境变量通过 OpenSSH `SendEnv` 发送；值不会嵌入本地 ssh argv 或 fleet 日志中。

SSH worker 规范示例：

```json
{
  "id": "builder-1",
  "name": "Builder 1",
  "host": {
    "kind": "ssh",
    "host": "builder.example.com",
    "user": "codewhale",
    "port": 22,
    "identity": "~/.ssh/codewhale_fleet",
    "working_directory": "/srv/codewhale/work",
    "env_allowlist": ["CODEWHALE_PROFILE"],
    "codewhale_binary": "/usr/local/bin/codewhale"
  },
  "capabilities": ["local", "linux", "tests"],
  "max_concurrent_tasks": 1
}
```

默认值是有意保守的：

- 不启用托管控制平面或云配置；
- SSH 需要显式的主机、工作目录和 CodeWhale 二进制路径；
- 类似密钥的环境名称，如 `TOKEN`、`SECRET`、`PASSWORD`、`API_KEY` 和 `PRIVATE_KEY` 会被适配器允许列表拒绝；
- 密钥应保留在 CodeWhale 配置提供者或远程主机配置中，而不是任务指令、argv 或 fleet 日志中。

## 安全和信任边界

Agent Fleet 强制执行一个信任等级模型，将 worker 分为四个等级。信任等级决定了 worker 可以访问什么（密钥、网络、工作区写入）以及在被授予这些权限之前必须如何证明其身份。

### 信任等级

| 等级 | 访问权限 | 需要 |
|-------|--------|----------|
| `sandbox` | 无网络、无密钥、仅写入 `.codewhale/fleet/` | 无 — 新 worker 的默认值 |
| `local` | 工作区读取、有门控的写入、已配置的密钥 | 本地进程（相同 uid） |
| `remote-verified` | 网络访问、有界能力授予、已配置的密钥 | SSH 主机密钥验证或等效认证 |
| `operator` | 对所有人密钥的完全访问、无限制写入、任何操作 | 操作者拥有的机器 |

默认信任等级是 `sandbox`。操作者必须通过安全策略显式提升 SSH 或容器 worker 的信任。

### 安全策略

fleet 运行可以携带一个可选的 `security_policy` 块，定义默认信任等级、worker 可以解析哪些密钥、授予哪些能力以及最大信任等级的上限：

```json
{
  "security_policy": {
    "default_trust_level": "sandbox",
    "allowed_secrets": [
      {"key": "GH_TOKEN", "source": "env"},
      {"key": "CODEWHALE_API_KEY", "source": "keyring"}
    ],
    "capability_grants": [
      {
        "capability": "network",
        "scope": "github.com",
        "reason": "PR review needs GitHub API access"
      }
    ],
    "max_trust_level": "remote_verified",
    "require_identity_verification": true
  }
}
```

当运行没有显式的 `security_policy` 时，worker 继承保守的默认值：`sandbox` 信任、无密钥、无能力授予、无身份验证要求。

### 密钥引用

密钥从不以明文形式存储在任务规范、告警配置或 worker 定义中。相反，每个密钥都是一个 `FleetSecretRef` — 一个密钥名称加上一个可选的来源提示，告诉 fleet 管理器在哪里解析值：

```json
{"key": "GH_TOKEN", "source": "env"}
```

支持的来源：
- `"env"` — 从进程环境变量解析
- `"keyring"` — 从操作系统密钥环解析（macOS Keychain、Windows Credential Manager、Linux Secret Service）
- `"file"` — 从 `~/.codewhale/secrets/` 解析
- 缺失 — 按默认顺序尝试所有来源（先存储，然后环境变量）

密钥引用在日志和账本条文中被脱敏：`<secret:env.GH_TOKEN>`。

### Worker 认证

Worker 通过以下三种方式之一向 fleet 管理器认证：

- **无** — 共享相同 uid 的本地 worker（默认）
- **SSH 密钥** — 可选的主机密钥指纹固定和 known-hosts 验证。`host_key_fingerprint` 字段（SHA256:...）固定预期的服务器密钥，防止首次连接时的 MITM 攻击。
- **Token** — 从 `FleetSecretRef` 解析的 Bearer token，适用于 fleet 代理后的远程 worker。
- **mTLS** — 相互 TLS，使用客户端证书和密钥支持的私钥。

SSH worker 在生产环境中应始终设置 `host_key_fingerprint`：

```json
{
  "id": "builder-1",
  "name": "Builder 1",
  "trust_level": "remote_verified",
  "host": {
    "kind": "ssh",
    "host": "builder.example.com",
    "user": "codewhale",
    "port": 22,
    "identity": "~/.ssh/codewhale_fleet",
    "host_key_fingerprint": "SHA256:aLGqZo1M6c...",
    "known_hosts": "~/.ssh/known_hosts",
    "working_directory": "/srv/codewhale/work",
    "env_allowlist": ["CODEWHALE_PROFILE"],
    "codewhale_binary": "/usr/local/bin/codewhale"
  },
  "capabilities": ["local", "linux", "tests"],
  "max_concurrent_tasks": 1
}
```

### 告警通道密钥

告警通道（Slack、通用 webhook、PagerDuty）使用 `FleetAlertEndpoint` 而不是原始 URL。webhook URL 可以为非敏感端点内联提供，或作为密钥引用：

```json
{
  "kind": "slack",
  "webhook": {
    "url_ref": {"key": "CODEWHALE_FLEET_SLACK_WEBHOOK", "source": "env"},
    "secret_ref": {"key": "CODEWHALE_FLEET_SLACK_SIGNING_SECRET", "source": "keyring"}
  }
}
```

`secret_ref` 字段为 webhook 负载签名提供可选的 HMAC 密钥，从不以明文存储。

### 配置文件

`config.toml` 中的 `[fleet]` 表设置全局信任策略默认值：

```toml
[fleet]
default_trust_level = "sandbox"
require_identity_verification = true
max_trust_level = "operator"

[fleet.exec]
# 递归深度与独立子代理共享一个轴 — fleet worker 就是一个无头子代理。0 阻止子代理（根 worker 仍然运行）；
# 3 是默认值；显式配置会被钳制到共享的安全上限。
max_spawn_depth = 3
```

这些默认值适用于不携带自己的 `security_policy` 的 fleet 运行。每次运行的策略始终覆盖配置默认值。

### 能力授予

能力授予是附加的、有范围的权限，用于授权特定操作。默认情况下，worker 不获得任何授予（最小权限）。常见授予：

- `"network"`，范围 `"github.com"` — 允许到 GitHub 的出站 HTTP
- `"git-push"` — 允许 `git push` 到远程
- `"provider-secrets"` — 允许访问提供商 API 密钥
- `"release"` — 允许发布相关操作（打标签、发布）
- `"workspace-write"`，范围 `"crates/tui/**"` — 允许在路径内写入

### 环境清理

主机适配器层在 worker 启动时强制执行环境清理：

- 默认情况下，仅 `HOME`、`PATH` 和平台特定变量（`SYSTEMROOT`、`COMSPEC`）被注入到 worker 进程中
- 环境允许列表拒绝任何包含 `SECRET`、`TOKEN`、`PASSWORD`、`PASSWD`、`API_KEY`、`CREDENTIAL` 或 `PRIVATE_KEY` 的键
- SSH worker 仅通过 OpenSSH `SendEnv` 发送显式允许列表中的变量
- 密钥值从不嵌入到 worker argv、任务指令或 fleet 日志中 — 仅出现密钥引用，并且它们始终被脱敏
