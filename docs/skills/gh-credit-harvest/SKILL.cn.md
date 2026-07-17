---
name: gh-credit-harvest
description: "将一个社区 PR 收割到发布分支，保留作者身份和署名，验证通过，并附温暖的感谢。"
---

# gh-credit-harvest

将**恰好一个**社区 PR 收割到真实落地分支，保留完整的作者身份和机器可读署名，验证通过后感谢贡献者。PR 是证据：从代码、测试、评论和检查来判断它，绝不从标题判断。未经 Hunter 批准，不要合并、关闭、打标签或发布——此技能落地带署名的提交并发布感谢；工作流会关闭 PR。

## 何时使用

- 你已获批准将一个**特定**社区 PR 落地到发布分支。
- PR 尚未在落地分支上（如果已存在，请用 `gh-close-issues` 带署名关闭）。
- 落地分支可能仅在本地（例如 `<release-branch>`）；基于 main 的"可合并"标记不证明它能干净落地。

## 工作流程

1. 找到真实的落地分支（Hunter 命名的分支，不总是 `main`）并获取 PR 头部：
   ```bash
   git switch <release-branch>
   git fetch origin pull/<N>/head
   git log -1 --format='%H %an <%ae>' FETCH_HEAD   # 要保留的作者
   ```
2. 从证据而非标题审查。阅读 diff、测试、关联 issue、评论和 CI：
   ```bash
   gh pr view <N> --repo Hmbown/CodeWhale --json title,author,files,statusCheckRollup
   gh pr diff <N> --repo Hmbown/CodeWhale
   ```
3. 测试针对**真实**落地分支的可合并性（仅本地分支的 main 标记会撒谎）：
   ```bash
   git merge-tree $(git merge-base HEAD FETCH_HEAD) HEAD FETCH_HEAD   # 空/干净 = 无冲突
   ```
4. 落地它，优先 cherry-pick——它自动保留原始作者：
   ```bash
   git cherry-pick <sha>            # 来自 FETCH_HEAD 的一个或多个提交
   ```
5. 如果有冲突、有噪音或需要 squash，重新应用窄范围片段并以显式作者 + 署名尾部提交。从 `.github/AUTHOR_MAP` 解析 `--author` 和共同作者（回退到数字 noreply）：
   ```bash
   gh api users/<handle> --jq '"\(.id)+\(.login)@users.noreply.github.com"'
   git commit --author="Name <ID+handle@users.noreply.github.com>" -m "fix(scope): what changed (#<N>)" \
     -m "Harvested from PR #<N> by @<handle>" \
     -m "Co-authored-by: Name <ID+handle@users.noreply.github.com>"
   ```
   正文中的 `Harvested from PR #<N> by @<handle>` 行是 `.github/workflows/auto-close-harvested.yml` 匹配以在提交到达 `main` 时自动带署名关闭的内容。
6. 格式化并运行受影响 crate 的目标测试——只落地绿色的：
   ```bash
   cargo fmt --all
   cargo test -p <crate>            # PR 涉及的一个或多个 crate，而非整个工作区
   python3 scripts/check-coauthor-trailers.py --author-map .github/AUTHOR_MAP --range HEAD~1..HEAD --check-authors
   ```
7. 在 PR 上发布简短、温暖、具体的感谢——命名修复了什么，不戏剧化。保持 PR 开放；提交到达 `main` 时工作流带署名关闭它：
   ```bash
   gh pr comment <N> --repo Hmbown/CodeWhale \
     --body "感谢 @<handle> —— 对 <具体 bug> 的干净修复。已收割到 v0.8.61 通道，保留你的作者身份；到达 main 后将自动带署名关闭。"
   ```

具体示例：PR #3221 by @hongchen1993（在 exec 中尊重 `DEEPSEEK_BASE_URL`/`DEEPSEEK_MODEL`）可干净 cherry-pick，因此其作者无需手动尾部信息即可保留；在涉及 crate 上运行 `cargo test -p` 足以绿色落地。

## 红线 / 禁忌

- 不要仅从标题或标签判断或落地——阅读代码、测试、评论和检查。
- 不要信任仅本地发布分支的 GitHub main 可合并标记；用 `git merge-tree` 证明。
- 不要 squash 掉原始作者。能 cherry-pick 时优先；只在必要时回退到 `--author` + 尾部信息。
- 不要编造共同作者邮箱。使用 `.github/AUTHOR_MAP`，然后数字 noreply；绝不使用原始第三方、`.local`、占位符或 bot 邮箱。
- 不要省略 `Harvested from PR #<N> by @<handle>` 正文行——没有它 PR 不会带署名自动关闭。
- 不要落地红色，不要每个提交收割超过一个 PR，不要将无关变更批量放入收割。
- 未经 Hunter 批准，不要合并、关闭、打标签、发布或推送发布工件。保持评论积极且署名。
- 已在落地分支上？不要重新收割——通过 `gh-close-issues` 带署名关闭。
