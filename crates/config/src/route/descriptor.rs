//! 基于现有内置提供商注册表的提供商描述符（#3084）。
//!
//! [`ProviderDescriptor`] 是对已在 [`crate::provider`] 中的静态 [`provider::Provider`] trait 对象的轻量级路由视角视图。
//! 它仅暴露路由解析所需的传输层事实（id、基础 URL、默认有线模型、环境变量、协议），而无需复制注册表。
//!
//! 由于描述符持有 `&'static dyn Provider`，因此有意不派生 `Serialize`/`PartialEq`。
//! 切勿将 [`ProviderDescriptor`] 嵌入 `Serialize` 结构体中；请改为序列化已解析的事实。

use crate::ProviderKind;
use crate::provider::{self, Provider};

use super::RequestProtocol;
use super::ids::{ProviderId, WireModelId};

/// 内置提供商传输事实的路由面向视图。
///
/// 持有一个 trait 对象，因此故意不可序列化/不可比较。
#[derive(Clone, Copy)]
pub struct ProviderDescriptor {
    /// 此描述符描述的提供商类型。
    pub kind: ProviderKind,
    /// 支撑的静态提供商元数据条目。
    pub inner: &'static dyn Provider,
}

impl ProviderDescriptor {
    /// 为已知的提供商类型构建描述符。
    #[must_use]
    pub fn for_kind(kind: ProviderKind) -> Self {
        Self {
            kind,
            inner: provider::provider_for_kind(kind),
        }
    }

    /// 规范提供商 id。
    #[must_use]
    pub fn id(&self) -> ProviderId {
        ProviderId::from(self.inner.id())
    }

    /// 未设置覆盖时的默认基础 URL。
    #[must_use]
    pub fn default_base_url(&self) -> &'static str {
        self.inner.default_base_url()
    }

    /// 未选择模型时的默认有线模型 id。
    #[must_use]
    pub fn default_wire_model(&self) -> WireModelId {
        WireModelId::from(self.inner.default_model())
    }

    /// 此提供商 API 密钥的环境变量候选。
    #[must_use]
    pub fn env_vars(&self) -> &'static [&'static str] {
        self.inner.env_vars()
    }

    /// 此提供商选择的有线协议。
    #[must_use]
    pub fn protocol(&self) -> RequestProtocol {
        self.inner.wire()
    }
}

impl std::fmt::Debug for ProviderDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderDescriptor")
            .field("kind", &self.kind)
            .field("id", &self.inner.id())
            .field("protocol", &self.inner.wire())
            .finish()
    }
}

/// 具体端点的传输事实。
///
/// 与 [`ProviderDescriptor`] 不同，此结构拥有纯数据，可以安全地嵌入可序列化的路由输出中（参见 [`super::candidate::ResolvedEndpoint`]）。
#[derive(Debug, Clone)]
pub struct EndpointDescriptor {
    /// 稳定的端点键（例如 `"chat"`、`"responses"`）。
    pub endpoint_key: String,
    /// 此端点使用的有线协议。
    pub protocol: RequestProtocol,
    /// 此端点的默认基础 URL。
    pub default_base_url: String,
    /// 是否支持流式传输。
    pub streaming: bool,
}
