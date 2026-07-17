# 模式与审批

codewhale 有两个相关概念：

- **TUI 模式**：你所处的可见交互类型（Plan/Act/Operate）。
- **审批姿态**：UI 在执行工具之前的询问激进程度。
- **Workflow 覆盖层**：可选的长时间运行编排，当任务需要多个协调的工作者时，可在任何 TUI 模式之上运行。

模型选择是独立的。`--model auto` 和 `/model auto` 在每个回合中将路由到具体的模型和思考级别；它们不是 TUI 模式，也不属于 `Tab` 循环。

Workflow 也与 `Tab` 模式循环分离。它是用于可重复工作流和 Fleet 工作者的可见持续工作层。高扇出通过持久的 Fleet 支持的工作者路由，而不是仅提示的子代理扇出。活动模式仍然控制权限；Workflow 控制是否将大型任务规划为具有自己进度视图的可恢复工作流。

## TUI 模式

按 `Tab` 完成作曲器菜单、在回合运行时将草稿排队作为下一回合的后续操作，或在作曲器空闲时循环切换可见模式：**Plan → Act → Operate → Plan**。
按 `Shift+Tab` 循环切换权限姿态（Ask → Auto-Review → Full Access）。
按 `Ctrl+T` 循环切换推理努力程度。
运行 `/mode` 打开模式选择器，或使用 `/mode act`、
`/mode plan`、`/mode operate` 或 `/mode yolo`（已弃用的兼容性别名）直接切换。

- **Plan**：设计优先的提示。只读调查工具保持可用；shell 和补丁执行保持关闭。当你想大声思考并产生一个计划交给人类（稍后的你自己或审查者）时使用此模式。
- **Act**（Agent）：多步骤工具使用。在交互式 TUI 会话中，shell 工具（`exec_shell`、`task_shell_start`、`task_shell_wait`）默认可用，审批提示门控每次调用。设置顶层 `allow_shell = false` 以在某个工作区/配置文件下隐藏 shell 工具。文件写入无需提示即可允许。
- **Operate**：指挥者姿态——优先使用 Fleet 成员 + `/workflow` 编排，而非单打独斗的内联工具链；默认委托。
- **YOLO**（已弃用）：映射到 Act + 完全访问权限（`Shift+Tab` 到 Bypass）。仅在受信任的仓库中使用。

**Act** 被接受为 Agent 模式的别名。保存的设置仍规范化为 `agent` 以保持向后兼容。

### 各模式下的工具可用性

| 工具系列 | Plan | Act | Operate |
|:---|:---:|:---:|:---:|
| 只读文件、搜索和诊断工具 | 是 | 是 | 是 |
| 文件写入和补丁工具 | 否 | 是 | 是 |
| Shell 工具（`exec_shell`、`task_shell_start`、等待、交互、取消） | 否 | 默认审批门控，`allow_shell = false` 时隐藏 | 是 |
| 付费或外部服务工具 | 审批门控 | 审批门控 | 自动批准 |
| 访问工作区根目录之外 | 否 | 仅信任模式 | 是 |

如果 Agent 模式下模型可见目录中缺少 shell 工具，请检查活动配置/配置文件或运行时会话中是否有显式的 `allow_shell = false`。持久化任务和自动化保持保守的省略字段默认值；它们仅在任务设置显式授予时才获得 shell 访问权限。
`allow_shell = true` 仅控制 shell 可用性；直接的多行 `exec_shell` 命令仍会被 shell 安全验证阻止。对于 heredoc、嵌入式脚本或长手动流程，请使用单行命令、先编写脚本/文件，或通过 `task_shell_start`/后台 shell 运行。
YOLO 同时开启 shell 访问、信任模式和自动批准。

所有具有操作能力的模式都可以通过 `rlm_open`、`rlm_eval`、`rlm_configure` 和 `rlm_close` 访问持久化的 RLM 会话。在 RLM Python REPL 内部，`sub_query_batch` 可扇出 1-16 个廉价的并行子调用，固定使用 `deepseek-v4-flash`。当工作对父级转录来说太大或太重复时，模型会使用它。

快速的 `deepseek-v4-flash` / 关闭思考模式路径在产品语言中称为 Fin。Fin 是路由、摘要、廉价子调用和协调工作的接缝；它不改变审批行为。

`/goal` 设置带有可选令牌预算的会话目标，并将活动目标作为 Work 上下文可见。`/goal pause` 暂停目标继续而不更改目标，`/goal resume` 恢复并将目标重新发送到回合中，`/goal complete` 标记为完成，`/goal blocked` 标记为阻塞，`/goal clear` 清除它。目标状态不改变活动的 TUI 模式、审批模式或模型路由。这与 `--model auto` 保持区别，后者仅控制模型和思考选择。

Workflow 建立在相同的分离之上：目标可以要求代理继续工作，而 Workflow 为大型扇出提供可重复的工作流/进度界面。在 UI 中，Workflow 运行应作为主屏幕上的覆盖层显示，而不是作为 Agent、Plan 和 YOLO 旁边的第四种模式。

应用服务器客户端可以使用 `thread/goal/set` 持久化线程范围的目标，使用 `thread/goal/get` 读取，使用 `thread/goal/clear` 清除。该持久化记录携带 `active`、`paused`、`blocked`、`usage_limited`、`budget_limited` 或 `complete` 状态以及令牌/时间计数字段，适用于需要线程恢复语义的客户端。

## 兼容性说明

- 带有 `default_mode = "normal"` 的旧设置文件仍加载为 `agent`；保存时会重写为规范化的值。

## Escape 键行为

`Esc` 是取消栈，不是模式切换。

- 首先关闭斜杠菜单或瞬态 UI。
- 如果回合正在运行，取消活动请求。
- 如果作曲器为空，丢弃排队的草稿。
- 如果有文本存在，清除当前输入。
- 否则为无操作。

## 审批模式

你可以在运行时覆盖审批行为：

```text
/config
# 编辑 approval_mode 行：suggest | auto | never
```

遗留说明：`/set approval_mode ...` 已被 `/config` 取代。

- `suggest`（默认）：使用上述各模式规则。
- `auto`：自动批准所有工具（类似于 YOLO 审批行为，但不强制 YOLO 模式）。
- `never`：阻止任何不被视为安全/只读的工具。

## 小屏幕状态行为

当终端高度受限时，状态区域首先压缩，以便页眉/聊天/作曲器/页脚保持可见：

- 加载和排队状态行按可用高度预算。
- 当完整预览无法容纳时，排队预览折叠为紧凑摘要。
- `/queue` 工作流仍然可用；紧凑状态仅影响渲染密度。

## 工作区边界和信任模式

默认情况下，文件工具限制在 `--workspace` 目录内。启用信任模式以允许文件访问工作区之外：

```text
/trust
```

YOLO 模式自动启用信任模式。

## MCP 行为

MCP 工具暴露为 `mcp_<server>_<tool>`，并使用与内置工具相同的审批流程。只读 MCP 助手在建议审批模式下可能自动运行；可能有副作用的 MCP 工具需要审批。

参见 `MCP.md`。

## 相关 CLI 标志

运行 `codewhale --help` 获取规范列表。常用标志：

- `-p, --prompt <TEXT>`：一次性提示模式（打印并退出）
- `codewhale exec --auto --output-format stream-json <PROMPT>`：运行支持工具的非交互式代理，每行发出一个 JSON 对象，用于测试框架和后端包装器
- `codewhale exec --resume <ID|PREFIX> <PROMPT>` / `--session-id <ID|PREFIX>`：以非交互方式继续已保存的会话
- `codewhale exec --continue <PROMPT>`：以非交互方式继续此工作区最近保存的会话
- `codewhale fork <ID|PREFIX>` / `codewhale fork --last`：将已保存的会话复制到新的同级会话中；分叉的会话保留附加的父会话元数据，并在会话列表中显示该谱系
- `--model <MODEL>`：使用 `codewhale` 外观时，将 DeepSeek 模型覆盖转发到 TUI
- `--workspace <DIR>`：文件工具的工作区根目录
- `--yolo`：以 YOLO 模式启动
- `-r, --resume <ID|PREFIX|latest>`：恢复已保存的会话
- `-c, --continue`：恢复此工作区中最近的会话
- `--max-subagents <N>`：限制为 `1..=128`
- `--mouse-capture` / `--no-mouse-capture`：选择加入或退出内部鼠标滚动、转录选择、右键上下文操作和转录滚动条拖动。鼠标捕获在非 Windows 终端和 Windows Terminal/ConEmu/Cmder 上默认启用，因此拖动选择仅复制转录文本，从段落中移除视觉换行列换行符，并保持在转录窗格范围内；按住 Shift 拖动或使用 `--no-mouse-capture` 进行原始终端选择。它在旧版 Windows 控制台（没有 `WT_SESSION` / `ConEmuPID` 的 CMD）和 JetBrains JediTerm（PyCharm/IDEA/CLion 等）内部默认关闭——这些终端声明支持鼠标但将 SGR 鼠标事件作为原始文本转发（#878, #898）。在默认关闭的任何地方使用 `--mouse-capture` 选择加入。原始终端选择可能跨越右侧边栏并包含视觉换行，因为终端（而不是 TUI）控制选择。
- `--profile <NAME>`：选择配置配置文件
- `--config <PATH>`：配置文件路径
- `-v, --verbose`：详细日志记录

## 分支和回滚

DeepSeek-TUI 有三条相关但有意分离的恢复路径：

- `codewhale fork <ID>` 从现有的已保存对话创建新的已保存会话，并记录源会话 ID。这是探索不同答案路径而不覆盖原始会话的安全方式。
- Esc-Esc 回溯将实时转录回退到先前的用户提示，并将该提示恢复到作曲器中进行编辑。
- `/restore` 和 `revert_turn` 工具从 side-git 快照恢复工作区文件。`/restore list [N]` 在选择回滚点之前列出更多快照选项。它们不重写对话历史。

Pi 风格的文件内树浏览器是一个更大的 UI/数据模型项目。v0.8.40 发布了有界的分支/回溯原语和显式谱系元数据。
