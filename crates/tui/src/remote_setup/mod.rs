//! `codewhale remote-setup` —— 引导式远程代理部署包生成。
//!
//! 仅生成型 MVP：向导收集云目标、聊天桥接和模型提供商，
//! 然后将部署包（环境文件、systemd 单元、RUNBOOK）渲染到 `--out`。
//! `--apply` 云 CLI 自动配置路径已打桩（"尚未实现"）——绝不执行任何操作。
//!
//! 设计与 `crates/config/src/lib.rs` 中的表驱动提供商注册表一致：
//! 向导遍历 [`registry::CLOUD_TARGETS`]、[`registry::BRIDGES`]
//! 以及现有的 `codewhale_config::provider` 注册表，而不是硬编码矩阵。

pub mod bundle;
pub mod registry;

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Args;

use bundle::{BundleInputs, DEFAULT_PORT, DEFAULT_WORKERS, ProviderInfo, write_bundle};
use registry::{BRIDGES, BridgeSpec, CLOUD_TARGETS, CloudTarget};

/// `codewhale remote-setup` 的 clap 标志，符合 RFC 命令表面。
#[derive(Args, Debug, Clone, Default)]
pub struct RemoteSetupArgs {
    /// 云目标 slug（lighthouse, azure, digitalocean）。跳过提示。
    #[arg(long)]
    pub cloud: Option<String>,
    /// 聊天桥接 slug（feishu, telegram）。跳过提示。
    #[arg(long)]
    pub bridge: Option<String>,
    /// 提供商 slug；根据提供商注册表验证。跳过提示。
    #[arg(long)]
    pub provider: Option<String>,
    /// 包输出目录（默认为 `./codewhale-deploy/<cloud>-<bridge>`）。
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// 生成包，不执行配置（默认）。
    #[arg(long, default_value_t = false)]
    pub generate_only: bool,
    /// 运行云 CLI 以自动配置（MVP：尚未实现）。
    #[arg(long, default_value_t = false, conflicts_with = "generate_only")]
    pub apply: bool,
    /// 跳过最终确认门控（CI / 非交互式）。
    #[arg(long, default_value_t = false)]
    pub yes: bool,
    /// 如果缺少任何必需值则失败而不提示。
    #[arg(long, default_value_t = false)]
    pub non_interactive: bool,
}

/// 由 TUI 命令调度器调用的入口点。
pub fn run_remote_setup(args: RemoteSetupArgs) -> Result<()> {
    print_header();

    let cloud = resolve_cloud(&args)?;
    let bridge = resolve_bridge(&args)?;
    let provider = resolve_provider(&args)?;

    println!();
    println!("Plan:");
    println!("  cloud    : {} ({})", cloud.display, cloud.slug);
    println!("  bridge   : {} ({})", bridge.display, bridge.slug);
    println!(
        "  provider : {} ({}) — key var {}",
        provider.display, provider.slug, provider.key_var
    );
    println!("  hint     : {}", bridge.setup_hint);

    // 使用代码库已建立的 CSPRNG 模式（uuid v4，如同 acp_server.rs）
    // 生成共享运行时令牌——绝不使用 Math.random / 基于时间的随机数。
    let runtime_token = generate_runtime_token();

    let inputs = BundleInputs {
        cloud,
        bridge,
        provider: provider.clone(),
        model: "auto".to_string(),
        runtime_token,
        provider_key_value: format!("replace-with-{}-key", provider.slug),
        bridge_secret_values: bridge
            .secret_keys
            .iter()
            .map(|k| {
                (
                    (*k).to_string(),
                    format!("replace-with-{}", k.to_ascii_lowercase()),
                )
            })
            .collect(),
        allowlist: String::new(),
        port: DEFAULT_PORT,
        workers: DEFAULT_WORKERS,
        workspace: "/opt/whalebro".to_string(),
    };

    let out_dir = args.out.clone().unwrap_or_else(|| {
        PathBuf::from("codewhale-deploy").join(format!("{}-{}", cloud.slug, bridge.slug))
    });

    // 始终渲染包，即使请求了 --apply。
    let written = write_bundle(&inputs, &out_dir)?;
    println!();
    println!("Generated bundle in {}:", out_dir.display());
    for path in &written {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        println!("  - {name}");
    }

    if args.apply {
        // MVP：自动配置路径有意尚未实现。
        println!();
        println!("auto-provision not yet implemented; bundle generated, follow RUNBOOK.md");
    } else {
        println!();
        println!(
            "Next: open {}/RUNBOOK.md and follow the steps.",
            out_dir.display()
        );
    }

    Ok(())
}

fn print_header() {
    use crate::palette;
    use colored::Colorize;
    let (r, g, b) = palette::WHALE_INFO_RGB;
    println!("{}", "CodeWhale Remote Setup".truecolor(r, g, b).bold());
    println!("{}", "======================".truecolor(r, g, b));
    println!("Generate a deploy bundle for a remote CodeWhale agent (cloud + chat bridge).");
}

// ---------------------------------------------------------------------------
// 解析：标志 -> 提示（除非 --non-interactive） -> 验证后的值
// ---------------------------------------------------------------------------

fn resolve_cloud(args: &RemoteSetupArgs) -> Result<&'static CloudTarget> {
    if let Some(slug) = &args.cloud {
        return registry::cloud_by_slug(slug)
            .ok_or_else(|| anyhow::anyhow!("unknown cloud '{slug}'. {}", cloud_choices()));
    }
    if args.non_interactive {
        bail!(
            "--cloud is required in --non-interactive mode. {}",
            cloud_choices()
        );
    }
    let idx = prompt_choice(
        "Cloud target",
        &CLOUD_TARGETS
            .iter()
            .map(|c| format!("{} ({})", c.display, c.slug))
            .collect::<Vec<_>>(),
    )?;
    Ok(&CLOUD_TARGETS[idx])
}

fn resolve_bridge(args: &RemoteSetupArgs) -> Result<&'static BridgeSpec> {
    if let Some(slug) = &args.bridge {
        return registry::bridge_by_slug(slug)
            .ok_or_else(|| anyhow::anyhow!("unknown bridge '{slug}'. {}", bridge_choices()));
    }
    if args.non_interactive {
        bail!(
            "--bridge is required in --non-interactive mode. {}",
            bridge_choices()
        );
    }
    let idx = prompt_choice(
        "Chat bridge",
        &BRIDGES
            .iter()
            .map(|b| format!("{} ({})", b.display, b.slug))
            .collect::<Vec<_>>(),
    )?;
    Ok(&BRIDGES[idx])
}

fn resolve_provider(args: &RemoteSetupArgs) -> Result<ProviderInfo> {
    if let Some(slug) = &args.provider {
        return ProviderInfo::from_slug(slug).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown provider '{slug}'. Known: {}",
                codewhale_config::ProviderKind::names_hint()
            )
        });
    }
    if args.non_interactive {
        bail!(
            "--provider is required in --non-interactive mode. Known: {}",
            codewhale_config::ProviderKind::names_hint()
        );
    }
    // 从现有注册表中按其规范名称列出提供商。
    let providers: Vec<ProviderInfo> = codewhale_config::ProviderKind::all()
        .iter()
        .filter_map(|kind| ProviderInfo::from_slug(kind.as_str()))
        .collect();
    let labels: Vec<String> = providers
        .iter()
        .map(|p| format!("{} ({})", p.display, p.slug))
        .collect();
    let idx = prompt_choice("Model provider", &labels)?;
    Ok(providers[idx].clone())
}

fn cloud_choices() -> String {
    format!(
        "Choices: {}",
        CLOUD_TARGETS
            .iter()
            .map(|c| c.slug)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn bridge_choices() -> String {
    format!(
        "Choices: {}",
        BRIDGES
            .iter()
            .map(|b| b.slug)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

// ---------------------------------------------------------------------------
// 提示辅助函数（复用 main.rs 中 `pick_session_id` 的 stdin 模式）
// ---------------------------------------------------------------------------

/// 打印编号菜单，从 stdin 读取基于 1 的选择，返回索引。
fn prompt_choice(title: &str, options: &[String]) -> Result<usize> {
    println!();
    println!("{title}:");
    for (idx, opt) in options.iter().enumerate() {
        println!("  {:>2}. {}", idx + 1, opt);
    }
    print!("Enter a number: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    if input.is_empty() {
        bail!("No selection made.");
    }
    let n: usize = input
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid input: {input}"))?;
    options
        .get(n.saturating_sub(1))
        .map(|_| n - 1)
        .ok_or_else(|| anyhow::anyhow!("Selection out of range"))
}

/// 从两个随机 v4 UUID（通过 uuid 使用 OS CSPRNG）生成运行时令牌，
/// 匹配此 crate 中现有的令牌生成模式。
fn generate_runtime_token() -> String {
    let a = uuid::Uuid::new_v4().simple().to_string();
    let b = uuid::Uuid::new_v4().simple().to_string();
    format!("{a}{b}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_token_is_long_and_hex() {
        let t = generate_runtime_token();
        assert_eq!(t.len(), 64, "two simple uuids = 64 hex chars");
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        // 连续两次生成的令牌不同（随机，非固定）。
        assert_ne!(t, generate_runtime_token());
    }

    #[test]
    fn unknown_flags_fail_with_choices() {
        let args = RemoteSetupArgs {
            cloud: Some("nope".to_string()),
            non_interactive: true,
            ..Default::default()
        };
        let err = resolve_cloud(&args).unwrap_err().to_string();
        assert!(err.contains("unknown cloud"));
        assert!(err.contains("digitalocean"));

        let args = RemoteSetupArgs {
            bridge: Some("nope".to_string()),
            non_interactive: true,
            ..Default::default()
        };
        let err = resolve_bridge(&args).unwrap_err().to_string();
        assert!(err.contains("unknown bridge"));

        let args = RemoteSetupArgs {
            provider: Some("nope".to_string()),
            non_interactive: true,
            ..Default::default()
        };
        let err = resolve_provider(&args).unwrap_err().to_string();
        assert!(err.contains("unknown provider"));
    }

    #[test]
    fn non_interactive_requires_flags() {
        let args = RemoteSetupArgs {
            non_interactive: true,
            ..Default::default()
        };
        assert!(
            resolve_cloud(&args)
                .unwrap_err()
                .to_string()
                .contains("--cloud is required")
        );
    }

    #[test]
    fn flags_resolve_to_registry_rows() {
        let args = RemoteSetupArgs {
            cloud: Some("digitalocean".to_string()),
            bridge: Some("telegram".to_string()),
            provider: Some("deepseek".to_string()),
            non_interactive: true,
            ..Default::default()
        };
        assert_eq!(resolve_cloud(&args).unwrap().slug, "digitalocean");
        assert_eq!(resolve_bridge(&args).unwrap().slug, "telegram");
        assert_eq!(resolve_provider(&args).unwrap().slug, "deepseek");
    }
}
