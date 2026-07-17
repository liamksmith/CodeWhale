# CodeWhale v0.8.67 Cursor 内部测试记录

日期：2026年7月7日
测试者：Codex，通过 Cursor 集成终端中的计算机使用功能
工作区：`/Volumes/VIXinSSD/CW/codewhale`
证据日志：`target/dogfood/cursor-v0867/cursor-dogfood-20260707-041000.log`

## 摘要

本次 Cursor 内部测试通过 Cursor 自己的集成终端对已安装的 `0.8.67` 二进制文件进行了测试，发现本地运行时状态良好。该版本现已在所检查的发布发布面上可见：GitHub Release、npm `codewhale@0.8.67` 以及发布脚本检查的 crates。

本次测试未声称覆盖完整的可视化 TUI。它使用了内部测试文档的无头等效方案来处理许多发布 issue，并在下方记录了剩余的需手动视觉检查的项目。

## 正常工作的项目

- Cursor 集成终端可看到最终发布 SHA：
  - `737ac9872808deb96d6dc1dea0c2d79aa84e5f6a`
  - `737ac9872 (HEAD -> main, tag: v0.8.67, origin/main, origin/HEAD)`
- PATH 可见的二进制文件报告了最终构建版本：
  - `codewhale 0.8.67 (737ac9872808)`
  - `codew 0.8.67 (737ac9872808)`
  - `codewhale-tui 0.8.67 (737ac9872808)`
- GitHub release 真值：
  - `v0.8.67` 存在，非草稿，非预发布，发布时间为 `2026-07-07T08:28:12Z`。
  - `v0.8.67` 里程碑中没有未关闭的 GitHub issue。
- npm 发布后，发布版本验证通过：
  - `./scripts/release/check-published.sh 0.8.67`
  - `npm codewhale@0.8.67 is published`。
  - `npm codewhaleBinaryVersion=0.8.67`。
  - 17 个已检查的 crates.io 包可见。
- 本地关卡通过：
  - `./scripts/release/check-versions.sh`
  - `git diff --check`
  - `cargo fmt --all --check`
  - `cargo build -p codewhale-tui --locked`
- Doctor/setup 界面通过：
  - `codewhale-tui doctor --json` 输出有效 JSON，包含 `.setup`。
  - 隔离的 `CODEWHALE_HOME` doctor 输出有效 JSON，包含 `.setup`。
  - `codewhale doctor | head -n 1` 无 stderr 输出并通过静默 SIGPIPE 处理退出。
- 功能列表正常：
  - `shell_tool`、`subagents`、`web_search`、`apply_patch`、`mcp` 和 `exec_policy` 为 stable/enabled。
  - `vision_model` 为 beta/disabled。
- 设置通道 QA 通过：
  - `CODEWHALE_BIN=target/release/codewhale-tui ./scripts/v0867-setup-qa.sh`
  - 结果：`33 passed, 0 failed`。
- 核心内部测试区域的回归测试通过：
  - `subagent`：298 通过。
  - 针对性的 subagent/delegate/worktree/budget/goal/config/setup/localization/pricing/model catalog/status/fleet/workflow 过滤器全部成功退出（匹配到测试的）。
  - `cargo test -p codewhale-workflow -p codewhale-workflow-js --locked` 通过了所有报告的单元、VM 和 doctest 套件。
- 无头运行时冒烟测试通过：
  - `codewhale app-server --stdio` health/capabilities/prompt 界面正常。
  - `auth list` 显示了 `deepseek`、`openrouter`、`xiaomi-mimo` 和 `zai` 的已配置路由。
  - 使用 `deepseek-v4-flash` 的 DeepSeek 实时 exec 冒烟测试通过并返回了哨兵值。

## 不工作/阻塞项

- 在 npm 发布后，所检查的发布面未发现当前发布阻塞项。
- 其余阻塞项（如有）应来自手动 TUI 内部测试或下游安装冒烟测试，而非注册表可见性。

## 仍需手动检查的缺口

- 旧的可见 Cursor 聊天摘要仍引用 `dc320ebf8`；新的终端证据将其纠正为 `737ac9872808`，但过时的转录在视觉上令人困惑。
- 内部测试回归列表包含两个匹配到零测试的过滤器：
  - `child_hit_max_steps`
  - `missing_message`
  这些并未导致命令失败，但应在内部测试提示词中重命名或替换为当前测试名称。
- 仅对 DeepSeek 进行了实时冒烟测试，以控制模型支出。配置的 `zai`/GLM、`xiaomi-mimo` 和 `openrouter` 已发现但在本轮测试中未进行实时调用。
- 当前 home 的 `doctor --json` 报告 `first_run_ready=true` 和 `update_ready=true`，但 `operate_ready=false`，尽管提供商认证和 fleet 就绪状态看起来已配置。这可能是因为实时验证为 false 而预期如此，但面向用户的就绪状态含义应予以澄清。
- 视觉 TUI 检查仍需真人/代理通过：
  - `/setup` 欢迎文案和选择/起草/批准流程。
  - `/setup` Constitution 步骤选项：引导预览、保留现有、模型草案、捆绑。
  - `/constitution` 管理器层渲染。
  - 审批提示的语气和破坏性样式。
  - `/fleet setup` 模型草案和 TOML 预览/批准流程。
  - 运行中的 worker/fleet 轮次期间的旋转动画和实时侧边栏详情。

## 发布就绪性判断

本地运行时就绪性和发布可见性看起来良好。我建议保留到后续 issue 而非必须随 `0.8.67` 制品发布的产品质量项目是：过时的内部测试过滤器和模糊的 `operate_ready=false` doctor 信号。
