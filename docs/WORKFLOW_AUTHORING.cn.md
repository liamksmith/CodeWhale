# 工作流编写

> **普通的 multi-agent 工作不需要此文件。** 首选自然语言 + 软自动启动：CodeWhale 决定，指示计划，并可能通过 TUI 模态框询问设置问题。参见 [自动工作流](AUTOMATIC_WORKFLOWS.md)。

Workflow 有一个运行时边界：编写的源代码降低为类型化的 Rust `WorkflowSpec`，Rust 验证 IR，调度器/无头 worker 运行时执行叶子节点。编写语言不会获得隐藏权限来拥有文件、shell、网络、提供者、取消或 TUI 状态。

`workflow` 工具上的兼容启动路径：

| 输入 | 何时使用 |
|-------|-------------|
| `plan` | 结构化目标 / 阶段 / 子项（首选 agent 路径） |
| `script` | 模型拥有的简短内联 JS |
| `source_path` | 工作区中已检入的 `.workflow.js` / `.workflow.ts` |

有关从 Fleet 任务规范到 Workflow 编写和监控的引导式演练，请参见 [Fleet + Workflow 教程](FLEET_WORKFLOW_TUTORIAL.md)。


## 访问模型

Workflow 脚本是**仅协调器**。它自身没有文件系统或 shell。真正的工作发生在脚本启动的子 agent 中。

| 层 | 可以访问什么 |
|-------|--------------------|
| Workflow 脚本（JS VM） | 脚本变量、分支/循环、`task()` / `parallel()` / `pipeline()`、`phase` / `log`、`budget` / `args`。**无**直接 FS、shell、网络、环境变量、导入、时钟或随机性。 |
| Workflow 生成的子 agent | 正常工具表面（读取/搜索/编辑/写入、shell、web、MCP），受角色姿态、允许列表和父策略约束。具有写入功能的角色的文件编辑在 Workflow 下自动接受；shell / web / MCP 仍然需要父级自动批准或失败关闭。 |
| 父会话 | 工作目录、配置的工具/MCP、权限模式、沙箱/网络规则。 |

### 规模

- 一次运行中最多 **16 个并发**活跃 agent（额外的生成等待槽位）。
- 每次运行最多 **1,000 个 agent**（VM 生命周期生成上限）。
- 软自动启动仍然使用较低的软上限（`auto_start_child_limit`）。

有关失败关闭的主机表面清单，请参见 Workflow JS 沙箱测试。

## 语言选择

| 表面 | 优势 | 权衡 | v0.8.60 立场 |
|---|---|---|---|
| YAML / JSON IR | 简单、可审查、无运行时 | 对生成的工作流冗长 | 保留为交换/调试格式 |
| JavaScript | 熟悉的对象语法和轻松的 agent 生成 | 如果作为通用运行时执行则不安全 | 通过仅声明式编译子集成为一流的编写方式 |
| TypeScript | 面向工作流 SDK 的最佳编辑器/类型支持 | 如果支持完整 TS 则需要剥离/类型检查 | 目前相同的仅编译子集；以后提供更丰富的 SDK |

默认的高能力路径是 TypeScript/JavaScript 编写，但仅作为编译步骤。编译器接受来自 `.workflow.js` 或 `.workflow.ts` 的 `workflow({...})` 内的 JSON 兼容对象，将其降低为 `WorkflowSpec`，并运行 Rust 验证门禁。（Starlark 编写是引导参考并已被移除；Workflow 编写仅限 JS。）

## 契约

接受的源格式：

```js
export default workflow({
  "id": "issue-audit-js",
  "goal": "使用并行 agent 审计问题修复",
  "nodes": [
    {
      "branch": {
        "id": "parallel-audit",
        "children": [
          { "agent": { "id": "code-audit", "prompt": "审查代码", "agent_type": "review" } },
          { "agent": { "id": "test-audit", "prompt": "审查测试", "agent_type": "verifier" } }
        ]
      }
    },
    { "reduce": { "id": "summary", "inputs": ["code-audit", "test-audit"], "prompt": "总结" } }
  ]
});
```

支持的节点包装器：`agent`、`branch`、`sequence`、`reduce`、`teacher_review`、`loop_until`、`cond` 和 `expand`。带有 `kind` / `spec` 的原始 `WorkflowNode` JSON IR 也仍然有效。

`agent` 节点可以声明 `"profile": "reviewer"` 以作为命名 Fleet 名册配置文件运行。名称在编译时被修剪和小写，并且必须是单个令牌（无空格、引号或 `=`）；保存的名册在调度时解析，agent 上的显式字段覆盖配置文件默认值。

编译器拒绝有效构造，如 `import`、`require`、`fetch`、`process`、`Deno`、`Bun`、`child_process`、文件读取/写入、`eval`、`async` 和 `await`。这故意比 JavaScript 更严格：工作流源是熟悉的声明格式，而不是第二个执行运行时。

## 验证

- `cargo test -p codewhale-workflow --locked javascript`

当前示例：`workflows/issue_audit.workflow.js`。

## Agent 编写的 Fleet Workflow

主要产品流程不是"要求用户编写脚本"。主 agent 应该决定任务何时需要工作流编排，起草 Workflow 源，显示当前权限模式的计划，然后让运行时编译和监控它。

Workflow 拥有计划：阶段、分支、循环、归约器和中间结果。Fleet 拥有持久子 agent 配置：槽位、配置文件、模型、工具姿态、启动并发、租约、心跳、日志、收据和恢复/停止/重启控制。换句话说，工作流可以选择和监控 Fleet 槽位，但不能成为拥有自己的 shell 或文件系统权限的第二个执行器。

Fleet 启动验证在任何 Workflow IR 降低为 worker 之前应用保守的默认形状：

- 每次工作流运行最多 100 个总 worker agent；
- 最多 5 个递归 Fleet 环；
- 循环需要 `max_iterations`；
- 动态 `expand` 节点需要 `max_children` 和模板。

这些限制约束工作流总体规模，而不是瞬时启动并发。有效的 100-agent 工作流仍然可以通过较小的 Fleet worker 池排出。模型选择按槽位进行：DeepSeek 预设可以为编排器建议 `deepseek-v4-pro`，为附近的 worker 建议 `deepseek-v4-flash`，但用户和 agent 可以在任务需要时覆盖任何槽位。
