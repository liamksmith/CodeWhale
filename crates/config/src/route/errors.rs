//! 路由解析错误（#3384）。
//!
//! `thiserror` 不是此 crate 的依赖项，因此 [`Display`] 和 [`std::error::Error`] 是手动实现的。不添加新依赖。

use std::fmt;

use super::ids::ProviderId;

/// 为什么 [`super::resolver::RouteResolver`] 无法生成候选路由。
#[derive(Debug, Clone)]
pub enum RouteError {
    /// 请求的模型选择器为空。
    EmptyModel,
    /// 无法解析指定的提供商。
    InvalidProvider(String),
    /// 一个模型匹配了多个提供商；调用者必须消除歧义。
    AmbiguousModel(Vec<ProviderId>),
    /// 为严格的直接提供商请求了明显外部的模型。
    ForeignModelForDirectProvider {
        /// 拒绝该模型的严格直接提供商。
        provider: ProviderId,
        /// 被拒绝的外部模型选择器。
        model: String,
    },
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyModel => write!(f, "model selector was empty"),
            Self::InvalidProvider(name) => write!(f, "invalid provider: {name}"),
            Self::AmbiguousModel(providers) => {
                let names: Vec<&str> = providers.iter().map(ProviderId::as_str).collect();
                write!(
                    f,
                    "model matches multiple providers ({}); specify a provider",
                    names.join(", ")
                )
            }
            Self::ForeignModelForDirectProvider { provider, model } => write!(
                f,
                "model {model:?} is not served by direct provider {}",
                provider.as_str()
            ),
        }
    }
}

impl std::error::Error for RouteError {}
