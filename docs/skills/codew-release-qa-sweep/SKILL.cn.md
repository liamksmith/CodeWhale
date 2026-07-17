---
name: codew-release-qa-sweep
description: "在声称 CodeWhale 发布工作完成之前使用：运行全部门控扫描并列出人工 QA 目标。"
---

# CodeWhale 发布 QA 扫描

在声称任何 CodeWhale 发布工作"已完成"之前运行本扫描。绿色自动门控扫描加上三个手动 QA 目标是证据标准。无扫描，无"完成"——准确报告每个步骤的运行内容和结果。

## 何时使用

- 在告诉 Hunter（或 PR 线程）发布工作已完成或可合并之前。
- 将 PR 收割/落地到发布分支之后、发布边界之前。
- 在**真实**落地分支（例如 `<release-branch>`，通常是仅本地的）上验证候选发布版本时。

## 自动门控扫描

从仓库根目录按顺序运行。遇到第一个失败就停止并报告。

```bash
# 0. 确认你在真实的发布头部，而非基于 main 的假设。
git branch --show-current          # 期望例如 <release-branch>
git status --short                 # 工作树应为干净状态

# 1. 格式化 + 多余的空白/冲突标记
cargo fmt --all --check
git diff --check

# 2. 库/协议/cli/流程/状态测试，锁定
cargo test -p codewhale-config -p codewhale-protocol -p codewhale-cli \
  -p codewhale-workflow -p codewhale-state --locked

# 3. TUI 测试二进制，锁定
cargo test -p codewhale-tui --bins --locked

# 4. 真 PTY 发布运行时 QA（密封 HOME + 环回供应商）
cargo test -p codewhale-tui --test release_runtime_qa --locked -- --test-threads=1

# 5. TUI debug 构建，锁定
cargo build -p codewhale-tui --locked

# 6. 交付二进制的发布构建，锁定
cargo build --release --locked -p codewhale-cli -p codewhale-tui

# 7. 版本漂移门控（工作区 ↔ npm ↔ Cargo.lock ↔ changelog ↔ README）
./scripts/release/check-versions.sh

# 8. 二进制冒烟
./target/release/codewhale --version
```

如果要验证 PR 是否可以落地，还要针对**实际**发布头部测试可合并性，而非 main 的绿色标记：

```bash
git merge-tree $(git merge-base <release-branch> <pr-head>) <release-branch> <pr-head>
```

对 `main` 干净的 PR 仍可能与发布分支冲突。

## 人工 QA 目标

单元/构建门控不覆盖真 TUI。全部三项都要演练并记录你的观察：

可重复的本地基准是 `release_runtime_qa`：它在伪终端中使用密封 home 和环回模拟供应商启动真实 TUI 进程，然后断言以下每个场景。即使在单独的手动视觉检查时也要运行它；测试不会留下供应商流量或凭据。

1. **六 worker 扇出活性（#3216/#2211）。** 启动 6 个子代理。确认输入、渲染、取消和侧边栏全程保持响应，且 **Esc 在扇出中途取消**（提示词中断，不是卡死的 ~24s 突发或冻结）。对于 #3289 的 Windows Terminal 复测路径：在 plan 模式下启动，向计划添加后续输入，按 Esc，切换到 yolo/accept 流程，触发至少两次 auto/Fleet worker 启动，并持续输入/取消/模式切换检查数分钟。如果冻结复现，附上日志。
2. **多终端路由隔离（#3227）。** 在不同的供应商/模型路由上打开多个终端。确认零跨终端污染和无供应商+模型不匹配——每个终端遵守其自身路由。
3. **排队转向 + Ctrl+S（#3203）。** 在繁忙回合中排队一条转向消息；确认 Ctrl+S 发送排队/草稿消息且排队转向状态清晰可读。

## 报告格式

报告一份清单：每个命令、通过/失败以及显著的输出行（测试计数、`--version` 字符串、`check-versions.sh` 结论）。对于人工 QA，说明每个目标实际观察到的内容，引用 issue 编号。如果某个步骤被跳过或无法运行（例如没有显示器进行 TUI QA），明确说明——不要暗示你没有的覆盖率。

## 红线 / 禁忌

- 不要在缺少上述证据时声称"完成"、"通过"或"可合并"。没有命令输出的断言不可接受。
- 不要信任 main 的可合并性标记用于发布分支；使用 `git merge-tree` 针对真实头部。
- 不要因为构建变绿就跳过人工 TUI 目标——冻结、路由不匹配和转向回归存在于运行时，而非门控。
- 未经 Hunter 明确批准，不要打标签、发布、创建 GitHub Release、推送工件或合并/关闭任何 PR 或 issue。绿色扫描是就绪证据，而非许可。
- 永远不要仅从 PR 标题或标签收割/关闭——从代码、测试、评论和检查中审查。
- 当扫描清除了收割的 PR 时，保留贡献者署名：cherry-pick 保留原始作者，否则添加 `Co-authored-by: Name <email>` 和 `Harvested-from: PR #N by @handle`，以便到达 main 时的自动关闭工作流署名贡献者。
- 保持任何面向贡献者的评论积极且署名；门控保持干运行/建议状态，除非 Hunter 批准强制执行。
