# CodeWhale v0.8.68 — Agent 工作流手册

本文档告知自主 agent 如何系统性地完成 v0.8.68 发布。它与以下内容配合使用：

- **里程碑：** `v0.8.68`（GitHub 里程碑 #53）
- **架构跟踪器：** issue [#4175](https://github.com/Hmbown/CodeWhale/issues/4175)（Fleet / Workflow / Lane / Runtime）
- **分类数据包：** issue [#4092](https://github.com/Hmbown/CodeWhale/issues/4092)
- **主检查清单：** `CODEWHALE_0_8_68.md`（工具工作区）或合并后在仓库根目录的跟踪器
- **Workflow 文件：** `workflows/v0868_*.workflow.js`

### 架构阶段（stopship 后的产品工作）

| 阶段 | Issue | 范围 |
|-------|-------|-------|
| 1 | [#4176](https://github.com/Hmbown/CodeWhale/issues/4176) | Lane CLI + Runtime（tmux、worktree、日志） |
| 2 | [#4177](https://github.com/Hmbown/CodeWhale/issues/4177) | Workflow 步骤 → Fleet 角色 |
| 3 | [#4179](https://github.com/Hmbown/CodeWhale/issues/4179) | 角色之间的门禁和交接 |
| Dogfood | [#4178](https://github.com/Hmbown/CodeWhale/issues/4178) | Stopship 作为 fleet 支持的 lane |

词汇：**Fleet** = 谁 · **Workflow** = 什么顺序 · **Lane** = 运行实例 · **Runtime** = 哪里/如何（tmux、VM、CI）。

## 真实来源

- **实现基础：** `main` — 所有 v0.8.68 修复分支都从这里开始。PR #4099 已合并快速切换；不要使用 `work/v0.9.0-cutover` 或 `.cw-worktrees/v0867-pr4047`。
- **`codex/0868-next`：** 仅作为过时参考。仅当特定 issue 需要特定提交时才从中 cherry-pick — 永远不要将其视为活跃开发分支。
- **手册/workflow 定义：** 已在 [PR #4163](https://github.com/Hmbown/CodeWhale/pull/4163) 中合并到 `main`；实现 PR 从 `main` 分支。

## 推迟策略（v0.8.69 / 架构重构）

推迟 v0.8.69 重构和广泛的功能 lane，除非它们**直接解除** stopship issue（#4090、#4093、#4094）的阻塞。

| 类别 | 时间 | 备注 |
|----------|------|-------|
| Stopship（#4090、#4093、#4094） | **现在** | Wave 1 — 阻塞发布 |
| Dogfood 回归（#3986、#3990） | Stopship 之后 | 同一 lane，较低优先级 |
| Catalog lane（Wave 2） | Stopship 绿灯后 | #4109、#4114–#4119、#4139–#4141、#4184–#4188 |
| Workflow UI lane（Wave 3） | Stopship 绿灯后 | #4038、#4110、#4120–#4135 |
| TUI 文案 lane（Wave 4） | Stopship 绿灯后 | #4112、#4142–#4148 |
| v0.8.69 重构 / 0.9.0 架构 | **推迟** | 除非修复 #4090/#4093/#4094 所必需 |

标记为 `v0.8.69` 但仍处于里程碑 `v0.8.68` 的 issue 应在清扫期间重新分类为 DEFER（0.9.0），除非与 stopship 修复相关。

## 快速开始

```bash
# 1. 同步并验证分支（实现始终从 main 开始）
cd CodeWhale
git fetch origin
git checkout main && git pull origin main
git checkout -b codex/v0868-fix-<issue>   # 例如 codex/v0868-fix-4090
git status -sb

# 2. 看板真实状态
gh issue list -R Hmbown/CodeWhale --milestone "v0.8.68" --state open --limit 200
gh pr list -R Hmbown/CodeWhale --state open --limit 50 \
  --json number,title,isDraft,mergeable,milestone

# 3. 阅读分类数据包（不要跳过）
gh issue view 4092 -R Hmbown/CodeWhale

# 4. 在更改前后运行验证门禁
cargo fmt --all --check
cargo clippy --workspace --all-features --locked -D warnings \
  -A clippy::uninlined_format_args -A clippy::too_many_arguments \
  -A clippy::unnecessary_map_or -A clippy::collapsible_if -A clippy::assertions_on_constants
cargo test --workspace --locked
cargo build --release -p codewhale-tui
```

## 执行顺序（waves）

从上到下执行。**在 stopship 绿灯之前不要启动 Waves 2–4 或 v0.8.69 重构**（#4090、#4093、#4094 已关闭或在 `main` 上验证已修复）。

| Wave | Workflow 文件 | 主题 | GitHub Issues | 状态 |
|------|---------------|-------|---------------|--------|
| 0 | `v0868_issue_sweep.workflow.js` | 分类 + 发布计划 | 所有里程碑 | 按需 |
| 1 | `v0868_stopship_lane.workflow.js` | 发布阻塞 + dogfood 回归 | #4090、#4093、#4094、#3986、#3990 | **活跃** |
| 2 | `v0868_catalog_lane.workflow.js` | 模型目录 + Models.dev 实时目录 | #4109、#4114–#4119、#4139–#4141、#4184–#4188 | 推迟 |
| 3 | `v0868_workflow_ui_lane.workflow.js` | Workflow 编排 UI | #4038、#4110、#4120–#4135 | 推迟 |
| 4 | `v0868_tui_copy_lane.workflow.js` | 转录/文案打磨 | #4112、#4142–#4148 | 推迟 |
| 5 | `v0868_release_gate.workflow.js` | 最终验证 + 交接 | 里程碑收尾 | Waves 1–4 之后 |

### Models.dev 实时目录链（Wave 2）

在 stopship 绿灯后按顺序执行：

**#4184 → #4185 → #4186 → #4187 → #4188**

| Issue | 范围 |
|-------|-------|
| [#4184](https://github.com/Hmbown/CodeWhale/issues/4184) | Models.dev 作为 provider/model 元数据的真实来源 |
| [#4185](https://github.com/Hmbown/CodeWhale/issues/4185) | 在目录解析器中接受当前实时 Models.dev schema |
| [#4186](https://github.com/Hmbown/CodeWhale/issues/4186) | 将 Models.dev provider ID 标准化到 CodeWhale provider 类型 |
| [#4187](https://github.com/Hmbown/CodeWhale/issues/4187) | 获取并缓存实时 Models.dev 目录到 ProviderLake |
| [#4188](https://github.com/Hmbown/CodeWhale/issues/4188) | 在实时目录落地后降级精选的捆绑模型数据 |

父跟踪器：[#4109](https://github.com/Hmbown/CodeWhale/issues/4109)。

## 如何启动 Workflow

在启动实现 agent 之前从 `main` 分支：

```bash
git checkout main && git pull origin main
git checkout -b codex/v0868-stopship-<issue>
```

### Fleet 支持的 stopship lane（dogfood #4178）

命名 fleet 文件：[`fleets/v0868-stopship.toml`](../fleets/v0868-stopship.toml)（角色：`scout`、`implementer`、`reviewer`、`verifier`、`release_lead`）。Workflow：`workflows/v0868_stopship_lane.workflow.js`（步骤绑定 fleet `role` — 而非原始 provider/model 身份）。

**目标形态**（Phase 1 Lane CLI #4176 + Phase 2 角色解析 #4177）：

```bash
# 创建绑定到 stopship + fleet 的持久化 tmux 支持的 lane 并启动 Workflow
codewhale workflow run stopship \
  --issue 4090 \
  --fleet v0868-stopship \
  --runtime tmux \
  --goal "修复 #4090、#4093、#4094。从 main 分支实现。"

codewhale lane list
codewhale lane attach <lane-id>          # 或：codewhale lane attach <lane-id> --print
codewhale lane logs <lane-id>
codewhale lane stop <lane-id>
```

`workflow run` 验证已检入的 Workflow 源码和命名的 Fleet 花名册，创建 Lane 记录，并通过选定的 Runtime 后端启动现有的无头 Workflow 工具。Workflow 驱动程序在派生子 agent 之前通过命名的 fleet 解析每个 `task({ role })`。

在不启动 agent 的情况下验证 fleet 角色解析：

```bash
# 纯单元路径（CI 安全）
cargo test -p codewhale-workflow --lib named_fleet
```

### 直接工具路径

从 CodeWhale TUI 或无头 exec：

```bash
# 无头 stopship lane（当前推荐用于 CI/VM agent）
codewhale exec --auto --output-format stream-json \
  "运行 workflows/v0868_stopship_lane.workflow.js，分支 codex/v0868-stopship。修复 #4090、#4093、#4094。从 main 分支。使用 fleets/v0868-stopship.toml 中的 fleet profiles scout/builder/reviewer/verifier。"

# 每个 issue 的无头执行（单个 stopship issue）
codewhale exec --auto --output-format stream-json \
  "为 issue #4090 运行 workflows/v0868_issue_implement.workflow.js。从 main 分支。"

# TUI 显式路径
/workflow start workflows/v0868_stopship_lane.workflow.js
```

Workflow 首先使用只读 scout，然后按顺序使用实现 agent。在默认模式下，写 agent 需要批准；对无头 VM 运行使用 `--auto`。
在人工验证 `main` 之前，**不要**关闭 #4090/#4093/#4094。

## 每个 issue 的实现（单个 issue）

对于一个 `agent-ready` 的 issue：

1. `gh issue view <N> -R Hmbown/CodeWhale`
2. 确认 issue 在里程碑 `v0.8.68` 中且具有标签 `v0.8.68`
3. 运行 `workflows/v0868_issue_implement.workflow.js`，在目标中包含 issue 编号
4. 或使用无头方式：`codewhale exec --auto`，以 issue 正文作为提示
5. 打开引用 `Fixes #<N>` 的 PR；合并之前不要关闭 issue

Agent 执行的标签规范：

```bash
gh issue edit <N> --add-label agent-in-progress --remove-label agent-ready
# PR 合并后：
gh issue close <N> --comment "已在 PR #<PR> 中修复"
```

## PR 收录 lane（与 waves 并行）

审查社区 PR 而不压缩作者身份。来自 #4092 的顺序：

| PR | Issue | 备注 |
|----|-------|-------|
| #4088 | #4026 | 可合并；终端选择高亮 |
| #4087 | #4082 | 草稿重构；完成审查 |
| #4084 | #4065 | Fleet 别名清理 |
| #3761 | #3757 | 冲突；如有需要则 cherry-pick |
| #3969 | #3965 | 冲突；首先与 #4065 对齐 |

## 需要加载的技能

从 `docs/skills/` 复制或引用这些维护者技能：

- `gh-compile-issues` — 用证据分类为已完成/快速修复/设计/推迟
- `codew-release-qa-sweep` — 发布门禁命令
- `gh-find-prs` — 在实现之前定位相关 PR

## Agent 约束

- **不要**在未经明确批准的情况下推送到 `main`、打标签、发布或关闭 issue
- **不要**强制推送或修改已推送的提交
- **务必**为每个"完成"声明引用 `path:line` 证据
- **务必**在每个 wave 之后运行验证门禁
- **务必**在切换 agent 时更新 issue #4092 的交接记录

## 里程碑状态（2026-07-07）

- **真实来源：** `main`（PR #4099 已合并 — 快速切换已落地）
- 里程碑 `v0.8.68`（#53）：约 70 个开放 / 约 105 个总计
- 标签：`v0.8.68` 与里程碑成员同步
- 发布阻塞： #4093、#4094
- 最高 dogfood 回归： #4090（Ctrl+C 重新提示）
- **推迟：** v0.8.69 重构和 Waves 2–4，直到 stopship 绿灯
- **仅过时参考：** `codex/0868-next` — 在需要时按提交 cherry-pick
