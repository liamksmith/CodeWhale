//! CodeWhale 模型事实的单一来源（#3071, #3073）。
//!
//! 历史上，"这个模型的上下文窗口/最大输出是多少/它是否支持推理？"
//! 由几个硬编码位置回答：
//!
//! * [`crate::models::context_window_for_model`] /
//!   [`crate::models::known_context_window_for_model`] 用于上下文窗口，
//! * [`crate::models::max_output_tokens_for_model`] 用于输出限制，
//! * [`crate::models::model_supports_reasoning`] 用于推理标志，
//! * `crates/config/src/lib.rs` 中的 `DEFAULT_*` 模型 ID 常量用于每个提供商默认提供的标准模型。
//!
//! 此模块是将它们整合到一个地方的 **基础**：一个以模型 ID 为键的 [`ModelMetadata`] 注册表，
//! 加上一个统一的 [`lookup`] 入口点。它有意是 **附加性的** —— 现有的调用站点
//! 在此次传递中保持不变，将在后续更改中迁移为使用注册表
//!（因此今天行为不变）。
//!
//! ## 播种纪律（无漂移）
//!
//! 注册表不会重新声明上下文窗口/最大输出/推理
//! 数字。相反，它通过调用现有的 `crate::models` 函数来 **播种** 每个条目，
//! 因此注册表永远不会与 `models.rs` 悄然不一致。
//! 标准模型 ID 来自配置 crate 提供的相同提供商默认值
//!（参见 [`SEED_MODEL_IDS`]）。[`tests::registry_context_window_matches_models_rs`]
//! 漂移保护随后重新断言示例的等价性，以便如果未来的更改
//! 将播种替换为硬编码字面量，CI 会立即捕获漂移。
//!
//! 注意：这里的公开表面有意尚未被生产调用点使用
//!（消费者在后续传递中接入），因此在此前模块级别允许
//! `dead_code`。
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::models::{
    context_window_for_model, max_output_tokens_for_model, model_supports_reasoning,
};

/// 模型条目的粗略提供商分组。
///
/// 这有意是一个小而稳定的枚举，而不是 `config::ApiProvider` 的重新导出：
/// 注册表的工作是回答"这是什么类型的模型"，
/// 而许多模型（Kimi、GLM、Qwen……）可以通过
/// 几个具体的提供商访问。路由决策仍然在
/// `config::ApiProvider` / `model_routing` 中；这只是一个提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelProvider {
    /// DeepSeek 系列模型（一级公民；保留完整支持）。
    DeepSeek,
    /// Anthropic Claude 模型。
    Anthropic,
    /// OpenAI 公共 API 模型（GPT-5.5 / GPT-5.6 系列）。
    OpenAi,
    /// OpenAI Codex 路由模型（gpt-5*-codex）。
    OpenAiCodex,
    /// Moonshot / Kimi 模型。
    Moonshot,
    /// Z.ai GLM 模型。
    Zai,
    /// MiniMax 模型。
    Minimax,
    /// 阿里 Qwen 模型。
    Qwen,
    /// Arcee Trinity 模型。
    Arcee,
    /// 小米 MiMo 模型。
    XiaomiMimo,
    /// Meta Muse 模型。
    Meta,
    /// xAI / Grok 模型。
    Xai,
    /// 其他未分类的模型（仍通过 `models.rs` 启发式方法在可能时获得真实元数据）。
    Other,
}

/// 一行模型事实，在 [`lookup`] 中查找。
///
/// 所有数字字段都从 `crate::models` 播种，因此它们与
/// 旧版查找保持同步（参见模块文档）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelMetadata {
    /// 发送给提供商的标准模型 ID（例如 `"deepseek-v4-pro"`）。
    pub id: &'static str,
    /// 粗略的提供商分组。
    pub provider: ModelProvider,
    /// 如果已知，近似的上下文窗口（以 token 为单位）。
    pub context_window: Option<u32>,
    /// 如果已知，近似的最大输出 token 数。
    pub max_output: Option<u32>,
    /// 模型是否发出必须在答案正文中保持分离的推理/思考内容。
    pub supports_reasoning: bool,
}

impl ModelMetadata {
    /// 通过从现有的 `crate::models` 查找中播种每个事实来为 `id` 构建元数据行。
    /// 这是唯一的构造函数，这使注册表不会偏离 `models.rs`。
    fn seed(id: &'static str, provider: ModelProvider) -> Self {
        Self {
            id,
            provider,
            context_window: context_window_for_model(id),
            max_output: max_output_tokens_for_model(id),
            supports_reasoning: model_supports_reasoning(id),
        }
    }
}

/// 注册表的标准 `(model id, provider)` 种子。
///
/// 这些镜像了 `crates/config/src/lib.rs` 提供的提供商默认值
///（`DEFAULT_*_MODEL` 常量）加上 [`crate::models::known_context_window_for_model`] 中
/// 显式枚举的模型。保持此列表精选：
/// 这是我们做出头等承诺的模型集合。未知 ID 仍然通过
/// [`lookup`] 经由 `models.rs` 启发式方法回答，它们只是
/// 不在此处预播种。
const SEED_MODEL_IDS: &[(&str, ModelProvider)] = &[
    // --- DeepSeek（一级公民；配置 DEFAULT_DEEPSEEK_MODEL / NIM / OpenAI
    // / Atlascloud / Novita / Fireworks / Siliconflow / SGLang / vLLM /
    // Huggingface / Together / Volcengine / WanjieArk / Ollama 默认值） ---
    ("deepseek-v4-pro", ModelProvider::DeepSeek),
    ("deepseek-v4-flash", ModelProvider::DeepSeek),
    ("deepseek-ai/deepseek-v4-pro", ModelProvider::DeepSeek),
    ("deepseek-ai/deepseek-v4-flash", ModelProvider::DeepSeek),
    ("deepseek/deepseek-v4-pro", ModelProvider::DeepSeek),
    ("deepseek/deepseek-v4-flash", ModelProvider::DeepSeek),
    ("deepseek-reasoner", ModelProvider::DeepSeek),
    ("deepseek-coder:1.3b", ModelProvider::DeepSeek),
    // --- Anthropic（配置 DEFAULT_ANTHROPIC_MODEL + models.rs 行） ---
    ("claude-opus-4-8", ModelProvider::Anthropic),
    ("claude-sonnet-4-6", ModelProvider::Anthropic),
    ("claude-sonnet-5", ModelProvider::Anthropic),
    ("claude-fable-5", ModelProvider::Anthropic),
    ("claude-haiku-4-5", ModelProvider::Anthropic),
    // --- OpenAI 公共 API + Codex（配置 DEFAULT_OPENAI_CODEX_MODEL） ---
    ("gpt-5.5", ModelProvider::OpenAi),
    ("gpt-5.5-pro", ModelProvider::OpenAi),
    ("gpt-5.6", ModelProvider::OpenAi),
    ("gpt-5.6-sol", ModelProvider::OpenAi),
    ("gpt-5.6-terra", ModelProvider::OpenAi),
    ("gpt-5.6-luna", ModelProvider::OpenAi),
    ("gpt-5-codex", ModelProvider::OpenAiCodex),
    ("gpt-5.3-codex", ModelProvider::OpenAiCodex),
    // --- Moonshot / Kimi（配置 DEFAULT_MOONSHOT_MODEL / KIMI_CODE） ---
    ("kimi-k2.7-code", ModelProvider::Moonshot),
    ("kimi-k2.6", ModelProvider::Moonshot),
    ("kimi-for-coding", ModelProvider::Moonshot),
    ("moonshotai/kimi-k2.7-code", ModelProvider::Moonshot),
    ("moonshotai/kimi-k2.6", ModelProvider::Moonshot),
    // --- Z.ai GLM（配置 DEFAULT_ZAI_MODEL） ---
    ("z-ai/glm-5.1", ModelProvider::Zai),
    ("z-ai/glm-5.2", ModelProvider::Zai),
    ("glm-5.1", ModelProvider::Zai),
    ("glm-5.2", ModelProvider::Zai),
    // --- MiniMax（配置 DEFAULT_MINIMAX_MODEL） ---
    ("minimax/minimax-m3", ModelProvider::Minimax),
    ("minimax-m3", ModelProvider::Minimax),
    ("minimax/minimax-m2.7", ModelProvider::Minimax),
    ("minimax-m2.7", ModelProvider::Minimax),
    // --- Qwen（OpenRouter 路由默认值） ---
    ("qwen/qwen3.6-flash", ModelProvider::Qwen),
    ("qwen/qwen3.6-plus", ModelProvider::Qwen),
    ("qwen/qwen3.6-35b-a3b", ModelProvider::Qwen),
    // --- Arcee Trinity（配置 DEFAULT_ARCEE_MODEL） ---
    ("trinity-large-thinking", ModelProvider::Arcee),
    ("arcee-ai/trinity-large-thinking", ModelProvider::Arcee),
    ("trinity-mini", ModelProvider::Arcee),
    // --- Sakana / Fugu（配置 DEFAULT_SAKANA_MODEL） ---
    ("fugu-ultra-20260615", ModelProvider::Other),
    ("fugu-ultra", ModelProvider::Other),
    // --- StepFun（配置 DEFAULT_STEPFUN_MODEL） ---
    ("step-3.7-flash", ModelProvider::Other),
    // --- 小米 MiMo（配置 DEFAULT_XIAOMI_MIMO_MODEL） ---
    ("mimo-v2.5-pro", ModelProvider::XiaomiMimo),
    ("mimo-v2.5-pro-ultraspeed", ModelProvider::XiaomiMimo),
    ("mimo-v2.5", ModelProvider::XiaomiMimo),
    // --- Meta 模型 API（配置 DEFAULT_META_MODEL） ---
    ("muse-spark-1.1", ModelProvider::Meta),
    // --- xAI / Grok（配置 DEFAULT_XAI_MODEL） ---
    ("grok-4.5", ModelProvider::Xai),
    ("grok-4.3", ModelProvider::Xai),
    ("grok-build", ModelProvider::Xai),
    ("grok-composer-2.5-fast", ModelProvider::Xai),
    ("grok-4.20-0309-reasoning", ModelProvider::Xai),
    ("grok-4.20-0309-non-reasoning", ModelProvider::Xai),
];

fn registry() -> &'static BTreeMap<&'static str, ModelMetadata> {
    static REGISTRY: OnceLock<BTreeMap<&'static str, ModelMetadata>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        SEED_MODEL_IDS
            .iter()
            .map(|&(id, provider)| (id, ModelMetadata::seed(id, provider)))
            .collect()
    })
}

/// 按 ID 查找模型事实。
///
/// 当 `model` 是标准 [`SEED_MODEL_IDS`] 之一时返回预播种的 [`ModelMetadata`]
///（不区分大小写）。对于任何其他 ID，此函数回退到
/// 相同的 `crate::models` 启发式方法（显式 `_Nk` 后缀、DeepSeek/Claude
/// 家族规则等），并将提供商报告为 [`ModelProvider::Other`]，因此
/// 调用者总能获得可用的答案，而不是对于真实模型返回 `None`。
///
/// 仅当 ID 无法被任何现有来源识别时返回 `None`
///（没有种子匹配且 `models.rs` 不产生上下文窗口）。
#[must_use]
pub fn lookup(model: &str) -> Option<ModelMetadata> {
    if let Some(meta) = registry().get(model) {
        return Some(meta.clone());
    }
    // 不区分大小写的种子匹配（模型 ID 由旧版 `models.rs` 辅助方法进行小写比较，
    // 因此这里也遵循此规则）。
    let lowered = model.to_lowercase();
    if lowered != model
        && let Some(meta) = registry().get(lowered.as_str())
    {
        return Some(meta.clone());
    }

    // 未预播种：委托给现有的启发式方法。如果它们至少认出模型（任何已知的上下文窗口），
    // 则合成一行，以便单一查找入口点仍然适用于长尾 ID。
    let context_window = context_window_for_model(model);
    let max_output = max_output_tokens_for_model(model);
    let supports_reasoning = model_supports_reasoning(model);
    if context_window.is_none() && max_output.is_none() && !supports_reasoning {
        return None;
    }
    Some(ModelMetadata {
        // ID 在此非 `'static`；我们无法存储它，因此此合成行
        // 报告空 ID。预播种的行（常见情况）携带真实 ID。
        // 这使公开类型保持 `'static` 干净而不泄漏。
        id: "",
        provider: ModelProvider::Other,
        context_window,
        max_output,
        supports_reasoning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 漂移保护（#3071, #3073）。
    ///
    /// 注册表必须与 `crate::models` 就其声称知道的每个模型的上下文窗口一致。
    /// 今天它们一致是因为注册表是从 `models.rs` *播种* 的；此测试存在
    /// 以便如果未来的更改将播种替换为从 `models.rs` 漂移的硬编码字面量，
    /// CI 在此失败，而不是交付两个不一致的事实来源。
    #[test]
    fn registry_context_window_matches_models_rs() {
        // 跨越每个提供商分组和旧版表生成的每个不同窗口桶的代表性样本。
        let sample = [
            ("deepseek-v4-pro", Some(1_000_000)),
            ("deepseek-v4-flash", Some(1_000_000)),
            ("deepseek-coder:1.3b", Some(128_000)),
            ("claude-opus-4-8", Some(1_000_000)),
            ("claude-sonnet-4-6", Some(1_000_000)),
            ("claude-sonnet-5", Some(1_000_000)),
            ("claude-fable-5", Some(1_000_000)),
            ("claude-haiku-4-5", Some(200_000)),
            ("gpt-5.5", Some(1_050_000)),
            ("gpt-5.6", Some(1_050_000)),
            ("gpt-5.6-terra", Some(1_050_000)),
            ("gpt-5-codex", Some(400_000)),
            ("kimi-k2.7-code", Some(262_144)),
            ("kimi-k2.6", Some(262_144)),
            ("z-ai/glm-5.1", Some(202_752)),
            ("z-ai/glm-5.2", Some(1_000_000)),
            ("minimax/minimax-m3", Some(1_000_000)),
            ("minimax-m2.7", Some(204_800)),
            ("qwen/qwen3.6-flash", Some(1_000_000)),
            ("qwen/qwen3.6-35b-a3b", Some(262_144)),
            ("trinity-large-thinking", Some(262_144)),
            ("trinity-mini", Some(128_000)),
            ("mimo-v2.5-pro", Some(1_000_000)),
            ("mimo-v2.5-pro-ultraspeed", Some(1_000_000)),
            ("mimo-v2.5", Some(1_000_000)),
            ("muse-spark-1.1", Some(1_000_000)),
            ("grok-4.5", Some(500_000)),
            ("grok-4.3", Some(1_000_000)),
            ("grok-4.20-0309-reasoning", Some(2_000_000)),
        ];
        for (model, expected) in sample {
            let meta = lookup(model)
                .unwrap_or_else(|| panic!("seeded model {model} should be in the registry"));
            // 1. 注册表值等于文档化的期望值。
            assert_eq!(
                meta.context_window, expected,
                "registry context window for {model} drifted from expected"
            );
            // 2. 注册表值等于实时的 models.rs 值（真正的保护：
            //    捕获任何将来漂移的硬编码字面量）。
            assert_eq!(
                meta.context_window,
                context_window_for_model(model),
                "registry context window for {model} drifted from models.rs"
            );
        }
    }

    #[test]
    fn registry_max_output_and_reasoning_match_models_rs() {
        for &(id, _) in SEED_MODEL_IDS {
            let meta = lookup(id).unwrap_or_else(|| panic!("{id} should be seeded"));
            assert_eq!(
                meta.max_output,
                max_output_tokens_for_model(id),
                "registry max_output for {id} drifted from models.rs"
            );
            assert_eq!(
                meta.supports_reasoning,
                model_supports_reasoning(id),
                "registry supports_reasoning for {id} drifted from models.rs"
            );
        }
    }

    #[test]
    fn deepseek_models_are_classified_as_deepseek() {
        // 品牌/头等 DeepSeek 支持保护：默认的 DeepSeek
        // 模型必须存在并被归类为 DeepSeek。
        for id in [
            "deepseek-v4-pro",
            "deepseek-v4-flash",
            "deepseek-ai/deepseek-v4-pro",
        ] {
            let meta = lookup(id).expect("DeepSeek default should be seeded");
            assert_eq!(meta.provider, ModelProvider::DeepSeek);
            assert_eq!(meta.context_window, Some(1_000_000));
        }
    }

    #[test]
    fn xai_models_are_classified_as_xai() {
        let meta = lookup("grok-4.5").expect("xAI default should be seeded");
        assert_eq!(meta.provider, ModelProvider::Xai);
        assert_eq!(meta.context_window, Some(500_000));
        assert!(meta.supports_reasoning);

        let fast = lookup("grok-4.20-0309-non-reasoning").expect("xAI fast model should be seeded");
        assert_eq!(fast.provider, ModelProvider::Xai);
        assert_eq!(fast.context_window, Some(2_000_000));
        assert!(!fast.supports_reasoning);
    }

    #[test]
    fn meta_muse_spark_is_classified_as_meta() {
        let meta = lookup("muse-spark-1.1").expect("Muse Spark default should be seeded");
        assert_eq!(meta.provider, ModelProvider::Meta);
        assert_eq!(meta.context_window, Some(1_000_000));
        assert_eq!(meta.max_output, Some(32_000));
        assert!(meta.supports_reasoning);
    }

    #[test]
    fn lookup_is_case_insensitive_for_seeded_ids() {
        let lower = lookup("deepseek-v4-pro").expect("seeded");
        let upper = lookup("DeepSeek-V4-Pro").expect("case-insensitive seed match");
        assert_eq!(upper.id, "deepseek-v4-pro");
        assert_eq!(upper.context_window, lower.context_window);
        assert_eq!(upper.provider, ModelProvider::DeepSeek);
    }

    #[test]
    fn lookup_falls_back_to_models_rs_for_unseeded_known_ids() {
        // `deepseek-v3.2-256k-preview` 不在 SEED_MODEL_IDS 中，但 models.rs
        // 通过显式的 `_Nk` 提示认出它。统一的查找入口点
        // 必须仍然回答它，而不是返回 None。
        let meta = lookup("deepseek-v3.2-256k-preview").expect("known via models.rs heuristics");
        assert_eq!(meta.context_window, Some(256_000));
        assert_eq!(
            meta.context_window,
            context_window_for_model("deepseek-v3.2-256k-preview")
        );
        assert_eq!(meta.provider, ModelProvider::Other);
    }

    #[test]
    fn lookup_returns_none_for_completely_unknown_model() {
        assert!(lookup("totally-made-up-model-xyz").is_none());
    }
}
