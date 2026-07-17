---
name: gh-assign-issues
description: "用于批量将 GitHub issues 分配到里程碑和/或负责人，并逐一验证。"
---

# gh-assign-issues

将一组 CodeWhale issues 批量重新定位或分配到里程碑和/或负责人，逐一验证。里程碑（或负责人）变更是信号；不要用评论来叙述。使用 `gh` 进行所有 GitHub 调用。

## 何时使用

- 你有一份具体的 issue 编号列表（例如来自分类），需要移入 `v0.8.61` 等发布里程碑，或分配给负责人。
- 里程碑被重命名/创建后，其 issues 需要重新指向。
- 你已分组完收件箱，希望队列反映此状态而不公开发布 noise。

此技能仅更改里程碑/负责人。不关闭、标记、评论、合并或发布。这些由维护者处理。

## 工作流程

1. 确认准确的里程碑标题。`gh issue edit --milestone` 按名称匹配，因此拼写错误会静默失败（或更糟，无操作）。读取真实标题并记录开放数量的起始值：

   ```bash
   gh api repos/Hmbown/CodeWhale/milestones \
     --jq '.[] | "\(.number)\t\(.title)\topen=\(.open_issues)\tstate=\(.state)"'
   ```

   逐字复制标题字符串（例如 `v0.8.61`）。如果目标里程碑缺失或已关闭，停止并询问维护者；不要创建。

2. 预检查每个编号。`gh issue edit` 会很乐意将 PR 或已关闭 issue 重新定位，所以先过滤。`url` 揭示了 PR-vs-issue（`/pull/` vs `/issues/`）；跳过非 OPEN 或 PR 的项目：

   ```bash
   for N in 3101 3102 3103; do
     gh issue view "$N" --repo Hmbown/CodeWhale \
       --json number,state,url,milestone \
       --jq '"\(.number)\t\(.state)\t\(.url)\tmilestone=\(.milestone.title // "none")"'
   done
   ```

   标记任何 `url` 包含 `/pull/`（是 PR）或状态非 `OPEN` 的行，将其从下方循环中排除。

3. 逐个应用变更，报告每个 issue 的成功情况。使用 `--milestone`、`--add-assignee` 或两者：

   ```bash
   for N in 3101 3102 3103; do
     if gh issue edit "$N" --repo Hmbown/CodeWhale \
          --milestone "v0.8.61" >/dev/null 2>&1; then
       echo "ok   #$N -> v0.8.61"
     else
       echo "FAIL #$N (PR? closed? bad milestone title?)"
     fi
   done
   # owners: 添加 --add-assignee handle（不要编造登录名）
   ```

   编辑是幂等的：重新指向已正确的 issue 无影响。

4. 验证里程碑已移动。重新运行步骤 1 的命令，确认开放数量增加了你移入的 issue 数（减去跳过的）。用步骤 2 的视图抽查几个，确认 `milestone.title` 现在是目标。

5. 报告一份紧凑的台账：已移动的 issues、已跳过的 issues 及原因（PR / 已关闭 / 标题不匹配）、前后开放数量以及任何负责人分配。不发布公开评论。

## 红线 / 禁忌

- 不要仅从标题列表编辑。解析为 OPEN issue 编号并在编辑前确认每个是 issue 而非 PR（url 中的 `/pull/`）。
- 不要猜测里程碑标题或负责人登录名。近似的名称静默无操作或静默失败；无效的登录名在循环中报错。
- 不要发布"移到 v0.8.61"的评论。里程碑变更是信号；额外 noise 对善意贡献者是干扰。
- 不要关闭、合并、标记、打标签或发布作为此技能的一部分。这些需要明确的维护者批准（参见 AGENTS.md）。
- 不要跳过步骤 4。未变的开放数量意味着标题错误或每次编辑静默失败。
- 保留贡献者署名：此技能永不更改作者身份、收割尾部信息（`Co-authored-by` / `Harvested-from`）或关闭引用。
