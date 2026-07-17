# Xiaomi MiMo Token 计划证据，2026-06-23

本文件夹包含 issue #2621 的浏览器捕获笔记。截图和提取的 JSON 来自小米拥有的页面，以便未来的 provider 目录更新可以将仓库元数据与实时真相来源进行比较。

## 来源

- [Xiaomi MiMo 模型摘要](https://mimo.mi.com/docs/en-US/quick-start/summary/model)
- [Xiaomi MiMo 按量付费定价](https://mimo.mi.com/docs/en-US/price/pay-as-you-go)
- [Xiaomi MiMo Token 计划](https://platform.xiaomimimo.com/token-plan)
- 次要输入：`/Users/hunterbown/Downloads/ai_provider_models_2026_catalog.xlsx`

## 发现

- `mimo-v2.5-pro`、`mimo-v2.5-pro-ultraspeed` 和 `mimo-v2.5` 在 CodeWhale 元数据中被视为 1,000,000 token 的 Xiaomi MiMo V2.5 聊天模型。
- `mimo-v2-omni` 仍是一个 256K 窗口的 V2 系列模型；CodeWhale 不将其用作当前的 `xiaomi-mimo` Omni 简写。
- Token 计划使用量基于信用/配额，与按量付费账户余额不互通。因此，CodeWhale 将直接的 `xiaomi-mimo` 成本保留为未知，直到小米暴露可靠的余额端点。
- 工作簿快照将 `mimo-v2.5` 列为 262,144 token，但小米当前的官方模型摘要显示 V2.5 聊天模型为 1,000,000 token。以官方文档为准。

## 捕获

- `03-xiaomi-model-table.png` / `.json`：官方模型表和 RPM/TPM 说明。
- `04-xiaomi-payg-pricing.png` / `.json`：PAYG 与 Token 计划的分离。
- `05-xiaomi-payg-pricing-table.png`：PAYG 定价表。
- `07-xiaomi-token-plan-public.png` / `.json`：公开 Token 计划套餐页面。

这些文件是证据笔记，而非实时 CI 夹具。自动化测试应断言 CodeWhale 的内置元数据和 provider 行为；未来的文档刷新任务可以重新捕获这些页面并标记差异以供审查。
