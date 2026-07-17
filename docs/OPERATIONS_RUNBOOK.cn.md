# codewhale 运维手册

本手册涵盖本地 CLI/TUI 运行时的实用调试和事件响应。

## 快速排查

1. 确认二进制文件 + 配置：
   - `cargo run -- --version`
   - `cat ~/.codewhale/config.toml`（或检查已配置的 profile）
2. 启用详细日志：
   - `RUST_LOG=deepseek_cli=debug cargo run`
   - 对于 HTTP 重试/重连：`RUST_LOG=deepseek_cli::client=debug cargo run`
3. 捕获当前状态：
   - `ls ~/.codewhale/sessions`
   - `ls ~/.codewhale/sessions/checkpoints`
   - `ls ~/.codewhale/tasks`

## 事件：轮次卡住或流式输出停止

症状：
- TUI 停留在加载状态
- 部分助手输出但未完成

检查：
1. 查看重试/健康日志（`deepseek_cli::client`）
2. 验证端点连通性：
   - `curl -sS https://api.deepseek.com/beta/models -H "Authorization: Bearer $DEEPSEEK_API_KEY"`
3. 确认工具输出中没有本地沙箱/权限死锁

操作：
1. 如果前台 shell 命令正在运行，按 `Ctrl+B` 将其移到后台（轮次继续运行，命令变为 `/jobs` 下的后台任务）；如果要取消轮次，请改用 `Ctrl+C`。
2. 如果命令是在后台启动的，请让助手使用 `exec_shell_cancel` 和返回的任务 ID 取消它。
3. 当你想停止请求本身时，使用 `Esc` 或 `Ctrl+C` 中断当前轮次。
4. 重试提示词；如果仍然失败，重启 TUI。
5. 重启后，验证之前排队/进行中的运行时轮次显示为已中断，而不是保持运行状态。

## 事件：网络中断 / 离线行为

预期行为：
- 离线模式激活时，新提示词会排队
- 队列状态持久化到 `~/.codewhale/sessions/checkpoints/offline_queue.json`

检查：
1. 在 TUI 中打开队列：`/queue list`
2. 确认持久化的队列文件存在且时间戳更新

操作：
1. 恢复网络连接
2. 重新发送排队的条目（通过 `/queue edit <n>` + Enter，或正常输入流程）
3. 确保队列为空时清除队列文件

## 事件：需要崩溃恢复

预期行为：
- 检查点存储在 `~/.codewhale/sessions/checkpoints/latest.json`
- 启动时默认开启新会话，除非提供了 `--resume`/`--continue`

操作：
1. 通过 `codewhale --resume <id>` 或在 TUI 中按 `Ctrl+R` 显式恢复之前的工作
2. 如果需要检查检查点，检查 `latest.json` 中的 schema 不匹配/详细信息
3. 如果 schema 版本比二进制文件支持的更新，升级二进制文件或删除过期的检查点

## 事件：持久化状态 Schema 错误

症状：
- 类似 `schema vX is newer than supported vY` 的错误

受影响的存储：
- 会话（`~/.codewhale/sessions/*.json`）
- 运行时 thread/turn/item 记录
- 任务（`~/.codewhale/tasks/tasks/*.json`）

操作：
1. 确认二进制版本和迁移预期
2. 在编辑前备份状态目录
3. 执行以下之一：
   - 使用更新的兼容二进制文件运行，或
   - 归档不兼容的记录并重新生成状态

## 事件：MCP/工具执行失败

检查：
1. 验证 `~/.codewhale/mcp.json` schema 和服务器命令路径
2. 确认服务器进程可以手动启动
3. 检查 TUI 历史/日志中的沙箱拒绝

操作：
1. 使用所需审批重试（或仅在适当时使用 YOLO）
2. 临时禁用失败的 MCP 服务器并隔离问题
3. 通过 `/mcp` 诊断验证后重新启用

## 事后检查清单

1. 保留日志和相关状态文件
2. 记录触发条件、影响和缓解措施
3. 添加或更新回归测试（重试/恢复/schema）
4. 如果行为发生变化，更新本运维手册和架构文档
