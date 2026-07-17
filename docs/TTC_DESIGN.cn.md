# CodeWhale 中的测试时计算（TTC）—— 设计

状态：**已批准方向**（维护者认可）。综合自三项独立评审——verify 工具实现贡献者、GLM 5.2 和一次内部分析——结论一致。本文档即为规范；实现在 stopship 之后（v0.8.69）落地，并且已拆分，因此本文档中的任何内容都不会阻塞 v0.8.68 版本发布。

## TTC 在此处的含义

在决策时投入*更多推理*以获得更好的答案，由**智能体自行判断**——而非始终开启的固定开销。两个能力：

- **(A) 智能体调用的 `verify` / 批评审查（critic pass）**——模型选择在声称完成之前对其最近的成果进行对抗性审查，以捕获"通过但错误"的情况（典型场景：某个修复通过了 16/16 的 CI，但只覆盖了 CLI 路径，未覆盖交互式 TUI；批评审查捕获了该问题，而确定性 CI 未能捕获）。
- **(B) 子智能体的推理能力升级**——解除对 `Low` 级别的硬性上限，使 Fleet 角色能按其任务所需的级别进行思考。

## 核心原则：一个 `CriticEngine`，三种触发方式

抽取出一个单一的 `CriticEngine`，它拥有：**目标上下文快照**（最近的工具调用 + 声称完成的状态 + diff/证据收集）、**提示词模板族**（对抗性的"反驳它"）、**推理力度**（Max）、**禁用工具**标志，以及**结构化裁决模式**（`verdict: pass|fail|uncertain`，findings `[{severity, issue, evidence, suggested_fix}]`，`unresolved_risk`）。

然后由三个*不同的*入口点共享该引擎，但**各自保持独立的调用契约**——不要合并这些触发方式：

| 触发方式 | 契约 | Issue |
|---|---|---|
| `verify` **工具** | 同步，模型选择，默认开启 | #4196（MVP 在 PR #4199 中） |
| advisor **观察者** | 异步，限速，默认关闭 | #3982 |
| verification **关卡** | 回合后，确定性（编译/测试/lint/review） | #4013 |

> 统一**引擎**，绝不统一**触发方式**。将同步/模型选择 + 异步/节流 + 回合后/确定性合并在一起，只会制造一个怪物。共享引擎可保持零漂移，同时各入口点保持自身特性。

## (A) `verify` 工具

**为什么是工具而不是批评子智能体：**`verify` 工具*在结构上同构于现有的 `review` 工具*——相同的 `ToolSpec` trait、相同的 `ToolRegistryBuilder` 路径、相同的 `Feature` 门控、相同的 `MessageRequest` 推理规范化。它继承了所有现有保证，几乎不增加新的暴露面。批评*子智能体*将是一个**带有第二个策略面的第二个运行时**（spawn 深度、allowlist、子智能体层级解析）——典型的拼凑式设计味道。（一个能自主探索的批评子智能体，作为可选跟进功能，可能在 #4193 的 spawn 工作稳定后*以同一工具契约为底层*回归——但它 NOT 是默认方案。）

**接口：**通过 `ToolRegistryBuilder::with_verify(critic)` 注册，由 `Feature` 标志门控。输入：`claim`（必填）+ 可选的 `requirement`、`scope`（`diff|staged|none`）、`base`、`files[]`、`focus`。它确定性地快照证据（按 scope 取 diff —— **包括给定 base 时未提交的工作树变更**，按 PR #4199 的修复——加上指定文件），构建一条 `ReasoningEffort::Max` 且**禁用工具**的 `MessageRequest`，并将结构化裁决作为工具结果返回。

**接入位置：**标准工具循环。无需新的控制面。模型像调用 `read`/`edit`/`review` 一样调用它。

**模型如何决策（且滥用受限）：**
- **Constitution 规则**（harness 强制执行，非文字描述）：在调试、多文件变更、安全敏感编辑、或涉及*不同交互面*（CLI 与 TUI、同步与异步）的变更时，在声称完成之前进行 verify。绿色 CI 但实际错误的情况是典型触发场景。
- **引擎级限速 / `TtcBudget`**：每回合（一次 verify）和每会话预算，保存在**引擎**中，而非 `MessageRequest` 中。
- **反馈循环**：裁决结果返回给模型；连续数次干净裁决应通过 Constitution 规则抑制该会话后续的调用。模型看到自身的命中率并自我纠正。

**裁决语义**（PR #4199，已硬化）：任何**中等严重程度或以上**的发现强制使 `unresolved_risk = true`，并将 `upheld` 裁决降级为 `uncertain`；仅 `low` 级别的小问题可豁免。默认为建议性质；Constitution 规则可将 `fail`/`unresolved_risk` 裁决作为智能体在声称完成前必须处理的事项（harness 中的软阻塞，而非工具中的硬编码）。

**递归/成本限制：**
1. *构造限制*：在 critic 调用内禁用工具 ⇒ 无法进一步工具调用。
2. *注册表限制*：构建 `SubAgentRuntime` allowlist 时**从结构上拒绝** `verify`——通过 `ToolRegistryBuilder` 检查的 `Feature::CriticProducerOnly`（或等效物）。spawn 深度守卫仅是*备用*防线，而非主要防线。
3. *预算限制*：引擎查询每会话 `TtcBudget`。

## (B) 子智能体推理——将天花板替换为地板

`auto_reasoning.rs` 中的 bug 从来都不是"Low 是错的"——问题在于 **Low 是子智能体的*天花板***。修复：**Low 保持为默认*地板*；移除天花板。**

**层级解析顺序：**`Profile (#4137) → 显式任务覆盖 → 会话默认 → Low`。
- `SubAgentRuntime.reasoning_effort` 继续原样转发。
- 子智能体内的 `Auto` 通过**Fleet 角色感知解析器**解析（`review` 角色 profile 固定为 High，`search` 角色固定为 Low，`planner` 固定为 Max）——而不是通过全局关键词解析器。
- `agent` 工具的 `reasoning_effort` 变为"从 Fleet profile 继承，除非在 spawn 时显式覆盖。"

**不要更改默认地板 Low**——子智能体流量主要是搜索/查找，静默提高地板将提升所有现有 fleet 的成本。不制造意外优于自作聪明。这与 #4137（profile 携带 reasoning 层级及 provider/model）组合使用，而非与之竞争。

## 反模式（在 CodeWhale 中特别显拼凑的设计）

1. **第二个 critic 实现**——如果 `verify`/`review`/#3982/#4013 各自实现自己的 prompt+调用+解析，四个路径在第一个 bug 时就分叉了。单一的 `CriticEngine` 就是全部游戏规则。
2. **用于推理升级的非工具控制面**——CodeWhale 的模型契约是工具驱动的；旁路通道破坏对称性，并渗透到每个 provider 适配器。`verify` 调用*就是*升级（内部使用 Max）。一套词汇。
3. **Constitution 文字中描述的递归策略**——在注册表构建器中强制执行；深度守卫是辅助的。
4. **成本核算泄漏到 `MessageRequest` 中**——预算属于引擎 + 会话 `TtcBudget`。不要让每个工具都感知成本。
5. **`auto_reasoning.rs` 变得 TTC 感知**——Auto 解析*每回合的能力级别*；模型决定 verify。保持它们分离，否则用户将面对无法理解的不可确定性。
6. **#4013 与 `verify` 之间的关卡排序模糊**——不同的生命周期点（回合中/模型选择 vs 回合后/确定性）。在 Constitution 中记录，以便贡献者不会合并它们。
7. **混淆观察者的契约与 verify 的契约**——#3982 是异步/节流/关闭；`verify` 是同步/选择/开启。共享引擎；绝不共享触发方式。

## Issue 映射与排序

- **#4196**——`verify` 工具。MVP 在 **PR #4199**（直接 critic，Max，禁用工具，递归守卫，裁决已硬化）。在合并前重构到提取出的 `CriticEngine` 上。*配置隔离（`crates/tui/src/tools/`）。*
- **CriticEngine 提取**——新增；将 `review` 的调用/解析重构到共享引擎中，然后让 `verify` 消费它。是后续将 #3982/#4013 接入的前提。
- **#4137**——Fleet profile 携带 `reasoning` 层级；驱动 (B)。*涉及 `crates/config`——与配置工作排序，并在 #4136（规范 AgentProfile）和 #4193（已落地）之后。*
- **(B) 解析器**——`auto_reasoning.rs` 天花板→地板 + Fleet 角色感知 Auto 解析。
- **#3982 / #4013**——rebase 到 `CriticEngine` 上作为额外的触发方式（后续）。

以上全部属于 **v0.8.69，在 v0.8.68 stopship 之后**（stopship 已绿灯）。
