//! 供应商描述符合规性测试 (#3084)。
//!
//! 这些测试断言 *每个* 已发布的 `ProviderKind` 都具有格式良好的
//! 面向路由的描述符，并且能够解析出默认路由——这样添加供应商时
//! 如果未正确连接其描述符/解析器行为，将在 CI 中失败，而不是在运行时。
//! 它们有意采用数据驱动方式，基于 [`ProviderKind::all`] 并且不依赖网络；
//! 供应商执行/适配器行为在其他地方测试。

use super::bundled_offerings;
use super::descriptor::ProviderDescriptor;
use super::ids::{LogicalModelRef, ProviderId};
use super::resolver::{RouteRequest, RouteResolver};
use crate::ProviderKind;

fn none_request(kind: ProviderKind) -> RouteRequest {
    RouteRequest {
        explicit_provider: Some(kind),
        model_selector: None,
        saved_provider_model: None,
        base_url_override: None,
    }
}

#[test]
fn every_provider_kind_has_a_wellformed_descriptor() {
    for &kind in ProviderKind::all() {
        let descriptor = ProviderDescriptor::for_kind(kind);

        // 描述符 id 非空且与规范映射一致；
        // 不匹配意味着供应商被添加到了一张表但未添加到另一张表。
        assert!(
            !descriptor.id().as_str().trim().is_empty(),
            "{kind:?}: empty provider id"
        );
        assert_eq!(
            descriptor.id(),
            ProviderId::from_kind(kind),
            "{kind:?}: descriptor id disagrees with ProviderId::from_kind"
        );

        // 路由解析所依赖的传输事实必须存在。
        assert!(
            !descriptor.default_wire_model().as_str().trim().is_empty(),
            "{kind:?}: empty default wire model"
        );
        assert!(
            !descriptor.default_base_url().trim().is_empty(),
            "{kind:?}: empty default base URL"
        );

        // 任何声明的认证环境变量名称必须是真实、非空的键。
        for env_var in descriptor.env_vars() {
            assert!(
                !env_var.trim().is_empty(),
                "{kind:?}: empty env var name in descriptor"
            );
        }

        // 任何 kind 的线缆协议访问器都不应 panic。
        let _ = descriptor.protocol();
    }
}

#[test]
fn every_provider_kind_resolves_its_default_route() {
    let resolver = RouteResolver::new();
    let bundled = bundled_offerings();
    for &kind in ProviderKind::all() {
        let descriptor = ProviderDescriptor::for_kind(kind);
        let candidate = resolver.resolve(&none_request(kind)).unwrap_or_else(|err| {
            panic!("{kind:?}: default (None selector) route must resolve, got {err:?}")
        });

        assert_eq!(
            candidate.provider_kind, kind,
            "{kind:?}: resolved to a different provider"
        );
        assert_eq!(
            candidate.provider_id,
            ProviderId::from_kind(kind),
            "{kind:?}: resolved provider id mismatch"
        );

        // 解析器会优先选择该供应商的捆绑 *默认产品* 线缆 id
        //（如果存在），否则回退到描述符默认线缆模型。
        // 断言这一确切契约，以便将来基于目录的默认值与 `Provider::default_model()`
        // 之间发生偏离时，能给出诚实的消息而不是巧合地通过。
        let expected_wire = bundled
            .iter()
            .find(|offering| {
                offering.provider == ProviderId::from_kind(kind) && offering.default_for_provider
            })
            .map_or_else(
                || descriptor.default_wire_model().as_str().to_string(),
                |offering| offering.wire_model_id.as_str().to_string(),
            );
        assert_eq!(
            candidate.wire_model_id.as_str(),
            expected_wire,
            "{kind:?}: None selector must resolve to the bundled default offering (or descriptor default)"
        );
    }
}

#[test]
fn every_provider_kind_resolves_the_auto_selector() {
    let resolver = RouteResolver::new();
    for &kind in ProviderKind::all() {
        let request = RouteRequest {
            explicit_provider: Some(kind),
            model_selector: Some(LogicalModelRef::from("auto")),
            saved_provider_model: None,
            base_url_override: None,
        };
        let candidate = resolver
            .resolve(&request)
            .unwrap_or_else(|err| panic!("{kind:?}: `auto` must resolve, got {err:?}"));

        assert_eq!(
            candidate.provider_kind, kind,
            "{kind:?}: auto resolved to a different provider"
        );
        assert!(
            candidate.logical_model.is_auto(),
            "{kind:?}: `auto` must stay the auto sentinel, never a literal model"
        );
        // `auto` 在没有目录默认值时回退到描述符默认值，
        // 合规性测试 #2 已验证这一点；这里我们只断言它能解析。
        assert!(
            !candidate.wire_model_id.as_str().trim().is_empty(),
            "{kind:?}: auto resolved to an empty wire model"
        );
    }
}
