# 维护者 / 代理技能

用于维护 CodeWhale 的 GitHub 管理和发布 QA 工作流，编码为 `SKILL.md` 技能（Claude Code 和 CodeWhale 都加载的相同格式）。它们编码了用于组装 v0.8.61 发布的工作流。

激活方式：
- **Claude Code：** 将技能目录复制到 `.claude/skills/`（项目）或您的用户技能目录。
- **CodeWhale：** 复制到 CodeWhale 的 `skills_dir`（例如 `~/.codewhale/skills/`），或打包到 `crates/tui/assets/skills/` 并在 `crates/tui/src/skills/system.rs` 中注册以发布。

技能：gh-file-issue、gh-compile-issues、gh-assign-issues、gh-plan-issues、gh-find-prs、gh-treasure-hunt、gh-close-issues、gh-credit-harvest、codew-release-qa-sweep。
