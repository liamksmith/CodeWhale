---
name: gh-find-prs
description: "调查开放的 CodeWhale PRs，并根据代码、测试和检查对每个 PR 的可合并性和处置进行分类，针对真实落地分支测试。"
---

# gh-find-prs

调查开放 PR 队列并为每个 PR 分配处置——以代码、测试和检查为依据，绝不以标题为依据——并针对**真实**发布分支（通常仅在本地，例如 `<release-branch>`）测试实际可合并性，而非基于 main 的 GitHub 标记。

## 何时使用

- 维护者问"PR 队列里有什么？"、"我们能落地什么？"或"分类开放 PRs"。
- 发布剪辑之前，扫描社区贡献到活跃发布分支。
- 任何时候需要按 PR 做 DIRECT-MERGE / HARVEST / DEFER / CLOSE-WITH-NOTE 调用并附带署名。

这是**读取并推荐**。你**不**合并、关闭、打标签或发布。你展示证据和建议的处置；维护者批准。

## 工作流程

1. **盘点队列。** 一次调用，结构化：
   ```
   gh pr list --repo Hmbown/CodeWhale --state open \
     --json number,title,author,headRefName,baseRefName,isDraft,mergeStateStatus,statusCheckRollup
   ```
   记下 `mergeStateStatus`（CLEAN / BLOCKED / DIRTY / UNKNOWN），但仅视为提示——它是基于 `main` 计算的，而真实落地目标通常是不同分支。

2. **识别真实落地分支。** 发布头部经常仅在本地：
   ```
   git branch --list 'codex/v0.8*' 'codex/v0.9*'
   git log --oneline -1 <release-branch>
   ```
   使用该引用（而非 `main`）进行下方每个可合并性测试。

3. **从代码而非标题阅读每个候选。** 对每个非平凡 PR：
   ```
   gh pr view <N> --repo Hmbown/CodeWhale \
     --json files,additions,deletions,statusCheckRollup,body,comments
   gh pr diff <N> --repo Hmbown/CodeWhale
   ```
   阅读 diff。"fix(exec): ..."可能是无操作或回归；"chore"可能是真正的修复。判断变更、添加的测试和任何审查评论。

4. **解码检查失败——区分琐碎与真实。** 在 `statusCheckRollup` 中，找到每个 `conclusion: FAILURE` 并读取其 job。CodeWhale 的 CI jobs 是 `Lint`、`Test`、`Version drift`、`gate`、`npm wrapper smoke`、`Mobile runtime smoke`、`Documentation`、`GitGuardian Security Checks`。
   - 仅是 `cargo fmt` 漂移的 `Lint` 失败是琐碎的——可收割，落地时用 `cargo fmt --all` 修复。
   - `Test (...)` 或 Lint 下的 `clippy` 失败是真实的——信任之前先读日志。
   - 社区 PR 的 `Version drift` 失败是预期的（他们提升了版本，或没有）；不是收割的阻塞因素。

5. **针对真实发布头部测试合并。** `mergeStateStatus` 标记对本地分支撒谎。探测实际合并：
   ```
   git merge-tree --write-tree --messages <release-branch> origin/pr/<N>
   git merge-tree --write-tree --messages <release-branch> <pr-head-sha>
   ```
   退出 0 且无 `CONFLICT` 行 → 对发布分支干净（DIRECT-MERGE 候选，即使 GitHub 显示 BLOCKED/DIRTY）。有冲突 → HARVEST 或 DEFER。这是只读操作；仅在对象存储中写入对象，不影响任何分支或工作树。

6. **分配处置及所需署名。** 每个 PR 推荐恰好一个：
   - **DIRECT-MERGE** — diff 合理，检查绿色或可琐碎修复，`merge-tree` 对发布头部干净。通过 cherry-pick 落地以自动保留原始作者。
   - **HARVEST** — 变更好但有冲突、需要 fmt/rebase 或与发布工作纠缠。在发布分支上重新实现并以尾部署名（此处 cherry-pick 不保留作者身份）：
     ```
     Co-authored-by: Name <email>
     Harvested-from: PR #<N> by @handle
     ```
     `Harvested-from:` 尾部让 auto-close-at-main 工作流在变更到达 main 时带署名关闭 PR。
   - **DEFER** — 合理但被开放问题、缺失测试或发布冻结阻塞。留下积极具体评论；不关闭。
   - **CLOSE-WITH-NOTE** — 已替代、重复或超出范围。向维护者建议关闭并附带署名感谢；永远不要自己关闭。

7. **报告，不行动。** 输出紧凑表格：PR | 作者 | 落地分支裁决 | 检查摘要 | 处置 | 署名行。在此停止等待维护者批准。

## 红线 / 禁忌

- **不要从标题判断。** "fix(...)" / "feat(...)" / emoji 前缀测试 PR 什么都证明不了。每次都要打开 diff。
- **不要信任 `mergeStateStatus` 用于真实目标。** CLEAN/BLOCKED/DIRTY 是针对 `main`；始终用 `git merge-tree <release> <pr-head>` 确认。
- **不要混淆琐碎和真实的检查失败。** 仅 fmt 的 `Lint` 红色可收割；`Test (...)` 失败不可——读日志。
- **不要丢失署名。** 每次收割携带 `Co-authored-by:` + `Harvested-from:`；每次 cherry-pick 保留原始作者。不静默重新实现。
- **不要合并、关闭、重新定位、打标签、发布或发布。** 推荐；维护者决定。
- **不要发布负面或挑剔的评论。** GitHub 面向的评论积极且署名；将批评保留在给维护者的内部报告中。
- **不要修改工作树或任何分支。** `git merge-tree --write-tree` 是唯一允许的"写入"——仅触及对象存储。
