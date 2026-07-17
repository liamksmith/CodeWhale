# 发布检查清单

v0.8.21/v0.8.22 CHANGELOG 缺口证明了我们需要这份预标记检查清单。
从最终发布源上的干净工作树按顺序逐步进行。
将任何未勾选的复选框视为发布阻塞项。

关于底层工具（预检脚本、npm 冒烟测试、
发布 crates）的更多上下文，请参见 [`RELEASE_RUNBOOK.md`](RELEASE_RUNBOOK.md)。
对于较大的里程碑版本，在标记之前将任何版本特定的验收矩阵添加到
发布分支；用于此通用检查清单未枚举的提供商路由、功能门控、
GUI/运行时冒烟测试、远程工作台决策和贡献者署名。

## 0. 发布源已冻结

- [ ] 实时里程碑和 PR 队列不再包含针对此版本的工作：
      ```
      gh issue list --repo Hmbown/CodeWhale --milestone "vX.Y.Z" --state open
      gh pr list --repo Hmbown/CodeWhale --state open --limit 100
      ```
- [ ] 任何剩余的同主题工作已显式重新定位到后续版本或标记为已知问题。在仍计划合并更多同版本修复时不要提升版本号/标记。
- [ ] 发布标签尚未指向旧的源 SHA，或者维护者已刻意选择发布该确切旧 SHA：
      ```
      git ls-remote origin refs/heads/main refs/tags/vX.Y.Z
      gh release view vX.Y.Z --repo Hmbown/CodeWhale
      ./scripts/release/check-published.sh X.Y.Z
      ```
- [ ] 如果 `vX.Y.Z` 存在但没有 GitHub Release/包而 `main` 已经前移，则停止。选择以下之一：按原样发布现有标签、将后续工作提升到下一个补丁版本，或显式批准删除/重新创建未发布的标签。在 PR 清理期间不要静默移动标签。

## 1. CHANGELOG 条目存在于此版本

- [ ] `CHANGELOG.md` 顶部有 `## [X.Y.Z] - YYYY-MM-DD` 标题
- [ ] 条目致谢了每个外部贡献者、收集到的 PR 作者、
      关联问题报告者、复现/日志提供者、审查者和
      验证助手，其工作实质性地塑造了此版本。使用以下命令获取提交列表：
      ```
      git log vPREV..HEAD --no-merges --format="%h %an <%ae> %s" \
        | grep -v '<your-email@…>'
      ```
      对于每个贡献者，链接其显示名称和（已知时）
      `@github-handle`。然后检查关联的问题和收集到的 PR，以免报告者/助手因未创作提交而被遗漏。
- [ ] 条目使用 Keep a Changelog 标题——`Added`、`Changed`、
      `Fixed`、`Security`、`Removed`、`Deprecated`。仅当有用户必须规避的实质性内容时才添加 `Known issues`。
- [ ] 条目将所有引用的问题/PR 编号以 `#NNNN` 提及，以便 GitHub 的自动链接器能拾取它们。
- [ ] 运行 `scripts/sync-changelog.sh` 重新生成 `crates/tui/CHANGELOG.md`
      （嵌入在二进制文件中用于 `/change` 的最近发布片段）。
      不要手动编辑该文件，不要将完整的根 changelog 复制到其中——旧条目存放在 `docs/CHANGELOG_ARCHIVE.md` 中。

## 2. 版本号同步

- [ ] 运行 `./scripts/release/prepare-release.sh X.Y.Z`——它会提升
      工作区版本、每个 crate 的依赖版本号、
      `npm/codewhale/package.json`（`version` + `codewhaleBinaryVersion`）、
      README 安装标签示例、刷新 `Cargo.lock`、重新生成
      `crates/tui/CHANGELOG.md` 和 `web/lib/facts.generated.ts`，最后运行 `check-versions.sh`。在运行之前**先**写好 CHANGELOG 条目。
- [ ] `npm/deepseek-tui/package.json` 保持私有/仅兼容性，**不**提升或发布。
- [ ] `./scripts/release/check-versions.sh` 报告
      `Version state OK: workspace=X.Y.Z, npm=X.Y.Z, lockfile in sync.`
- [ ] `./scripts/release/check-ohos-deps.sh` 报告 OpenHarmony
      目标图未拉取不支持的 `nix` 0.28/0.29、
      `portable-pty`、`starlark`、`arboard` 或 `keyring` crate。

## 3. 预检门控

按顺序在仓库根目录运行：

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo check --workspace --all-targets --locked`
- [ ] `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- [ ] `cargo test --workspace --all-features --locked`
      （在声称是偶发错误之前，先用
      `cargo test -p PKG --bin BIN -- TEST_NAME` 单独重新运行任何单次失败。
      修改进程范围状态的测试——`HOME`、`cwd`、`RUST_LOG`——
      可能在并行时竞争。在 `Known issues` 中记录确认的偶发错误。）
- [ ] `./scripts/release/publish-crates.sh dry-run`

## 4. npm 包装器冒烟测试

- [ ] `cargo build --release --locked -p codewhale-cli -p codewhale-tui`
- [ ] `node scripts/release/npm-wrapper-smoke.js`
      （如果需要事后检查临时安装目录，设置 `DEEPSEEK_TUI_KEEP_SMOKE_DIR=1`。）

## 5. 分支和 PR

- [ ] 分支已推送：`git push -u origin work/vX.Y.Z-...`
- [ ] 使用 `gh pr create --base main --title "chore(release): prepare vX.Y.Z"` 打开 PR
- [ ] PR 目标为 `main` 并将在推送任何 `vX.Y.Z` 标签之前合并。不要标记仅发布分支；GitHub 在这些提交到达默认分支之前不会处理 `Closes #N` 关键字。
- [ ] PR 正文包括：
  - 一段发布主题摘要
  - 自上次发布以来的新提交清单
  - 显式标注任何 **Security** 条目，以便审查者看到
  - 贡献者致谢列表
  - CHANGELOG 中的 `Known issues` 块（如果有）
- [ ] PR 标题是**中性的**——不要在标题中放入 CVE 风格的语言或具体的攻击细节。这些留到推送标签后的 GitHub 发布说明。

## 5b. 分支卫生（合并后）

发布/集成合并落地后，明确发布尖端位置并**安全**清理陈旧分支。在临时/更新分支上的工作检出（即使 `HEAD` 已经匹配标签）会造成发布焦虑：贡献者无法判断他们的工作是否已合并。

- [ ] 首先运行模拟报告（只读，不删除任何内容）：

      ```sh
      ./scripts/release/branch-hygiene.sh --release-branch codex/vX.Y.Z
      ```

      它打印：当前检出的分支、本地 + 远程发布尖端和 main 引用；**可以安全删除**的分支（尖端已包含在配置的 main 引用或发布分支中）；以及**保留 / 需要审查**列表，列出每个分支名称、独特提交数、作者和保留原因。摘要行报告有多少个安全删除、多少个为贡献者工作保留、多少个需要人工决策。本地/远程发布尖端分歧会以非零退出。当规范发布引用位于 `upstream` 而非 `origin` 时使用 `--remote upstream`。
- [ ] 如果工作检出停在一个陈旧分支上，切换到发布分支并快进：

      ```sh
      git switch codex/vX.Y.Z
      git fetch origin && git merge --ff-only origin/codex/vX.Y.Z   # 如果落后
      ```
- [ ] 仅在审查模拟结果后，删除**安全**分支。先本地；添加 `--prune-remote` 以同时删除远程安全删除：

      ```sh
      ./scripts/release/branch-hygiene.sh --release-branch codex/vX.Y.Z --prune --yes
      ```

      脚本**绝不**自动删除来自 Hunter 以外的贡献者且有独特提交的分支，除非该工作已合并。这些会进入保留/审查列表，附带作者和原因；在删除分支之前审查、合并、署名收集或显式保留。有疑问时，保留分支并记录决策。

## 6. CI 绿通和审查

- [ ] 所有必需的 CI 作业均已通过。`versions` 作业应镜像预检的 `check-versions.sh`，是你的最后一道防线。
- [ ] PR 已经过审查。

## 7. 标记和发布（审查后）

- [ ] 发布 PR 已合并到 `main`，然后本地 `main` 已快进：
      `git switch main && git fetch origin main && git merge --ff-only origin/main`
- [ ] 发布源可从 `main` 到达：
      `./scripts/release/ensure-release-on-main.sh HEAD`
- [ ] 使用 **Create release tag** 工作流从最终 `main` SHA 创建 `vX.Y.Z`，或创建并推送签名的本地标签：
      `git tag -s vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z`
- [ ] `release.yml` 工作流已为此标签构建并上传工件到 GitHub release。
- [ ] 在发布 Cargo 或 npm 之前，公共 GitHub Release 资产已证明与标签提交匹配：
      ```
      ./scripts/release/verify-release-assets.sh X.Y.Z
      ```
      这会检查本地标签、远程标签、成功的 Release 工作流 SHA、面向 npm 的资产和 `codewhale-artifacts-sha256.txt` 清单。如果失败，在接触任何注册表之前重新运行或修复 GitHub Release 工作流。
- [ ] 实时 GitHub Release 正文有自己的 `## Contributors` 或 `## Credits` 部分；不要仅依赖"参见 CHANGELOG"。验证：
      ```
      gh release view vX.Y.Z --repo Hmbown/CodeWhale --json body \
        --jq '.body | test("## (Contributors|Credits)")'
      ```
- [ ] `npm view codewhale@X.Y.Z version codewhaleBinaryVersion --json`
      在 npm 注册表上报告新版本。
- [ ] `npm view deepseek-tui deprecated` 非空。旧版 npm 包已弃用，不得接收 `X.Y.Z` 发布。
- [ ] 分发渠道以规范渠道优先：网站安装页面（codewhale.net/install）首先显示 CodeWhale 原生命令（`npm install -g codewhale`、`curl .../install.sh | sh`）；Homebrew 标记为旧版兼容；shell 安装器使用 `docs/REBRAND.md#homebrew` 中记录的 codewhale 原生命名。
- [ ] `crates.io` 有新版本（或 `publish-crates.sh` 作业已推送）。
- [ ] `ghcr.io/hmbown/codewhale:vX.Y.Z` 和 `:latest` 已更新。
- [ ] 最终注册表验证通过：
      ```
      ./scripts/release/check-published.sh X.Y.Z
      ```

## 8. 标记后

- [ ] 编辑 GitHub release 说明以展开任何在 PR 标题/正文中有意省略的 CVE 风格或攻击细节。
- [ ] 在任何 release-workflow 重新运行后重新运行 GitHub Release 正文检查；工作流可能覆盖说明并意外移除贡献者署名。
- [ ] 在下一个版本的跟踪 issue 中记录任何推迟条目。
- [ ] 关闭此版本修复的任何 issue。

---

如果某一步骤失败，**修复其根本原因**而非跳过它。预提交钩子、签名和 CI 都在这里是为了捕获真正的问题。`--no-verify`、`--no-gpg-sign` 和越过审查者强制推送发布分支应通过约定保持硬禁用。
