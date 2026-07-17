# Issue 分类

## 过时 `needs-info` 清理

过时工作流仅作用于维护者已明确添加 `needs-info` 标签的 issue。这样可以防止旧的路线图、发布、安全性和当前里程碑工作被自动清理，除非维护者首先将 issue 标记为等待报告者输入。

必需的标签：

- `needs-info`：等待报告者信息或当前版本的复现详情。
- `stale`：不活跃的 `needs-info` issue，等待自动关闭。
- `keep-open`：受保护，因为维护者有意保持其开放。
- `pinned`：受保护的维护者 issue。

过时清理的受保护标签：

- `pinned`
- `keep-open`
- `release-blocker`
- `security`

一个 `bug` issue 不会仅因为是 bug 而受到保护。如果维护者同时为其添加了 `needs-info` 标签，则该 issue 有资格收到过时警告和关闭，除非存在上述受保护标签之一。

## 试运行查询

在更改过时策略或进行手动清理之前运行以下命令：

```sh
STALE_CUTOFF=$(python3 -c 'from datetime import date, timedelta; print(date.today() - timedelta(days=45))')
NEEDS_INFO_CUTOFF=$(python3 -c 'from datetime import date, timedelta; print(date.today() - timedelta(days=30))')

gh issue list --repo Hmbown/CodeWhale --state open \
  --search "updated:<${STALE_CUTOFF}" \
  --limit 100 \
  --json number,title,updatedAt,labels,url

gh issue list --repo Hmbown/CodeWhale --state open \
  --search "label:needs-info updated:<${NEEDS_INFO_CUTOFF}" \
  --limit 100 \
  --json number,title,updatedAt,labels,url

gh issue list --repo Hmbown/CodeWhale --state open \
  --search "created:<${STALE_CUTOFF} comments:0 -label:keep-open -label:release-blocker -label:security" \
  --limit 100 \
  --json number,title,createdAt,updatedAt,labels,url
```

使用 `updatedAt`、标签和当前发布相关性作为关闭依据。仅凭创建日期过于激进。

## 首次清理

在依赖自动化之前，执行一次手动清理：

- 仅在询问当前版本的复现详情后，将未解决的老 bug 报告标记为 `needs-info`。
- 关闭明显的 GUI、VS Code 和 Web UI 重复 issue，并附上指向规范的桌面/运行时 issue 的链接。
- 当 CodeWhale 品牌重塑和 README/历史记录工作已涵盖旧品牌讨论 issue 时，将其关闭为已取代。
- 使用 `keep-open` 保护有意的 v0.9.0 路线图片段，或将其关闭为被规范的史诗 issue 取代。

不要仅凭过时自动化来关闭发布阻塞项、安全性 issue 或活跃的里程碑工作。
