# Model Lab 路线图

Model Lab 是 CodeWhale 计划中的开放模型工作台。北极星目标很简单：
CodeWhale 应该让开源和开放权重模型在每个提供这些模型的提供商那里都能在终端编码工作流中实际可用。Model Lab 让这些模型变得可发现、可评估、可路由、可服务和可导出，同时不削弱当前的终端代理契约：本地工作区控制、显式的提供商认证、审批门控以及清晰的隐私边界。

本文档是路线图语言。以下部分工作集仅为路线图规划。

## 当前已实现

- DeepSeek 是当前的首选默认提供商，支持 `deepseek-v4-pro`、
  `deepseek-v4-flash`、流式思考块、Fin 路由、`DEEPSEEK_*`
  环境变量以及 `~/.deepseek` 配置兼容。
- OpenRouter、Novita、Fireworks、NVIDIA NIM、AtlasCloud、万界方舟、Hugging
  Face Inference Providers、通用 OpenAI 兼容端点、SGLang、vLLM
  和 Ollama 均作为支持的提供商路径，其 ID 出现在
  `/provider`、`codewhale --provider` 或 `codewhale models` 中。
- Hugging Face Inference Providers 通过位于 `https://router.huggingface.co/v1` 的
  OpenAI 兼容路由器提供。使用 `huggingface`、`hugging-face`、`hugging_face` 或 `hf` 选择路由；
  配置 `HUGGINGFACE_API_KEY` 或 `HF_TOKEN` 进行认证。
- 模型自动路由在每个回合中选择具体的 DeepSeek 模型和思考级别。
  它不是 TUI 模式。
- Fin 是快速的 `deepseek-v4-flash` 关闭思考模式路径，用于路由、
  摘要、轻量检查、RLM 子调用、唤醒验证和
  二进制补全检查。
- 自托管的 OpenAI 兼容端点可以通过 SGLang、vLLM、
  Ollama 或通用的 `openai` 提供商配置使用。

## 仍在计划中

- 原生的 Hugging Face Hub 浏览器、模型通行证选择器或直接的 Hub 搜索
  工作流。OpenAI 兼容的 Hugging Face Inference Providers 路由作为聊天提供商已单独实现。
- 内置的 Hugging Face 模型卡片、数据集、适配器、safetensors、Spaces 或
  Jobs 工作流。
- 原生的 Unsloth、NeMo 或 Arcee 集成。
- 专用的 Model Lab UI 标签页。
- 内置的评估排行榜、托管可观测性或训练基础设施
  编排。

在这些功能落地之前，请使用上述提供商路径、MCP 服务器或用户显式配置的外部
工作流。

## Model Lab 原则

Model Lab 应帮助用户回答实际问题：

- 这个回合应该由哪个模型处理？
- 我可以在本地或通过受信任的提供商运行哪个开放或开放权重模型？
- 哪个提供商提供具有我需要的延迟、价格、上下文窗口、
  许可证和隐私姿态的该模型？
- 这个模型花费了多少，表现如何，哪些数据离开了我的机器？
- 我能否复现、导出或自托管该路由？

它绝不应隐藏提供商边界、静默上传本地工件，或在 CodeWhale 实际能够路由到该模型之前
将其描述为可用。

## Hugging Face 工作集

当前已实现：

- Hugging Face Inference Providers 作为显式的 OpenAI 兼容路由器
  提供商，通过 `huggingface`、`hugging-face`、`hugging_face` 或
  `hf` 选择。
- 模型 ID 按选择原样发送到路由器，包括
  带组织前缀的 Hugging Face 模型 ID。

计划范围：

- Hub API 认证和模型发现。
- 以终端友好的方式展示模型卡片、许可证、标签、safetensors 元数据、适配器和数据集
  链接。
- 在已有的独立 Hugging Face Inference Providers 聊天路由之上，提供原生 Hub 浏览器和模型通行证元数据。
- Hugging Face Jobs 作为用户批准实验的可选远程执行路径。

当前非目标：在这些功能在代码中实现之前，声称存在原生 Hub 搜索、模型通行证、Spaces/Jobs 或
Model Lab UI。
推理提供商 API 密钥并不意味着 Hub 浏览/导出、上传或
Jobs 授权。

## Unsloth 工作集

计划范围：

- 为已拥有数据和计算路径的用户提供微调配方和适配器工作流。
- 保持数据集、适配器和检查点位置显式的导出指南。
- 对可返回本地服务或托管 OpenAI 兼容端点的模型的兼容性说明。

## NeMo 工作集

计划范围：

- 为运营 NVIDIA 中心基础设施的用户提供训练和对齐工作流说明。
- 在现有的 NVIDIA NIM 推理支持与未来的 NeMo 训练或自定义工作流之间保持清晰的边界。

## Arcee 工作集

计划范围：

- 小模型路由和专业化实验。
- 可导出的路由，明确显示任务是由较小模型、Fin 还是完整 DeepSeek 推理处理的。

## 服务工作集

计划范围：

- 为 SGLang、vLLM、Ollama 和 OpenAI 兼容网关提供更好的本地和私有服务体验。
- 健康检查、模型列表、上下文窗口元数据和路由验证。
- 无静默网络暴露：公共端点必须显式配置。

## 评估工作集

计划范围：

- 针对编码、审查、文档、发布检查和长上下文工作流的可复现任务套件。
- 并排路由比较，捕获确切的模型、提供商、思考级别、提示和工具策略。

## 可观测性工作集

计划范围：

- 本地优先的跟踪，涵盖回合路由、工具调用、审批、成本、缓存行为和上下文压力。
- 导出规则，在数据离开机器之前脱敏密钥并要求显式的用户操作。

## 训练基础设施工作集

计划范围：

- 数据集准备、适配器训练、工件命名和推广到服务的配方。
- 本地/私有工件与发布到 Hub 或注册表的任何内容之间的分离。

## 隐私和导出规则

- 本地文件、提示、转录、跟踪、模型输出、评估结果、
  适配器、数据集和检查点应保持本地，除非用户
  显式选择提供商或导出目标。
- 提供商认证必须保持显式。`DEEPSEEK_*`、OpenRouter、
  `HUGGINGFACE_API_KEY` / `HF_TOKEN` 和自托管凭据不应从不相关的配置中推断。
- 可导出的工件应包含来源信息：源模型、提供商、
  路由、工具策略、评估输入和脱敏状态。
- 公开分享、托管遥测、赞助徽章和外部品牌推广需要维护者批准。
