# Claude 插件兼容性

当 Claude Code 技能文件夹是纯 `SKILL.md` 目录时，CodeWhale 将它们视为指令包。它不运行 Claude Code 插件运行时。

## 支持的

- 由正常技能注册表发现的工作区或全局 `.claude/skills/<name>/SKILL.md` 目录。
- 包含一个选定技能目录的 GitHub 或 tarball 安装，如 `skills/<name>/SKILL.md`、`.agents/skills/<name>/SKILL.md`、`.claude/skills/<name>/SKILL.md`，或以 `skills/<name>/SKILL.md` 结尾的嵌套包布局。
- 选定技能目录内的配套文件，如 `references/`、`examples/` 或脚本，这些仅在技能被显式加载并信任后使用。

## 不作为插件运行时支持

Claude Code 插件功能在 v0.8.60 兼容性边界之外：

- `.claude-plugin/plugin.json` 元数据和激活语义。
- 自定义斜杠命令包。
- 插件构建步骤、编译的 TypeScript agent、仪表板服务器、共享插件状态或令牌门控的服务进程。
- 需要 Claude 特定运行时行为的 frontmatter 字段，如 `model: inherit`。

如果 Claude Code 插件仓库包含多个技能，一次安装或迁移一个 `skills/<name>` 目录。`/skill install` 会拒绝多技能插件归档并显示明确消息，因此它永远不会静默选择一个技能并丢弃插件运行时行为。

对于更丰富的集成，将插件的可执行接口包装为 MCP、hooks 或显式命名外部命令的 CodeWhale 技能。
