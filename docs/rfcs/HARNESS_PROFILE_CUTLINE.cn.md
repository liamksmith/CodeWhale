# Harness Profile 截止线

本笔记定义了 v0.9.0 的 HarnessProfile 工作顺序。自动 Harness Creator 必须在 profile 模式、解析器、种子 profile 和用户可见的状态界面明确且经过测试之后才能运行。

## 决策

对于 v0.9.0，CodeWhale 应首先将 harness profile 视为类型化策略数据。自动 profile 演进推迟到回放证据、候选清单和晋升门禁存在之后。

第一个实现通道止于：

1. `HarnessPosture` 枚举和策略开关。
2. `HarnessProfile` 模式和注册表。
3. 确定性 profile 解析器。
4. 常见模型家族的种子 profile。
5. 仓库 constitution 覆盖输入。
6. 已解析的 provider、模型、profile 和仓库法则的状态/UX 显示。

仅在这些界面可见且经过测试后，CodeWhale 才应添加证据存储、候选清单、晋升门禁或代理式的 Harness Creator。

## 必需的种子 Profile

| 模型家族 | 预期姿态 | 说明 |
| --- | --- | --- |
| DeepSeek V4 Pro / Flash | cache-heavy | 保持前缀稳定性和大上下文连续性。 |
| Xiaomi MiMo V2.5 Pro / UltraSpeed / V2.5 | cache-heavy | 类似的长上下文/缓存姿态，但路由和认证与 DeepSeek 保持分离。较旧的 V2 Flash 名称是历史示例，而非当前的直接 provider 默认值。 |
| Arcee Trinity Thinking | cache-heavy 或显式 Arcee profile | 直接 Arcee ID（如 `trinity-large-thinking`）不得隐藏在 OpenRouter 别名后面。 |
| Hugging Face / 本地 / 开放权重路由 | lean | 偏好较小的上下文包、更严格的工具界面和面向子代理的分解。 |
| 通用 OpenAI 兼容网关 | standard（除非匹配） | 不要仅从裸端点推断 provider 特定姿态。 |

Provider 路由、端点、模型 id、HarnessProfile 和仓库 constitution 必须分别可见。profile 解析器可以选择一个 profile，但不得静默更改 provider 认证、基础 URL、模型 ID、工具允许列表或仓库权限。

## 仓库 Constitution 边界

`.codewhale/constitution.json` 是本地仓库法则，而非另一个 provider profile。解析器可以在项目信任检查后将其作为输入读取，但 profile 选择必须同时显示：

- 面向模型的姿态，如 `cache-heavy` 或 `lean`；
- 仓库法则来源，如 `.codewhale/constitution.json` 或 none。

## 自动演进边界

AHE/GEPA 风格的 profile 演进是未来工作。只有在文本区分以下阶段后才可作为灵感引用：

1. 从记录的证据中提出候选；
2. 针对较弱或受限的学生进行回放/评估；
3. 带有必需测试和策略检查的晋升门禁决策；
4. 可检查的覆盖更新或回滚。

在模式/解析器/显示通道中，不得静默晋升、变更任何 v0.9.0 harness profile，或将其写入缓存主覆盖层。

## 冒烟证据

在 v0.9.0 发布包含超出模式解析和纯解析器检查的 HarnessProfile 运行时行为之前，验收矩阵应记录以下证据：

- DeepSeek V4 解析为 cache-heavy profile；
- Xiaomi MiMo 解析为 cache-heavy profile，不共享 DeepSeek 认证；
- Arcee 直接 `trinity-large-thinking` 通过直接 `arcee` 路由解析，而非 OpenRouter `arcee-ai/trinity-large-thinking` 别名；
- 通用/HF/本地模型解析为 lean 或 standard profile；
- TUI 或运行时状态界面分别显示 provider、模型、profile 和仓库 constitution；
- 在正常 Agent 或 WhaleFlow 运行期间不进行自动 profile 变更。

对于 v0.9.0，纯解析器测试可以满足 profile 选择证据，但状态显示和运行时使用保持推迟，直到单独的 PR 有意接线这些界面。发布说明仍应将 HarnessProfile 称为类型化模式/解析器基础，而非自动 harness 创建器。
