# CodeWhale 用户指南

本指南适用于你使用 CodeWhale 的第一个小时。它解释了主要工作流程、重要的安全控制，以及当你需要完整参考时下一步该去哪里。

CodeWhale 提供了更深入的参考文档，涵盖安装、配置、提供商、模式、快捷键绑定、工具和操作。将本页作为引导式教程，然后在需要每个选项时跟随"下一步"链接。

## 1. 欢迎使用 CodeWhale

CodeWhale 是一个终端编码代理。你从工作区运行它，给它一个任务，它可以使用结构化工具来检查文件、运行命令、编辑代码，并附带证据进行报告。

与普通聊天模型的重要区别在于，CodeWhale 是围绕一个 harness 构建的：

- 它保持活动工作区和会话可见。
- 它将每一轮通过显式模式和审批规则进行路由。
- 它在转录中显示工具调用，而不是隐藏工作。
- 它可以保存会话、分支对话以及稍后继续。
- 它可以运行子代理进行专注的后台工作。

你可以使用 CodeWhale 处理小问题：

```text
Explain the authentication flow in this repository.
```

你也可以用它进行多步骤工作：

```text
Find the failing validation path, propose a fix, and wait for my approval
before editing files.
```

对于一个新的仓库，请保守地开始。在要求 CodeWhale 更改文件之前，先要求它探索和计划。这样你就有了一个可审查的路径，并且更容易及早发现错误的假设。

下一步：[ARCHITECTURE.md](ARCHITECTURE.md) 解释了内部 harness 和运行时模型。

## 2. 首次启动

使用适合你机器的路径安装 CodeWhale。每个受支持的安装路径都提供 `codewhale` 调度器和 `codewhale-tui` 运行时。

```bash
# npm
npm install -g codewhale

# Cargo
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked

# Homebrew，仅限旧版安装
# tap/formula 仍使用旧的 deepseek-tui 名称。在新的安装中推荐使用 npm、
# Cargo、Docker 或直接下载，直到 formula 被重命名。
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

当你想要隔离的运行时时，也可以使用 Docker：

```bash
docker volume create codewhale-home
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:latest
```

从你希望 CodeWhale 工作的仓库或目录启动它：

```bash
codewhale
```

首次启动时，CodeWhale 会启动一个简短的 constitution 优先的设置路径：选择语言、审查提供商/模型就绪状态、审查运行时姿态，然后创建或确认你的 CodeWhale constitution。捆绑/默认 constitution 是有效的，你可以稍后使用 `/setup` 重新访问设置中心。

DeepSeek 是默认提供商。如果你想在首次启动之前或之后配置其密钥，最直接的设置路径是：

```bash
codewhale auth set --provider deepseek
```

你也可以通过环境变量提供密钥：

```bash
export DEEPSEEK_API_KEY="your-key"
codewhale
```

新的 CodeWhale 配置存储在 `~/.codewhale/config.toml` 下。旧版 `~/.deepseek/config.toml` 文件仍然支持从旧名称迁移的用户。

使用 `/constitution` 审查或更改常设指导。设置完成后，运行 doctor 检查：

```bash
codewhale doctor
```

当你需要机器可读的报告来提交 issue 时，使用 JSON 格式：

```bash
codewhale doctor --json
```

如果 doctor 命令报告被拒绝的密钥来自环境变量，在测试已保存的配置之前，删除或替换该环境变量。

下一步：[INSTALL.md](INSTALL.md) 涵盖平台特定的安装路径，[CONFIGURATION.md](CONFIGURATION.md) 涵盖配置解析，[PROVIDERS.md](PROVIDERS.md) 涵盖提供商 ID 和凭据。

## 3. 你的第一个任务

在真实工作区中从一个只读任务开始：

```text
Map the repository structure and tell me where the CLI entrypoint lives.
```

然后请求一个专注的计划：

```text
I want to add a small validation for empty config values. Inspect the relevant
code and propose the smallest safe change before editing anything.
```

当你准备好进行编辑时，明确说明验收标准：

```text
Implement the validation you proposed. Keep the change scoped to config
parsing, add or update the narrowest test, and run the relevant check.
```

好的首次提示词包含四个细节：

- 你想要的结果。
- 你关心的文件、功能或行为。
- 什么超出了范围。
- 什么验证应该算作完成。

例如：

```text
Fix the broken provider error message in the config loader. Do not change the
provider registry. Add a regression test and run only the config crate tests.
```

如果你不确定 bug 在哪里，请说明：

```text
Investigate why `codewhale doctor` reports the wrong provider. Do not edit
files yet. Return the likely cause, evidence, and a proposed patch plan.
```

对于不熟悉的代码，CodeWhale 在调查和实现分步进行时效果最好。对于小的、易于理解的更改，单个实现请求是可以的。

下一步：[MODES.md](MODES.md) 解释了何时使用 Plan、Agent 和 YOLO。

## 4. 理解界面

交互式 TUI 有几个稳定的区域：

- 标题栏：当前会话、活动模型、模式和高层状态。
- 转录区：对话、工具调用、命令输出摘要和模型响应。
- 编辑器：你输入提示词、斜杠命令和文件提及的地方。
- 侧边栏：工作状态、任务、代理或相关会话信息的上下文面板。
- 状态和页脚区域：实时活动、排队后续任务和简短命令提示。

页脚状态行是可配置的。运行 `/statusline` 来选择哪些页脚指示器可见，或在 `config.toml` 中设置 `[tui].status_items` 来控制选择和顺序。当前支持的键包括 `mode`、`model`、`cost`、`balance`（仅限 DeepSeek / DeepSeekCN）、`status`、`agents`、`reasoning_replay`、`prefix_stability`、`cache`、`context_percent`、`git_branch`、`last_tool_elapsed`（保留）、`rate_limit`（保留）和 `tokens`。省略 `status_items` 以保持内置默认顺序；将其设置为 `[]` 以隐藏可配置的指示器。

转录区是审计跟踪。当 CodeWhale 读取文件、运行命令或编辑代码时，操作会出现在那里。如果命令失败，使用可见的失败输出作为你下一步指令的一部分，而不是重新开始。

编辑器接受普通提示词和斜杠命令。输入 `/` 来发现可用的命令。当你希望模型专注于特定文件或目录而不是广泛搜索时，使用文件提及。

侧边栏在多步骤轮次中很有用。它可以在转录区继续增长的同时，保持目标、代理状态和上下文信息可见。

键盘快捷键因上下文、终端和平台而异。本指南避免重复完整的快捷键目录，以免与 TUI 产生漂移。

下一步：[KEYBINDINGS.md](KEYBINDINGS.md) 是完整的快捷键参考。

## 5. 模式

CodeWhale 有三种可见的 TUI 模式：

| 模式 | 用于 | 默认姿态 |
| --- | --- | --- |
| Plan | 更改前的探索、设计和审查 | 只读调查 |
| Agent | 普通的多步骤编码工作 | 带审批门控的工具使用 |
| YOLO | 你希望自动执行的受信任仓库 | 自动审批和信任 |

从 TUI 使用模式选择器切换模式：

```text
/mode
```

或直接切换：

```text
/mode plan
/mode agent
/mode yolo
```

Plan 模式是在不熟悉的仓库中最安全的起点。它用于检查和决策，而不是文件编辑。
对于非平凡的工作，Plan 模式的确认提示可以显示一个基于证据的 PlanArtifact：目标、上下文、使用的来源、关键文件、约束、方法、验证计划、风险和交接说明。当代理使用丰富的工件形状时，空的部分是可见的，因此你可以要求修订而不是接受一个不够详细的计划。

Agent 模式是大多数贡献工作的默认模式。它允许 CodeWhale 读取、运行检查和编辑文件，同时将危险操作限制在审批门控之后。

YOLO 模式适用于你希望模型在不停止等待审批的情况下行动的受信任工作区。不要在不信任的仓库中使用它。

模式与模型路由是分开的。当编辑器空闲时，`Tab` 循环可见模式，而 `/model auto` 控制每轮的模型和思考选择。

你也可以从 `/config` 通过编辑审批模式来更改审批行为。仅在你理解它如何改变工具执行时才使用此功能。

下一步：[MODES.md](MODES.md) 提供了完整的模式、审批和信任模式参考。

## 6. 斜杠命令

斜杠命令在编辑器中输入。当你希望直接更改 CodeWhale 状态而不是用自然语言向模型提问时，它们非常有用。

首次用户的常用命令：

| 命令 | 用途 |
| --- | --- |
| `/mode` | 打开模式选择器或使用 `/mode agent` 切换 |
| `/model` | 选择模型或使用 `/model auto` |
| `/provider` | 选择活动 API 提供商 |
| `/fleet` | 配置 Fleet 角色或打开 worker 状态 |
| `/config` | 编辑运行时和提供商设置 |
| `/statusline` | 选择哪些页脚状态指示器可见 |
| `/compact` | 压缩长上下文以回收 token 预算 |
| `/review` | 请求结构化审查工作流 |
| `/memory` | 在启用时检查或管理记忆 |
| `/mcp` | 配置或检查 MCP 服务器集成 |

工具箱命令在你直接输入时保持可搜索：`/models` 获取实时端点 ID，`/modeldb` 打开捆绑的模型参考，`/rlm` 打开手动持久 RLM 上下文。

当你想从默认的 DeepSeek 路由切换时使用 `/provider`。提供商 ID、环境变量、模型默认值以及能力说明保存在提供商注册表文档中。

软自动多代理工作：[AUTOMATIC_WORKFLOWS.md](AUTOMATIC_WORKFLOWS.md)。

持久多 worker 工作的下一步：[FLEET_WORKFLOW_TUTORIAL.md](FLEET_WORKFLOW_TUTORIAL.md) 教你 Fleet 任务规范、监控和 Workflow 编写。

当你希望 CodeWhale 每轮自动选择模型和思考级别时使用 `/model auto`。当你需要可重复的比较或严格的成本配置时使用固定模型。

当会话变长且模型开始携带过多历史记录时使用 `/compact`。压缩将原始转录细节替换为简洁的工作摘要。

本指南有意不列出每个命令。命令界面的变化比 onboarding 流程更频繁，当你在会话中时，TUI 命令面板是真理来源。

下一步：[CONFIGURATION.md](CONFIGURATION.md) 涵盖运行时设置，[MCP.md](MCP.md) 涵盖 Model Context Protocol 集成。

## 7. 使用工具

CodeWhale 工具是结构化的操作。模型不仅生成散文，还可以调用工具来检查和更改工作区。

工具支持的工作示例包括：

- 在解释文件之前读取它。
- 在提议重构之前搜索调用点。
- 运行一个专注的测试命令。
- 应用一个小型补丁。
- 打开一个子代理进行并行调查。

工具使用受模式、审批和沙箱策略的约束。确切的行为取决于当前模式和配置，但基本规则很简单：在 Plan 模式下开始只读探索，在 Agent 模式下进行正常更改，将 YOLO 保留给受信任的自动化。

工作区边界很重要。CodeWhale 被期望在你启动它的目录或你配置的工作区中工作。当任务应该保持在仓库内时，请明确说明：

```text
Only inspect and edit files under this repository. Do not touch parent
directories or global config.
```

当命令需要网络、在工作区外写入或有风险的 shell 操作时，除非你配置了更宽松的行为，否则会弹出审批提示。

好的工具指令是具体的：

```text
Run the narrowest test that covers this parser change. If it fails, report the
failure and stop before broadening the test scope.
```

避免在专注修复期间要求广泛的清理。较小的工具范围使转录更容易审查，最终差异更容易合并。

下一步：[TOOL_SURFACE.md](TOOL_SURFACE.md) 列出了工具界面，[SANDBOX.md](SANDBOX.md) 解释了沙箱行为。

## 8. 子代理和并行工作

子代理是后台子代理。父会话给子代理一个专注的任务，接收一个代理 id，可以在子代理运行时继续工作。

主要的编排工具是：

- `agent`：启动一个带有任务和角色的专注子代理。子代理在后台运行，并返回一个紧凑的收据加上转录句柄。

你通常不需要直接调用这些工具。用简单的语言请求并行工作：

```text
Open one read-only explorer for the config crate and another for the TUI
provider picker. Have both return file references and risks before we plan the
fix.
```

有用的角色包括：

| 角色 | 适用于 |
| --- | --- |
| `general` | 多步骤任务；未指定角色时的默认值 |
| `explore` | 只读代码映射 |
| `plan` | 设计和迁移规划 |
| `review` | 对现有更改的以 Bug 为重点的审查 |
| `implementer` | 严格指定的编辑 |
| `verifier` | 运行检查并报告通过/失败证据 |

子代理在工作可以被清晰地分离时最有用。不要将它们用于微小的编辑，也不要让多个代理同时写入相同的文件。

下一步：[SUBAGENTS.md](SUBAGENTS.md) 涵盖了角色、生命周期、并发和输出约定。

## 9. 技能

技能是可复用的指令包。技能通常是一个 `SKILL.md` 文件，教 CodeWhale 如何执行重复的工作流、使用工具系列或遵循项目约定。

在任务具有可重复过程时使用技能：

- 审查特定类型的 PR。
- 处理文档或电子表格格式。
- 遵循团队发布清单。
- 使用特定项目的记忆或 wiki 工作流。

在 TUI 中，`/skill` 在技能可用时激活技能，`/skills` 列出已安装的技能。命令面板也可以在普通斜杠命令旁边显示技能条目。

好的技能是范围狭窄的。它们应该告诉模型要遵循什么工作流、收集什么证据以及要避免什么。它们不应该隐藏凭据或替代普通的仓库文档。

如果仓库有自己的说明，将它们视为活动工作的一部分。在编辑之前阅读本地指导，并将任何贡献保持在仓库的约定范围内。

下一步：参见 [CLAUDE_PLUGIN_COMPAT.md](CLAUDE_PLUGIN_COMPAT.md) 了解 Claude Code 技能/插件兼容性，以及 [CONFIGURATION.md](CONFIGURATION.md) 了解配置路径和项目权限。

## 10. 获取帮助

从 doctor 输出开始：

```bash
codewhale doctor
```

提交详细 issue 时使用 JSON：

```bash
codewhale doctor --json
```

对于认证问题，检查哪个来源在生效：已保存的配置、密钥环、环境变量还是显式的启动标志。一个过时的 `DEEPSEEK_API_KEY` 环境变量可能会覆盖你期望使用的。

对于提供商问题，确认活动提供商和模型：

```text
/provider
/model
```

对于长而混乱的会话，使用 `/compact` 来减少上下文压力，或在同一工作区中启动一个新会话并总结你需要的内容。

报告 issue 时，包括：

- CodeWhale 版本。
- 安装方法。
- 操作系统和终端。
- 提供商和模型。
- 确切的命令或提示词。
- 相关的 doctor 输出。
- 问题是否发生在新工作区中。

不要将 API 密钥、私有源代码或密钥粘贴到公共 issue 中。

下一步：[OPERATIONS_RUNBOOK.md](OPERATIONS_RUNBOOK.md) 包含操作分类和恢复步骤。

## 常见问题

### CodeWhale 只适用于 DeepSeek 吗？

DeepSeek 是默认且优先的路由，但 CodeWhale 也支持其他托管和本地 OpenAI 兼容的提供商。使用 `/provider` 或 `codewhale --provider <id>` 选择提供商。配置非默认路由时，请保持提供商注册表为打开状态。

### 我应该先使用哪种模式？

对不熟悉的代码使用 Plan，对正常实现使用 Agent，仅在可接受自动执行的受信任仓库中使用 YOLO。

### 为什么 CodeWhale 在运行命令之前要询问？

审批是安全模型的一部分。Shell 命令、付费工具、写入以及超出预期工作区的操作可能会产生副作用。审批提示让你保持控制，同时仍允许模型执行有用的工作。

### 如何在 macOS 上运行 Python 文件？

在包含该文件的文件夹中打开终端并运行：

```bash
python3 your_file.py
```

如果 macOS 说 `python3` 缺失，从 [python.org](https://www.python.org/downloads/macos/) 安装 Python，或使用 Homebrew：

```bash
brew install python
```

在 CodeWhale 中，要求代理检查文件并使用 `python3 your_file.py` 运行它。如果脚本需要包，先在虚拟环境中安装它们：

```bash
python3 -m venv .venv
source .venv/bin/activate
python3 -m pip install -r requirements.txt
python3 your_file.py
```

### 我的配置存储在哪里？

新的 CodeWhale 配置使用 `~/.codewhale/config.toml`。旧版 `~/.deepseek/config.toml` 仍然被支持以保持兼容性。当工作区配置存在时，项目覆盖也可以影响行为。

### 如何保持成本可预测？

使用 `/model auto` 进行路由，当你需要严格的配置时选择固定模型，并压缩长会话。对于大型任务，要求 CodeWhale 在实现之前先计划，这样你就不会把 token 浪费在错误的路径上。

### 如何继续之前的工作？

CodeWhale 保存会话。使用会话选择器或 README 和模式指南中记录的恢复/继续 CLI 路径。对于有风险的实验，在改变方向之前分支会话。

`/sessions` 选择器默认以当前工作区为范围启动，因此恢复的会话保持附加到你打开的项目。在选择器中按 `a` 显示来自所有工作区的会话，或运行 `codewhale sessions` 列出所有已保存的会话及其上次更新的时间戳，然后再恢复特定的 id。

### 当模型感到困惑时我应该怎么做？

停止并重新陈述目标、约束和当前证据。如果转录很长，使用 `/compact` 或启动一个带有简短交接的新会话。如果问题是操作性的，运行 `codewhale doctor` 并检查报告的配置和提供商状态。

### 我应该将项目规则放在提示词中还是文件中？

对于持久的项目规则使用仓库文件，对于轮次特定的意图使用提示词。如果一个工作流跨项目重复出现，考虑将其转化为技能。

### CodeWhale 可以编辑当前仓库之外的文件吗？

这取决于工作区边界、沙箱设置、信任模式和审批策略。对于贡献工作，除非你有意需要其他内容，否则将指令限制在当前仓库范围内。

### 阅读本指南后我应该去哪里？

阅读你正在更改的事物的专注参考。对于大多数用户，接下来的页面是安装、配置、提供商、模式、快捷键绑定、工具和子代理。

下一步：[INSTALL.md](INSTALL.md)、[CONFIGURATION.md](CONFIGURATION.md)、[PROVIDERS.md](PROVIDERS.md)、[MODES.md](MODES.md) 和 [TOOL_SURFACE.md](TOOL_SURFACE.md)。
