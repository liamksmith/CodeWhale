//! 语言检测 + 将语言映射到处理它的 LSP 服务器二进制文件的固定字典。
//!
//! 内置字典覆盖常见语言。用户可以通过配置文件中的 `[lsp.servers]` 覆盖默认设置，
//! 并通过 `[lsp.custom]` 为额外的文件扩展名注册自定义语言服务器
//! （由 [`super::LspConfig`] 处理，非本文件处理）。

use std::path::Path;

/// 我们知道如何向 LSP 服务器查询的语言。通过 [`detect_language`] 从文件扩展名检测。
/// `Other` 是在我们没有该文件的 LSP 时使用的哨兵值——LSP 管理器将其视为"跳过"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Go,
    Python,
    TypeScript,
    JavaScript,
    Java,
    Php,
    Vue,
    C,
    Cpp,
    Other,
}

impl Language {
    /// 用作 `[lsp.servers]` 覆盖和日志行中键的稳定小写字符串。
    #[must_use]
    pub fn as_key(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Go => "go",
            Language::Python => "python",
            Language::TypeScript => "typescript",
            Language::JavaScript => "javascript",
            Language::Java => "java",
            Language::Php => "php",
            Language::Vue => "vue",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Other => "other",
        }
    }

    /// 在 `textDocument/didOpen` 中使用的 LSP `languageId` 值。我们遵循
    /// LSP 规范值：`rust`、`go`、`python`、`typescript`、`javascript`、
    /// `java`、`vue`、`c`、`cpp`。
    #[must_use]
    pub fn language_id(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Go => "go",
            Language::Python => "python",
            Language::TypeScript => "typescript",
            Language::JavaScript => "javascript",
            Language::Java => "java",
            Language::Php => "php",
            Language::Vue => "vue",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Other => "plaintext",
        }
    }
}

/// 通过扩展名检测 `path` 的语言。当扩展名未知（或文件无扩展名）时
/// 回退到 `Language::Other`，向管理器发出"跳过"信号。
#[must_use]
pub fn detect_language(path: &Path) -> Language {
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => ext.to_ascii_lowercase(),
        None => return Language::Other,
    };
    match ext.as_str() {
        "rs" => Language::Rust,
        "go" => Language::Go,
        "py" | "pyi" => Language::Python,
        "ts" | "tsx" => Language::TypeScript,
        "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
        "java" => Language::Java,
        "php" => Language::Php,
        "vue" => Language::Vue,
        "c" | "h" => Language::C,
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Language::Cpp,
        _ => Language::Other,
    }
}

/// "为 `lang` 运行什么可执行文件 + 参数"的固定默认值。
/// 当该语言没有连接 LSP 服务器时返回 `None`。TUI 配置层可以在运行时覆盖此字典。
#[must_use]
pub fn server_for(lang: Language) -> Option<(&'static str, &'static [&'static str])> {
    match lang {
        Language::Rust => Some(("rust-analyzer", &[])),
        Language::Go => Some(("gopls", &["serve"])),
        Language::Python => Some(("pyright-langserver", &["--stdio"])),
        Language::TypeScript | Language::JavaScript => {
            Some(("typescript-language-server", &["--stdio"]))
        }
        Language::Java => Some(("jdtls", &[])),
        Language::Php => Some(("intelephense", &["--stdio"])),
        Language::Vue => Some(("vue-language-server", &["--stdio"])),
        Language::C | Language::Cpp => Some(("clangd", &[])),
        Language::Other => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_rust_extension() {
        assert_eq!(detect_language(&PathBuf::from("foo.rs")), Language::Rust);
        assert_eq!(detect_language(&PathBuf::from("FOO.RS")), Language::Rust);
    }

    #[test]
    fn detects_unknown_as_other() {
        assert_eq!(
            detect_language(&PathBuf::from("notes.txt")),
            Language::Other
        );
        assert_eq!(detect_language(&PathBuf::from("README")), Language::Other);
    }

    #[test]
    fn detects_typescript_variants() {
        assert_eq!(
            detect_language(&PathBuf::from("foo.ts")),
            Language::TypeScript
        );
        assert_eq!(
            detect_language(&PathBuf::from("foo.tsx")),
            Language::TypeScript
        );
        assert_eq!(
            detect_language(&PathBuf::from("foo.js")),
            Language::JavaScript
        );
    }

    #[test]
    fn detects_java_extension() {
        assert_eq!(detect_language(&PathBuf::from("App.java")), Language::Java);
        assert_eq!(detect_language(&PathBuf::from("APP.JAVA")), Language::Java);
    }

    #[test]
    fn detects_php_extension() {
        assert_eq!(detect_language(&PathBuf::from("index.php")), Language::Php);
        assert_eq!(detect_language(&PathBuf::from("INDEX.PHP")), Language::Php);
        assert_eq!(detect_language(&PathBuf::from("router.php")), Language::Php);
    }

    #[test]
    fn detects_vue_extension() {
        assert_eq!(
            detect_language(&PathBuf::from("Component.vue")),
            Language::Vue
        );
        assert_eq!(
            detect_language(&PathBuf::from("COMPONENT.VUE")),
            Language::Vue
        );
    }

    #[test]
    fn language_ids_for_php_and_vue_match_lsp_values() {
        assert_eq!(Language::Php.as_key(), "php");
        assert_eq!(Language::Php.language_id(), "php");
        assert_eq!(Language::Vue.as_key(), "vue");
        assert_eq!(Language::Vue.language_id(), "vue");
    }

    #[test]
    fn server_for_php_is_intelephense() {
        let (cmd, args) = server_for(Language::Php).expect("php has a server");
        assert_eq!(cmd, "intelephense");
        assert_eq!(args, &["--stdio"]);
    }

    #[test]
    fn language_ids_for_java_and_vue_match_lsp_values() {
        assert_eq!(Language::Java.as_key(), "java");
        assert_eq!(Language::Java.language_id(), "java");
        assert_eq!(Language::Vue.as_key(), "vue");
        assert_eq!(Language::Vue.language_id(), "vue");
    }

    #[test]
    fn server_for_rust_is_rust_analyzer() {
        let (cmd, args) = server_for(Language::Rust).expect("rust has a server");
        assert_eq!(cmd, "rust-analyzer");
        assert!(args.is_empty());
    }

    #[test]
    fn server_for_java_is_jdtls() {
        let (cmd, args) = server_for(Language::Java).expect("java has a server");
        assert_eq!(cmd, "jdtls");
        assert!(args.is_empty());
    }

    #[test]
    fn server_for_vue_is_vue_language_server() {
        let (cmd, args) = server_for(Language::Vue).expect("vue has a server");
        assert_eq!(cmd, "vue-language-server");
        assert_eq!(args, &["--stdio"]);
    }

    #[test]
    fn server_for_other_is_none() {
        assert!(server_for(Language::Other).is_none());
    }
}
