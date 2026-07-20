//! 路由基础：为 EPIC #2608 新增的、运行时未接入的类型。
//!
//! 此模块树引入了规范的标识 newtype（#3084）和 `ReadyRouteCandidate` / `RouteResolver` 契约（#3384），
//! 而不触及任何运行时路由路径。`config.rs`、TUI、客户端或引擎目前尚未消费此处任何内容；
//! 它是一个自包含的接缝，后续 track 将接入。
//!
//! 层次结构：
//! - [`ids`] — 提供商/模型/线路字符串 newtype + 命名空间提示。
//! - [`descriptor`] — 静态提供商注册表的路由视角视图。
//! - [`offering`] — 提供商/模型提供接缝（线路 ID 绑定）。
//! - [`candidate`] — 运行时解析的可执行路由及其组成部分。
//! - [`errors`] — 路由解析错误。
//! - [`resolver`] — [`candidate::ReadyRouteCandidate`] 的唯一生产者。
//!
//! 命名：请求/响应线路形状拼写为 [`RequestProtocol`]，
//! 它是 [`crate::provider::WireFormat`] 的重新导出别名，而非第四个协议同义词。

#![allow(dead_code)]

/// 所选端点的请求/响应线路形状。
///
/// [`crate::provider::WireFormat`] 的别名；特意不是新枚举，以避免引入又一个协议同义词。
pub use crate::provider::WireFormat as RequestProtocol;

pub mod candidate;
pub mod descriptor;
pub mod errors;
pub mod ids;
pub mod offering;
pub mod resolver;

pub use candidate::{
    PricingSku, ReadyRouteCandidate, ResolvedAuthSource, ResolvedEndpoint, ValidationReport,
};
pub use descriptor::{EndpointDescriptor, ProviderDescriptor};
pub use errors::RouteError;
pub use ids::{LogicalModelRef, ModelId, NamespaceHint, ProviderId, WireModelId};
pub use offering::{ProviderModelOffering, RouteLimits, bundled_offerings};
pub use resolver::{RouteRequest, RouteResolver};

#[cfg(test)]
mod conformance_tests;
#[cfg(test)]
mod tests;
