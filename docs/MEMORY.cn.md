# 用户记忆

用户记忆功能为模型提供了一个小型的持久笔记文件，
该文件在每个轮次都会被注入到系统提示中。这是一个存放跨会话偏好的地方——
"我更喜欢 pytest 而不是 unittest"、"这个代码库使用 4 空格缩进"、
"提交前始终运行 `cargo fmt`"——无需在每次对话中重复说明。

记忆功能是**可选的**。当禁用时（默认），不会加载任何内容，
不会拦截任何操作，`remember` 工具也不会暴露给模型。
对于尚未启用该功能的用户，这保持了零开销的行为。

## 启用记忆

可以设置环境变量：

```bash
export DEEPSEEK_MEMORY=on
```

接受的 truthy 值为 `1`、`on`、`true`、`yes`、`y` 和
`enabled`。

……或者添加到 `~/.codewhale/config.toml`：

```toml
[memory]
enabled = true
```

切换后重启 TUI。禁用方式与此相反。

记忆文件默认位于 `~/.codewhale/memory.md`；可以通过
`config.toml` 中的 `memory_path` 或环境变量 `DEEPSEEK_MEMORY_PATH`
进行覆盖。当同时设置时，`DEEPSEEK_MEMORY_PATH` 优先于配置文件。
现有的 `~/.deepseek/memory.md` 文件在不存在 `.codewhale` 记忆文件时
作为旧版回退继续受到支持。

## 快速示例

```text
# remember that this repo prefers cargo fmt before commits
/memory
/memory path
/memory edit
/memory help
```

- 在输入框中输入 `# remember that this repo prefers cargo fmt before commits`
  即可追加一条带时间戳的条目，而不会触发对话轮次。
- 运行 `/memory` 可以确认功能写入位置以及当前存储的内容。
- 运行 `/memory edit` 可以在编辑器中手动整理文件。

## 注入的内容

当记忆功能启用且文件存在时，每个轮次的系统提示
都会携带一个额外的块：

```xml
<user_memory source="/Users/you/.codewhale/memory.md">
- (2026-05-03 22:14 UTC) prefer pytest over unittest
- (2026-05-03 22:31 UTC) this codebase uses 4-space indentation
…
</user_memory>
```

该块位于提示组装中的易变内容边界之上，
因此它保持在 DeepSeek 的前缀缓存中，逐轮次持续有效。
文件在每次构建提示时读取——通过 `/memory` 或外部编辑器进行的编辑
将在下一个轮次生效，无需重启。

大于 100 KiB 的文件会被加载但进行截断，并附加一个标记
以便您看到截断位置。

## 向记忆添加内容的三种方式

### 1. `# ` 输入框前缀（#492）

在输入框中输入以 `#` 开头（但不是 `##` 或 `#!`）的单行内容：

```
# remember to use 4-space indentation in this repo
```

TUI 会拦截该输入并追加一条带时间戳的条目到
记忆文件。**不会触发对话轮次**——您的输入被消费，状态栏
确认写入路径，然后您可以继续输入真正的问题。

多 `#` 前缀会故意穿透到正常的轮次提交，
以便您可以粘贴 Markdown 标题而不会意外触发。

### 2. `/memory` 斜杠命令（#491）

检查、清除或获取有关编辑文件的提示：

| 子命令          | 效果                                                 |
|---------------------|--------------------------------------------------------|
| `/memory`           | 内联显示解析后的路径和当前内容    |
| `/memory show`      | 无参数形式的别名                              |
| `/memory path`      | 只打印解析后的路径                          |
| `/memory clear`     | 用空标记替换文件                 |
| `/memory edit`      | 打印 `${VISUAL:-${EDITOR:-vi}} <path>` shell 命令行 |
| `/memory help`      | 显示命令特定帮助和当前路径       |

`/memory edit` 形式故意只打印命令，而不是
在进程内启动编辑器——这样保持了斜杠命令处理器的
简洁性，与您使用的编辑器无关。

您也可以从通用帮助界面发现该功能：

- `/help memory` 显示斜杠命令摘要和使用行。
- `/memory help` 打印记忆特定子命令以及解析后的路径。

### 3. `remember` 工具（自动更新，#489）

当记忆功能启用时，模型会获得一个 `remember` 工具，形状如下：

```json
{
  "name": "remember",
  "description": "Append a durable note to the user memory file...",
  "input_schema": {
    "type": "object",
    "properties": {
      "note": { "type": "string", ... }
    },
    "required": ["note"]
  }
}
```

当模型注意到值得跨会话保留的持久偏好、约定
或事实时，会使用此工具。该工具是自动批准的，
因为写入范围仅限于用户自己的记忆文件——将其置于
标准写入批准流程之后会违背自动记忆捕获的初衷。

如果模型将 `remember` 用于临时任务状态（"I'm
currently editing foo.rs"），结果无害但会浪费
上下文。工具的描述明确告诉模型**不要**这样做——
只记录持久的、单句的笔记。

## 文件格式

记忆是带有时间戳条目的纯 Markdown：

```markdown
- (2026-05-03 22:14 UTC) prefer pytest over unittest
- (2026-05-03 22:31 UTC) this codebase uses 4-space indentation
- (2026-05-04 09:02 UTC) all PRs need 2 reviewers before merge
```

您可以在任何编辑器中手动编辑该文件——加载器不关心时间戳格式；
它只是将整个文件作为记忆块读取。时间戳是一个约定，
以便在整理文件时知道每条笔记是何时添加的。

## 层级与导入

记忆是有意**用户作用域**而非仓库作用域的。它与项目指令来源
（如 `AGENTS.md`、`.codewhale/instructions.md`、旧版 `.deepseek/instructions.md`
和 `instructions = [...]`）并列而非内嵌。

- 使用**记忆**存放应随您跨仓库和会话的持久个人偏好。
- 使用**项目指令**存放应随代码库传播的仓库特定约定。

记忆加载器目前逐字读取一个解析后的文件路径。
当前**不**支持 `@path` 导入/包含；如果您需要
更大的可重用指令包，请将其放入项目指令
文件或技能中。

## 不应放入记忆的内容

记忆用于存放**持久**信号。不应存放在其中的内容：

- **密钥**——不要放 API 密钥、令牌、密码。该文件是磁盘上的
  纯文本，并会被逐字注入到系统提示中。
- **临时任务状态**——"I'm currently working on the parser"
  每个会话都不同；不应放在跨会话记忆中。
- **对话片段**——引用式笔记应放入笔记工具（`note`），
  而非记忆。
- **长指令**——超过几句话的内容应放在
  `AGENTS.md`（项目级别）或[技能](../crates/tui/src/skills/mod.rs)
  （可重用指令包）中。

## 隐私与作用域

记忆文件完全位于您本机的 `~/.codewhale/` 中。
它永远不会被上传到任何云服务——TUI 只会在系统提示中
通过内存内读取的方式引用它。如果您禁用该功能，文件会保留
在磁盘上，但要再次注入必须重新启用。

这意味着记忆是一个纯粹的本地构造。在
不同机器之间同步它是您的责任——像任何其他本地
配置一样对待 `~/.codewhale/memory.md` 文件。

## 设计决策

- **默认关闭。** 该功能不主动征求内容、写入内容
  或在未经显式选择加入的情况下暴露工具。
- **作用域仅为用户——而非仓库。** 记忆是故意不去中心化的：
  它没有项目级别的对等项、没有分层覆盖、
  也没有合并语义。这保持了零配置项目的安全性。
- **工具是自动批准的。** 记忆工具是通过系统提示
  插件注入的，而非标准的工具注册路径，
  以便它可以在 TUI 中独立于模型可见工具集进行调整。
  它是自动批准的，因为文件受文件系统所有权保护，
  路径是可审计的，并且 gating 会破坏自动捕获。
- **注入点保持在缓存中。** 记忆块位于
  提示前缀中所有轮次可变内容之上，因此它在
  DeepSeek 的前缀缓存中逐轮次保持命中，不会使推理成本翻倍。
