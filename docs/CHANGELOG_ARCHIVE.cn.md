# 更新日志归档

CodeWhale 的较早版本（v0.8.39 及更早）。近期版本见 [CHANGELOG.md](../CHANGELOG.md)。

## [0.8.39] - 2026-05-17

### 修复

- **飞书/Lark 桥接启动顺序受到保护。** 桥接现在在启动打开持久化线程状态之前保持 `ThreadStore` 已初始化，并添加了回归测试以防止将其移到首次使用之前。
- **`/model` 选择器再次即时打开精选列表。** 回退了 v0.8.38 的实时目录重做：选择器不再在打开时进行阻塞网络调用，再次显示精选的 `auto` / `deepseek-v4-pro` / `deepseek-v4-flash` 行。`/models` 命令仍然列出实时 provider 目录。
- **"批准此会话"再次按命令族分组。** 会话批准再次通过有损的、参数数量感知的指纹进行键控，因此批准 `cargo build` 也会覆盖 `cargo build --release`。拒绝保留 #1617 中的精确每次调用指纹，因此拒绝一次调用不再过度阻止稍后对同一工具的不同调用。
- **Docker 首次运行状态目录可写。** 镜像现在预先创建 `/home/deepseek/.deepseek` 并使用 `deepseek` 所有权，以便文档中指定的命名卷启动可以在首次使用时创建运行时线程状态（#1684）。
- **运行时 API 系统提示覆盖在第一个 turn 后仍然有效。** 使用 `system_prompt` 覆盖创建的线程现在在构建模型请求之前的模式/上下文刷新期间保留该提示（#1688）。
- **压缩在工具密集型历史记录中保留用户文本查询。** 自动压缩现在在保留的尾部仅包含工具调用/结果时固定最新的用户文本消息，避免下一个请求出现 OpenAI 兼容 Jinja 模板失败（#1704）。
- **翻页器跳转定位到可见底部。** 在翻页器中按 `G` 或 End 不再超出渲染限制，因此之后按 `k`/Up 可以立即向上滚动，鼠标滚轮现在直接滚动翻页器叠加层（#1706、#1716）。
- **鼠标滚轮作为箭头滚动保留编辑器草稿。** 当启用 `composer_arrows_scroll` 时，即使编辑器中有文本，Up/Down 现在也会滚动 transcript，而不是用输入历史替换草稿（#1677）。
- **多行编辑器箭头在输入行之间移动。** 普通 Up/Down 现在在多行草稿中先移动光标，然后再回退到输入历史，而单行鼠标滚轮作为箭头滚动保持不变（#1721）。
- **第三方 `reasoning_content` 流不再破坏文本输出。** 在 `reasoning_content` 中流式传输答案文本的通用 OpenAI 兼容 provider 现在将其渲染为普通文本，除非所选 provider 是其推理内容语义受支持的 provider（#1673）。
- **macOS 系统主题检测识别浅色模式。** 当 `COLORFGBG` 缺失或不可用时，`theme = "system"` 现在回退到 macOS 外观检测，并将缺失的 `AppleInterfaceStyle` 键视为浅色模式（#1670）。
- **`rlm_open` 接受模式填充的空白源字段。** 空的 `file_path`、`content` 和 `url` 字符串现在被视为不存在，因此提供一个真实源的调用不再因恰好一个源验证器而失败（#1712）。
- **调整大小后 transcript 翻页立即可用。** 终端调整大小后，PageUp/PageDown 现在使用调整后的视口高度，而不是在下次重新渲染之前回退到单行跳跃（#1715）。
- **`apply_patch` 在模糊匹配中保留制表符。** 当使用模糊匹配时，`apply_patch` 不再将补丁中的前导制表符替换为空格，保留 `.go`、`Makefile` 和类似文件中以制表符缩进的源的原始缩进（#1672）。

### 变更

- **`/model` 选择器再次成为精选设计界面。** 选择器再次显示稳定的三行设计界面（auto、V4 Pro、V4 Flash），每行提供实时账单/上下文信息。`/models` 仍然按 provider 列出完整的实时目录。当 provider 没有已知的便宜等级模型时，选择器回退到精简的实时模式。
- **`/statusline` 现在是选项加入的，默认隐藏。** 默认页脚不再显示 `cache prefix 0%` 或 `context 1.0M`。输入 `/statusline` 将其切换为可见，输出清晰的 token 计数和上下文使用率小部件。此设置不在会话之间持久化。
- **助手推理令牌在 `SessionUsage` 中标记为推理。** 流式推理内容现在在 provider 输出端被标记并计入推理计数。
- **飞书/Lark 桥接依赖项已锁定和审计。** 桥接现在提供包锁，在 Lighthouse 上可用时使用 `npm ci` 安装，并将 Lark SDK 的传递依赖 `axios` 覆盖到已修补的行。这解决了内部安全审计标记的 `CVE-2025-27152`。
- **Sakana AI `fugu-ultra` 模型已注册。** `fugu-ultra-20260615` 已添加到静态模型注册表，具有 262K 上下文窗口、65K 最大输出、推理支持和工具调用支持。
- **默认搜索后端切换到 Bing。** 在没有显式 `[search] provider` 配置或环境覆盖的情况下，`web_search` 后端现在默认为 Bing。DuckDuckGo 仍然可以通过 `[search] provider = "duckduckgo"` 选择，并保留其 Bing 回退路径。

### 修复（续）

- **首次运行引导在没有 API key 的情况下仍可使用。** 缺失密钥的启动不再在引导收集 provider 设置之前中止 TUI。
- **Streamable HTTP MCP 会话保留其服务器颁发的会话 ID。** 自定义标头也应用于 GET 预检请求，修复了需要两者的已认证 MCP 服务器。
- **DeepSeek 模型补全使用规范 ID。** 别名补全现在在写入配置之前解析为稳定的 DeepSeek 模型名称。
- **终端和子进程可靠性更紧密。** 信号关闭现在恢复终端，子任务保留代理环境变量，Windows Enter / CSI-u 输入处理避免了之前的事件不匹配。
- **长终端文本换行而不是溢出。** 流式输出、diff 渲染和翻页器现在对超长的无空格和 CJK 文本段进行强制换行。
- **发布和平台边缘更安全。** TUI 不再触发 Windows Instant-underflow 测试路径，不支持的桌面目标编译外部 URL 打开器，旧版 DeepSeek CN provider 别名反序列化为规范 DeepSeek provider。
- **页脚诊断更清晰。** 前缀缓存稳定性不再在默认页脚中显示，选项加入的 `/statusline` 小部件现在显示 `cache prefix 100%` 而不是模糊的 `P 100%`。
- **飞书/Lark 桥接依赖项已锁定和审计。**（见上文变更部分）
- **中国友好的更新回退。** `deepseek update` 现在通过 `DEEPSEEK_TUI_RELEASE_BASE_URL` 加 `DEEPSEEK_TUI_VERSION` 支持镜像发布资产，其网络故障提示指向在 GitHub 屏蔽网络后的用户使用 CNB `cargo install --git` 路径安装两个二进制文件。
- **CNB 是默认的腾讯候选发布镜像。** CNB 同步工作流现在镜像飞书/Lighthouse 发布分支，因此腾讯 Lighthouse 引导可以在发布分支合并前使用 CNB。

### 致谢

感谢 **ZzzPL ([@Oliver-ZPLiu](https://github.com/Oliver-ZPLiu))** 的 MCP Streamable HTTP 和 Homebrew 自动化修复（#1643、#1631），**Reid ([@reidliu41](https://github.com/reidliu41))** 的 CI、流式换行和模型补全修复（#1603、#1628、#1601），**MidoriKurage ([@mdrkrg](https://github.com/mdrkrg))** 的引导崩溃修复（#1598），**Gordon ([@gordonlu](https://github.com/gordonlu))** 的 Windows Enter / CSI-u 修复（#1612），**Aitensa ([@Aitensa](https://github.com/Aitensa))** 的 CJK diff/翻页器换行修复（#1622），**qiyan233 ([@qiyan233](https://github.com/qiyan233))** 的旧版 DeepSeek CN provider 别名（#1645），**jieshu666 ([@jieshu666](https://github.com/jieshu666))** 的重绘闪烁减少（#1563），**Vishnu ([@Vishnu1837](https://github.com/Vishnu1837))** 的信号终端恢复（#1586），以及 **axobase001 ([@axobase001](https://github.com/axobase001))** 的子任务代理环境变量保留（#1608）。
