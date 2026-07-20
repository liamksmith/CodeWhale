//! 用于提供者/模型/路由标识的透明字符串新类型。
//!
//! 这些类型在类型层面上使路由字符串的不同*含义*清晰可辨，从而调用者不会再混淆：
//!
//! - [`ProviderId`] — 提供者的规范标识（例如 `"deepseek"`）。
//! - [`ModelId`] — 规范的、与提供者无关的逻辑模型标识。
//! - [`WireModelId`] — 提供者拥有的、在请求中发送的线路模型标识
//!   （例如 Together 上的 `"deepseek-ai/DeepSeek-V4-Pro"`）。
//! - [`LogicalModelRef`] — 用户/选择器对模型的引用，可以是 `"auto"`、
//!   裸模型名，或带有聚合器前缀的字符串。
//!
//! [`ModelId`] 和 [`WireModelId`] 是故意设计为*不同*的类型，且永不可互换：
//! 规范的模型标识与放到线路上的提供者特定字符串不是同一回事。
//!
//! 不变量 (对 #2608 起关键作用): 命名空间前缀*永远不能*成为提供者。
//! 特意*不*提供从 [`LogicalModelRef`] 或 [`NamespaceHint`] 到 [`ProviderId`]
//! 的 `From`/`Into` 转换。像 `deepseek-ai/` 这样的前缀仅是目录/命名空间提示；
//! 它不能作为提供者所有权的证据。请不要添加此类转换。

use std::fmt;

use serde::{Deserialize, Serialize};

/// [`LogicalModelRef`] 的 `"auto"` 路由哨兵值。
///
/// `auto` 是一个选择性加入的路由哨兵——它绝不指向名为 "auto" 的字面模型。
/// 集中定义在此处，以便所有比较位置使用相同的拼写（#4158）。
pub const AUTO_SENTINEL: &str = "auto";

use crate::ProviderKind;

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// 借用内部的字符串切片。
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_newtype!(
    /// 提供者的规范标识符（例如 `"deepseek"`、`"openrouter"`）。
    ProviderId
);

string_newtype!(
    /// 规范的、与提供者无关的逻辑模型标识。
    ///
    /// 与 [`WireModelId`] 不同：这是“模型是什么”，而非“提供者在线上期望什么字符串”。
    ModelId
);

string_newtype!(
    /// 提供者拥有的、在请求中按原样发送的线路模型标识。
    ///
    /// 与 [`ModelId`] 不同：像 `"deepseek-ai/DeepSeek-V4-Pro"` 这样带有聚合器前缀的
    /// 字符串是线路标识，而非规范标识。
    WireModelId
);

string_newtype!(
    /// 用户/选择器对模型的引用。
    ///
    /// 可以是 `"auto"` 哨兵值、裸模型名，或带有聚合器前缀的字符串。
    /// [`LogicalModelRef`] 本身不携带提供者权限；参见 [`Self::namespace_hint`]。
    LogicalModelRef
);

impl ProviderId {
    /// 使用 [`ProviderKind`] 的规范标识构建 [`ProviderId`]。
    #[must_use]
    pub fn from_kind(kind: ProviderKind) -> Self {
        Self(kind.as_str().to_string())
    }
}

/// [`LogicalModelRef`] 携带的前导命名空间/组织前缀。
///
/// 命名空间提示*仅*是目录提示。它*永远不能*转换为 [`ProviderId`]；
/// 聚合器可能提供 `deepseek-ai/...` 但并非 DeepSeek 本身，
/// 自定义端点也可能合法地使用外观相似的字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NamespaceHint {
    /// `deepseek-ai/` 前缀.
    DeepseekAi,
    /// `deepseek/` 前缀.
    Deepseek,
    /// `anthropic/` 前缀.
    Anthropic,
    /// `openai/` 前缀.
    Openai,
    /// `qwen/` 前缀.
    Qwen,
}

impl LogicalModelRef {
    /// 借用原始的选择器字符串。
    #[must_use]
    pub fn raw(&self) -> &str {
        self.as_str()
    }

    /// 该选择器是否为显式的 `auto` 路由哨兵值。
    ///
    /// `auto` 是选择性加入的路由哨兵，绝不是字面模型标识。
    #[must_use]
    pub fn is_auto(&self) -> bool {
        self.raw() == AUTO_SENTINEL
    }

    /// 解析前导命名空间前缀（如果存在）。
    ///
    /// 仅当匹配到精选的聚合器/组织前缀时返回 `Some`。
    /// 这是关于目录命名空间的提示，并*不*标识提供者。
    #[must_use]
    pub fn namespace_hint(&self) -> Option<NamespaceHint> {
        let raw = self.raw();
        // Order matters: `deepseek-ai/` must be matched before `deepseek/`.
        if raw.starts_with("deepseek-ai/") {
            Some(NamespaceHint::DeepseekAi)
        } else if raw.starts_with("deepseek/") {
            Some(NamespaceHint::Deepseek)
        } else if raw.starts_with("anthropic/") {
            Some(NamespaceHint::Anthropic)
        } else if raw.starts_with("openai/") {
            Some(NamespaceHint::Openai)
        } else if raw.starts_with("qwen/") {
            Some(NamespaceHint::Qwen)
        } else {
            None
        }
    }
}
