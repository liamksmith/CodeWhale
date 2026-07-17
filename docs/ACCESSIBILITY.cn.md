# 无障碍访问

DeepSeek-TUI 运行在终端中，因此平台自身的无障碍访问栈（屏幕阅读器、放大镜、终端级主题）完成了大部分工作。TUI 提供少量切换开关，以降低屏幕阅读器和低运动偏好用户的视觉运动和密度。

## 快速参考

| 切换开关 | 默认值 | 效果 |
| --- | --- | --- |
| `NO_ANIMATIONS=1` 环境变量 | 未设置 | 启动时强制 `low_motion = true` 和 `fancy_animations = false`。覆盖 `settings.toml` 中保存的任何值。 |
| `low_motion` 设置 | `false` | 使用更平缓的流式输出节奏和更低的刷新频率，使光标/状态运动不那么激进。底部水条由 `fancy_animations` 单独控制。 |
| `fancy_animations` 设置 | `true` | 底部水柱动画条和脉冲式子代理计数器。设置为 `false` 可使实时对话的 chrome 保持静止。 |
| `status_indicator` 设置 | `whale` | 头部状态芯片。设置为 `dots` 使用紧凑圆点循环，或设置为 `off` 隐藏。 |
| `calm_mode` 设置 | `false` | 默认折叠工具输出详情并精简状态消息。对每次重绘都会播报的屏幕阅读器有用。 |
| `show_thinking` 设置 | `true` | 设置为 `false` 完全隐藏模型 `reasoning_content` 块。 |
| `show_tool_details` 设置 | `true` | 设置为 `false` 将工具调用渲染为一行，不展开载荷。 |

## 标准环境变量接口

在你的 shell 配置文件中设置这些变量，使其应用于每个会话：

```bash
# 强制低运动 + 禁用花哨动画。
export NO_ANIMATIONS=1

# 可选：遵循更广泛的终端颜色约定。
export NO_COLOR=1            # 底层 ratatui 后端会遵循此设置
```

`NO_ANIMATIONS` 接受 `1`、`true`、`yes` 或 `on` 中任意一个（不区分大小写）。任何其他值（包括 `0`、`false`、空值或未设置）将保持你保存的设置不变。

该覆盖在启动时应用一次。在会话中途更改环境变量无效 —— 设置只会在下次启动时重新读取。

## 通过 `/settings` 配置

相同的切换开关也可从命令面板访问：

* `/settings set low_motion on`
* `/settings set fancy_animations off`
* `/settings set calm_mode on`
* `/settings set status_indicator off`

通过此方式写入的设置会持久保存到 `~/.codewhale/settings.toml`（新安装），同时保留 `~/.deepseek/settings.toml` 和平台配置目录设置作为兼容回退。如果设置了 `NO_ANIMATIONS` 环境变量，它仍然会在启动时优先，因此取消设置该环境变量才能遵循你保存的选择。

Tilix 和 Terminator 会话会自动以低运动模式启动，因为这些基于 VTE 的终端在活跃对话期间会出现明显的重绘闪烁。如果你的终端版本渲染干净，你仍然可以在启动后覆盖保存的设置。

## 屏幕阅读器用户注意事项

* `low_motion` 将空闲重绘循环减慢到每帧约 120ms，这样光标就不会不断重新定位。结合 `calm_mode`，重绘频率保持在足够低的水平，使 VoiceOver / Orca 的播报与模型输出线性同步，而不是每次 tick 都重新朗读整个屏幕。
* 转录文本是纯文本 —— 没有图像或画布渲染 —— 因此任何与平台无障碍服务集成的终端（例如 macOS Terminal.app、iTerm2、Ghostty、Windows Terminal）都可以直接传递渲染内容。
* 如果你发现某个 UI 界面在 `low_motion = true` 时仍然产生运动效果，请以 [`PRIOR: Screen-reader / accessibility flag`](https://github.com/Hmbown/CodeWhale/issues/450) 标签提交 issue，并附上截图或终端录制。

## 相关 issue / 历史

* [#450](https://github.com/Hmbown/CodeWhale/issues/450) — 记录现有标志、添加 `NO_ANIMATIONS` 启动覆盖以及编写本页面。
* [#449](https://github.com/Hmbown/CodeWhale/issues/449) — 底部状态栏现在使用活动主题的对比色对，而非自定义调色板。
