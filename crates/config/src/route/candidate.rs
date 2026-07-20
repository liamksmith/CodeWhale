//! 运行时解析的可执行路由 (#3384)。
//!
//! [`ReadyRouteCandidate`] 是 #2608 契约的具体形式：
//!
//! > 执行需要一个 `ReadyRouteCandidate`。
//! > `ReadyRouteCandidate` 只能由 `RouteResolver` 生成。
//!
//! 字段是公开可*读*的，但该类型无法在此 crate 之外*构造*：
//! 结构体是 `#[non_exhaustive]`（其他 crate 无法通过结构体字面量构建它），
//! 并且有意不派生 `Deserialize`（因此也无法从 JSON 构建）。
//! 唯一的构造器是 [`ReadyRouteCandidate::new`]（`pub(super)`），
//! 而 [`super::resolver::RouteResolver::resolve`] 是其唯一的调用者。
//! 因此，候选者的存在即证明它已通过解析器。
//!
//! 延期：#3384 的完整设计还包含 `capabilities: CapabilityProfile`
//! 和 `config_snapshot: Config`。两者都被有意省略：将 `CapabilityProfile`
//! 拉入 `crates/config` 会强制进行 `tui -> config` 的类型移动，
//! 而嵌入 `Config` 会将候选者与完整的配置模型耦合。
//! 当这些类型在此 crate 中有了合适的位置时，它们将被添加。

use serde::{Deserialize, Serialize};

use super::RequestProtocol;
use super::ids::{LogicalModelRef, ModelId, ProviderId, WireModelId};
use super::offering::RouteLimits;
use crate::ProviderKind;

/// 路由将要通信的具体已解析端点。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedEndpoint {
    /// 已解析的基 URL（在任何覆盖之后）。
    pub base_url: String,
    /// 端点键（例如 `"chat"`、`"responses"`）。
    pub endpoint_key: String,
    /// 此端点使用的线路协议。
    pub protocol: RequestProtocol,
}

/// 为路由解析的认证来源类别。
///
/// 仅记录凭证的*来源位置*，绝不记录凭证值本身。
/// 有意不包含任何可能持有秘密的字段。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedAuthSource {
    /// 通过 CLI 标志/参数提供。
    Cli,
    /// 从配置文件中读取。
    ConfigFile,
    /// 从操作系统密钥环中读取。
    Keyring,
    /// 从环境变量中读取。
    Env,
    /// 通过运行命令产生。
    Command,
    /// 从命名的密钥中解析。
    Secret,
    /// 未解析到凭证。
    Missing,
}

/// 已解析路由的定价/配额类别。
///
/// 仅携带粗略的、非敏感的形态；不会包含秘密或账户 ID。
///
/// `PartialEq`（但不是 `Eq`：`Token` 费率是 `f64`）允许在测试中比较 offerings
/// 和 candidates，并让 [`super::offering::ProviderModelOffering`] 携带定价表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PricingSku {
    /// 按令牌定价。
    Token {
        /// 每百万令牌的输入价格（如果已知）。
        input_per_mtok: Option<f64>,
        /// 每百万令牌的输出价格（如果已知）。
        output_per_mtok: Option<f64>,
    },
    /// 订阅配额使用情况。
    SubscriptionQuota {
        /// 已使用的配额百分比（如果已知）。
        used_pct: Option<f32>,
        /// 配额重置时间（如果已知）。
        resets_at: Option<String>,
    },
    /// 预付账户余额。
    AccountCredits {
        /// 剩余余额（如果已知）。
        balance: Option<f64>,
    },
    /// 本地或其他不计费。
    LocalOrNotApplicable,
    /// 定价未知或已过时。
    UnknownOrStale,
}

/// 路由验证的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    /// 路由是否通过验证。
    pub ok: bool,
    /// 人类可读的诊断信息（建议性质；不含秘密）。
    pub messages: Vec<String>,
}

/// 运行时解析的可执行路由。
///
/// 字段对调用者来说是只读的；该类型无法在此 crate 外部构造
/// （`#[non_exhaustive]` + 无 `Deserialize`）。唯一的构造器是
/// [`Self::new`]，为 `pub(super)`；参见模块文档。
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ReadyRouteCandidate {
    /// 已解析的提供者 ID。
    pub provider_id: ProviderId,
    /// 已解析的提供者类型。
    pub provider_kind: ProviderKind,
    /// 用户/路由请求的选择器。
    pub logical_model: LogicalModelRef,
    /// 规范的模型标识（如果已解析）。
    pub canonical_model: Option<ModelId>,
    /// 提供者拥有的、放在请求上的线路 ID。
    pub wire_model_id: WireModelId,
    /// 已解析的端点传输信息。
    pub endpoint: ResolvedEndpoint,
    /// 已解析的认证来源类别（绝不会是秘密值）。
    pub auth: ResolvedAuthSource,
    /// 选定的线路协议。
    pub protocol: RequestProtocol,
    /// 路由/offering 范围的令牌限制（如果已知）。
    pub limits: RouteLimits,
    /// 定价/配额类别（如果已知）。
    pub pricing: Option<PricingSku>,
    /// 验证结果。
    pub validation: ValidationReport,
}

impl ReadyRouteCandidate {
    /// 创建一个候选者。限制给 [`super::resolver`]，以便解析器是
    /// 可执行路由的唯一生产者（#2608 变更门）。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        provider_id: ProviderId,
        provider_kind: ProviderKind,
        logical_model: LogicalModelRef,
        canonical_model: Option<ModelId>,
        wire_model_id: WireModelId,
        endpoint: ResolvedEndpoint,
        auth: ResolvedAuthSource,
        protocol: RequestProtocol,
        limits: RouteLimits,
        pricing: Option<PricingSku>,
        validation: ValidationReport,
    ) -> Self {
        Self {
            provider_id,
            provider_kind,
            logical_model,
            canonical_model,
            wire_model_id,
            endpoint,
            auth,
            protocol,
            limits,
            pricing,
            validation,
        }
    }
}
