# 自动 Workflow

对于普通的多 agent 工作，你**不需要**说"workflow"或编写 `.workflow.js` 文件。CodeWhale 会判断何时编排有帮助，告诉你形态，可能会问一个简短的设置问题，然后启动一个 Workflow。

相关文档：

- [Workflow 编写指南](WORKFLOW_AUTHORING.md) — 检入脚本和 IR
- [Fleet + Workflow 教程](FLEET_WORKFLOW_TUTORIAL.md) — 手动 Fleet 路径
- [配置](CONFIGURATION.md) — `[workflow]` 配置项
- [沙箱](SANDBOX.md) — Workflow VM 不能做什么

## 软自动（默认产品路径）

1. **你自然地提问** — "审计每个 crate 中的 unsafe，" "先探索再实现，" "并行比较这两个 provider。"
2. **CodeWhale 判断** — 广泛的、独立的或分阶段的工作触发 Workflow；单文件编辑、简单命令和纯问答不会触发。
3. **它先告诉你** — 例如"这看起来适合 Workflow — 三个 scout 然后一个 verifier。"
4. **可选设置** — 如果一两个事实会改变计划（只读 vs 写入、范围、子进程数量），它会打开 **`request_user_input`** 模态框（结构化的多选，而非冗长的自由文本面试）。
5. **启动** — 结构化的 `plan` JSON（目标 / 阶段 / 子进程）或简短的内联脚本。并行分支使用 `parallel()` 部分成功语义。

你仍然可以输入 `/workflow` 来强制"编排当前工作"。

## 只读自动启动 vs 写入批准

`[workflow]` 配置（参见 `config.example.toml`）：

| 配置项 | 默认值 | 含义 |
|------|---------|---------|
| `automatic` | `true` | 软自动编排已启用 |
| `auto_start_read_only` | `true` | 只读计划可以在没有写入批准卡片的情况下启动 |
| `require_approval_for_writes` | `true` | 写入 / 提权计划需要明确批准 |
| `auto_start_child_limit` | `8` | 自动子进程数量的软上限 |
| `max_children` / `max_depth` | `64` / `2` | 硬上限 |
| `default_token_budget` | `120000` | 共享预算提示 |
| `persist_completed_activity` | `true` | 保留已完成面板/历史活动 |

提权工作（写入、超出只读的 shell、网络、密钥、worktree、高预算）应在启动前显示批准卡片，包含目标、子进程摘要、能力标志和预算（#4126）。

## 运行期间你看到的内容

- **Workflow 面板** — 阶段、子进程、状态、预算
- **紧凑历史卡片** — 一个平静的行，可展开查看详情
- **每个委托单元一个产物** — 没有重复的"委托 + 工具卡片"
- **带类型的子进程身份** — 标签/角色；默认 UI 中没有"未知子进程"

取消会停止运行和子 agent。已完成的活动可以在会话期间持久化（配置后也可以在重启期间持久化）。

## 沙箱保证

Workflow JS VM **没有**文件系统、shell、网络、环境变量、导入、时钟或随机性。允许的主机调用：`task`、`parallel`、`pipeline`、`phase`、`log`、`budget`、`args`。实际工作在子 agent / Fleet 中进行，遵循正常的工具和批准策略。参见[沙箱](SANDBOX.md)。

## 综合与兼容性

- 对于必须返回结构化字段的子进程，优先使用 `responseSchema`。
- 失败的并行槽变为 `null`（部分成功）；在综合一个面向操作员的摘要之前过滤它们。
- 兼容性路径保留：`script`、`source_path`（检入的 `.workflow.js` / `.workflow.ts`）和结构化的 `plan`。

## 自动功能保持关闭的情况

以下情况会抑制自动 Workflow：

- 单文件编辑和微小的一步请求
- 简单命令 / 事实性问题
- 高度交互的设计对话
- 没有清晰分解的风险写入
- 估计子进程数量超过 `auto_start_child_limit`（先询问或缩减）

在这些情况下，CodeWhale 使用直接工具或单个 `agent` 代替。

## Dogfood 场景 (#4131)

自动 Workflow 的发布 lane dogfood 位于 [DOGFOOD_AUTOMATIC_WORKFLOWS.md](DOGFOOD_AUTOMATIC_WORKFLOWS.md)。它涵盖：

1. 只读仓库审计
2. 带 worktree implementer + verifier 的分阶段 bug 修复
3. 部分失败和综合
4. 运行中途取消

检入的 fixtures：[`docs/examples/dogfood-automatic/`](examples/dogfood-automatic/)。
面板回归测试在 `crates/tui/src/tui/widgets/workflow_panel.rs` 中使用 `dogfood_` 前缀。
