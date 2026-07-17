# Fleet + Workflow 教程

Fleet 和 Workflow 旨在协同工作，但它们解决问题的不同部分：

- **Fleet** 运行持久 worker，记录账本，保存日志和工件，并暴露状态/重启/停止控制。
- **Workflow** 描述编排：阶段、分支、归约器、循环，以及可以通过 Fleet/子代理运行时调度的代理叶节点。

**默认产品路径：** 使用自然语言请求。软自动 Workflow 会决定何时编排有帮助，**告诉你形状**，可能打开 `request_user_input` 进行 1–2 个设置选择，然后启动 — 你不需要为普通的多代理工作编写 workflow 文件。详情参见：
[自动 Workflow](AUTOMATIC_WORKFLOWS.md)。

本教程涵盖**手动** Fleet 任务规范 / 已检入 Workflow 路径，适用于需要持久主机 worker 和可审查规范的操作者。一句话的请求仍然不应在没有任何指示或审批的情况下静默生成 `tasks.json` 并启动 worker。

## 1. 准备工作区

从你希望 worker 检查或修改的工作区运行 Fleet：

```sh
codewhale fleet init
```

这将在 `.codewhale/fleet.jsonl` 创建工作区账本。Worker 日志和有界工件位于 `.codewhale/fleet/` 下；主机适配器日志位于 `.codewhale/fleet-host/` 下。

如果你想要命名的可复用 worker，打开 TUI 并运行：

```text
/fleet setup
```

选择一个角色，选择该配置文件是继承操作者路由还是指定特定的提供商/模型/思考等级，审查权限/工具/路由姿态，并批准渲染的 TOML。批准的配置文件保存在 `.codewhale/agents/<role>.toml` 下。Fleet 任务规范可以通过 `worker.agent_profile` 或更短的 `worker.profile` 别名引用这些配置文件。

## 2. 编写 Fleet 任务规范

`codewhale fleet run` 接受 JSON 或 TOML。已检入的 `docs/examples/fleet-dogfood.toml` 文件是现实的手动冒烟示例；下面的 JSON 展示了相同的编写形状，包含一个只读审查者和一个有界的文档笔记 worker。它保持禁用密钥，并将信任上限限制为 `sandbox`。

```json
{
  "name": "docs readiness check",
  "labels": {
    "kind": "tutorial"
  },
  "security_policy": {
    "default_trust_level": "sandbox",
    "max_trust_level": "sandbox",
    "allowed_secrets": [],
    "capability_grants": [],
    "require_identity_verification": true
  },
  "tasks": [
    {
      "id": "map-docs",
      "name": "Map current docs",
      "objective": "Find the docs that describe Fleet and Workflow.",
      "instructions": "Read docs/FLEET.md and docs/WORKFLOW_AUTHORING.md. Report the command surfaces, current limitations, and any confusing gaps.",
      "worker": {
        "role": "reviewer",
        "profile": "reviewer",
        "tools": ["rg", "sed", "git"],
        "model": "deepseek-v4-flash"
      },
      "workspace": {
        "required_files": ["docs/FLEET.md", "docs/WORKFLOW_AUTHORING.md"],
        "writable_paths": [],
        "environment": {
          "required": [],
          "allowlist": []
        }
      },
      "input_files": ["docs/FLEET.md", "docs/WORKFLOW_AUTHORING.md"],
      "expected_artifacts": ["log", "report"],
      "scorer": {
        "kind": "manual"
      },
      "retry_policy": {
        "max_attempts": 1
      }
    },
    {
      "id": "draft-gap-note",
      "name": "Draft gap note",
      "objective": "Draft a short local note for any missing tutorial steps.",
      "instructions": "Write a concise Markdown note with the missing Fleet + Workflow tutorial steps. Do not edit public docs unless explicitly asked.",
      "worker": {
        "role": "builder",
        "tools": ["rg", "sed"]
      },
      "workspace": {
        "required_files": ["docs/FLEET.md"],
        "writable_paths": [".codewhale/fleet"],
        "environment": {
          "allowlist": []
        }
      },
      "expected_artifacts": ["log", "report"],
      "scorer": {
        "kind": "manual"
      }
    }
  ]
}
```

将其保存为 `tasks.json`。

常用任务字段：

| 字段 | 用途 |
| --- | --- |
| `id`, `name` | 稳定的任务标识和显示名称。 |
| `objective`, `instructions` | Worker 目标和确切的操作指令。 |
| `worker.role` | 内置或自定义角色意图，例如 `reviewer`、`builder`、`read-only` 或 `smoke-runner`。 |
| `worker.profile` / `worker.agent_profile` | 来自 `.codewhale/agents/` 的已批准 Fleet roster 配置文件。 |
| `worker.tools` | 任务期望 worker 使用的工具名称。 |
| `worker.model` | 首选显式模型指定。路由解析仍然拥有提供商/模型验证。 |
| `worker.model_class`, `worker.loadout` | 兼容性路由提示，用于旧任务规范；新规范推荐使用 `worker.profile` 加已保存的配置文件路由指定。 |
| `workspace.required_files` | 任务开始前必须存在的文件。 |
| `workspace.writable_paths` | 在有效运行时姿态允许写入时，任务被允许写入的路径。 |
| `workspace.environment` | 必需或允许列表中的环境变量，仅限名称。 |
| `input_files`, `context` | 要注入到任务提示中的额外文件和字符串。 |
| `expected_artifacts` | 预期的工件类型：`log`、`report`、`patch`、`test_result`、`checkpoint` 或 `receipt`。 |
| `scorer` | 确定性或手动验证规则。 |
| `retry_policy`, `timeout_seconds`, `budget` | 重试和预算控制。 |

安全策略字段：

| 字段 | 用途 |
| --- | --- |
| `default_trust_level` | 默认 worker 信任等级。`sandbox` 是保守的默认值。 |
| `max_trust_level` | 运行中任何 worker 的上限。 |
| `allowed_secrets` | Worker 可以解析的密钥名称；永远不要在此处放置密钥值。 |
| `capability_grants` | 范围权限，例如 `network`、`git-push`、`provider-secrets`、`release` 或 `workspace-write`。 |
| `require_identity_verification` | 要求远程 worker 在提升信任前通过主机身份检查。 |
| `allow_parallel_reads` | 允许保守地批处理独立的只读操作。 |

## 3. 启动和监控 Fleet

启动运行：

```sh
codewhale fleet run tasks.json --max-workers 4
```

该命令会打印运行 ID 和 worker ID。在另一个终端中，监控账本状态：

```sh
codewhale fleet status
codewhale fleet inspect <worker-id>
codewhale fleet logs <worker-id>
codewhale fleet artifacts <worker-id>
```

当 worker 需要干预时，使用类型化控制：

```sh
codewhale fleet interrupt <worker-id>
codewhale fleet restart <worker-id>
codewhale fleet resume <run-id>
codewhale fleet stop --all
```

`resume` 用于在管理器退出、笔记本休眠或租约过期后的重启恢复。它重放账本并协调过时的工作，而不创建新的运行。

## 4. 编写 Workflow

Workflow 源代码是声明式的 JavaScript 或 TypeScript，会降级为类型化的 Rust `WorkflowSpec`。它不是通用的 JavaScript 运行时：`import`、进程访问、文件系统读/写、网络调用、`eval`、`async` 和 `await` 都会被拒绝。

创建一个已检入的文件，例如 `workflows/docs_readiness.workflow.js`。仓库中还包含 `workflows/issue_audit.workflow.js` 作为维护的示例。

```js
export default workflow({
  "id": "docs-readiness",
  "goal": "Inspect Fleet and Workflow docs, then synthesize a readiness note",
  "nodes": [
    {
      "branch": {
        "id": "parallel-docs-audit",
        "parallel": true,
        "children": [
          {
            "agent": {
              "id": "fleet-docs",
              "prompt": "Inspect docs/FLEET.md for command and task-spec coverage.",
              "agent_type": "review",
              "mode": "read_only",
              "profile": "reviewer",
              "file_scope": ["docs/FLEET.md"]
            }
          },
          {
            "agent": {
              "id": "workflow-docs",
              "prompt": "Inspect docs/WORKFLOW_AUTHORING.md for Workflow authoring coverage.",
              "agent_type": "review",
              "mode": "read_only",
              "profile": "reviewer",
              "file_scope": ["docs/WORKFLOW_AUTHORING.md"]
            }
          }
        ]
      }
    },
    {
      "reduce": {
        "id": "readiness-summary",
        "inputs": ["fleet-docs", "workflow-docs"],
        "prompt": "Summarize the exact docs gaps and the safest next edit."
      }
    }
  ]
});
```

当前的 Workflow 节点包装器有 `agent`、`branch`、`sequence`、`reduce`、`teacher_review`、`loop_until`、`cond` 和 `expand`。`agent.profile` 指定一个 Fleet roster 配置文件；显式的代理字段会覆盖配置文件的默认值。

面向模型的 `workflow` 工具可以从内联源代码或 `source_path` 启动、运行、检查或取消 workflow。当 CodeWhale 使用此路径时，如果 workflow 将启动多个 worker 或修改文件，请先要求它展示计划。

## 5. 自然语言输入

目前一个好的提示词是：

```text
Draft a Fleet task spec for this goal, but do not run it yet.
Show the proposed tasks, worker profiles, writable paths, expected artifacts,
scorers, and security policy. Keep secrets disabled unless I explicitly grant
them.
```

在审查生成的规范后，将其保存为 `tasks.json` 并运行上述 Fleet 命令。对于 workflow，让 CodeWhale 起草一个 `.workflow.js` 文件，展示计划，并仅在审批后使用 workflow 工具路径。

这个审查步骤是经过深思熟虑的。它使得提供商路由、DeepSeek 或其他模型支持、可写路径、网络访问和密钥使用在持久 worker 启动之前都保持显式。
