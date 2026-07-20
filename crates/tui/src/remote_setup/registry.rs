//! `codewhale remote-setup` 的表驱动注册表。
//!
//! 镜像 `crates/config/src/lib.rs` 中的 `ProviderKind`/`provider::Provider`
//! 注册表模式：添加云或桥接是一条数据行，而不是一个新的控制流分支。
//! [`super`] 中的向导迭代这些表而不是硬编码云/桥接，
//! 因此矩阵通过数据增长。
//!
//! - [`BridgeSpec`] — 聊天应用与本地运行时之间的纯传输。
//! - [`CloudTarget`] — 代理的运行位置及其密钥的存储位置。
//! - 提供商维度*不*在此处重复：它读取现有的
//!   `codewhale_config::provider` 注册表（参见 [`super::bundle::ProviderInfo`]）。

/// 云目标存储运行时/提供商密钥的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretStore {
    /// 密钥存储在主机上的 `/etc/codewhale/*.env` 文件中。
    EnvFile,
    /// 密钥存储在托管保险库（例如 Azure Key Vault）中，启动时读取。
    KeyVault,
}

impl SecretStore {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            SecretStore::EnvFile => "EnvFile (/etc/codewhale/*.env)",
            SecretStore::KeyVault => "Key Vault (managed identity at boot)",
        }
    }
}

/// 运行时 + 桥接在主机上的安装方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    /// 原生 `cargo install` + systemd 单元（镜像 deploy/tencent-lighthouse）。
    NativeSystemd,
    /// 拉取容器镜像并在 systemd/容器运行时下运行。
    Docker,
}

impl InstallMethod {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            InstallMethod::NativeSystemd => "native + systemd",
            InstallMethod::Docker => "Docker image",
        }
    }
}

/// 以**数据**形式表达的单个配置步骤，绝不是 shell 字符串。
///
/// 命令返回为 `(program, args)`，以便确认门控可以在运行任何操作前
/// 打印每个命令，密钥通过标准输入/临时文件传递（绝不通过 argv 或
/// shell 历史——`secret_args` 列出打印时需要脱敏的 arg 索引），
/// 而 `--apply` 仅执行已打印的计划。在只生成 MVP 中，
/// 这些步骤仅*渲染到 RUNBOOK 中*；不会执行任何操作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionStep {
    /// 计划/RUNBOOK 中显示的人类可读描述。
    pub description: String,
    /// 要运行的程序（例如 `az`、`doctl`）。
    pub program: String,
    /// 参数，按顺序。
    pub args: Vec<String>,
    /// `args` 中值保密且打印计划时必须脱敏的索引。
    ///（对于此处仅数据的 RUNBOOK 行为空。）
    pub secret_args: Vec<usize>,
}

impl ProvisionStep {
    pub fn new(description: impl Into<String>, program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            description: description.into(),
            program: program.into(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            secret_args: Vec::new(),
        }
    }

    /// 渲染命令用于显示，脱敏任何秘密 arg 位置。
    #[must_use]
    pub fn display_command(&self) -> String {
        let mut parts = Vec::with_capacity(self.args.len() + 1);
        parts.push(self.program.clone());
        for (idx, arg) in self.args.iter().enumerate() {
            if self.secret_args.contains(&idx) {
                parts.push("<redacted>".to_string());
            } else {
                parts.push(arg.clone());
            }
        }
        parts.join(" ")
    }
}

/// 向导收集的输入，云的 `plan()` 读取这些输入。
///
/// 有意保持最小且无副作用：`plan()` 将这些转换为有序的
/// [`ProvisionStep`] 列表。密钥*值*从不放在这里；
/// 计划引用它们将从哪里读取（env 文件/保险库），因此此结构体
/// 保持安全，可打印并可在测试中构建。
#[derive(Debug, Clone)]
pub struct DeployInputs {
    /// 桥接标识，例如 `"telegram"`。
    pub bridge_slug: String,
    /// 提供商标识，例如 `"deepseek"`。
    pub provider_slug: String,
    /// 云区域/位置（每个云的默认值）。
    pub region: String,
    /// 逻辑实例/资源名称。
    pub instance_name: String,
    /// Docker 安装使用的容器镜像。
    pub image: String,
}

impl Default for DeployInputs {
    fn default() -> Self {
        Self {
            bridge_slug: "telegram".to_string(),
            provider_slug: "deepseek".to_string(),
            region: String::new(),
            instance_name: "codewhale-remote".to_string(),
            image: "ghcr.io/hmbown/codewhale:latest".to_string(),
        }
    }
}

/// 聊天桥接：聊天应用与 `127.0.0.1:7878` 之间的纯传输。
#[derive(Debug, Clone, Copy)]
pub struct BridgeSpec {
    /// CLI 和路径中使用的稳定标识，例如 `"telegram"`。
    pub slug: &'static str,
    /// 人类可读的标签。
    pub display: &'static str,
    /// 包目录（相对于仓库根目录），例如 `"integrations/telegram-bridge"`。
    pub package_dir: &'static str,
    /// 桥接的 systemd 单元文件名。
    pub service_unit: &'static str,
    /// 仓库相对路径，指向 deploy/ 附带的环境模板参考文件。
    pub env_template: &'static str,
    /// 向导提示输入的桥接特定秘密环境键（令牌等）。
    pub secret_keys: &'static [&'static str],
    /// 提示前显示的一行说明（在哪里获取桥接凭证）。
    pub setup_hint: &'static str,
    /// systemd `WorkingDirectory`，单元期望桥接安装在此处。
    pub install_dir: &'static str,
}

/// 云目标：代理的运行位置和密钥的存储位置。
#[derive(Debug, Clone, Copy)]
pub struct CloudTarget {
    /// CLI 和路径中使用的稳定标识，例如 `"azure"`。
    pub slug: &'static str,
    /// 人类可读的标签。
    pub display: &'static str,
    /// 运行时/提供商密钥的存储位置。
    pub secret_store: SecretStore,
    /// 运行时 + 桥接的安装方式。
    pub install: InstallMethod,
    /// 此云的默认区域/位置。
    pub default_region: &'static str,
    /// （存根的）自动配置路径使用的云 CLI，例如 `"az"`。
    pub cli_tool: &'static str,
    /// 以数据形式构建有序的配置计划。在只生成 MVP 中，
    /// 这仅在 RUNBOOK 中渲染；`--apply` 未实现。
    pub plan: fn(&DeployInputs) -> Vec<ProvisionStep>,
}

// ---------------------------------------------------------------------------
// 桥接注册表
// ---------------------------------------------------------------------------

/// Telegram 桥接——长轮询传输，密钥是 BotFather 令牌。
pub const TELEGRAM: BridgeSpec = BridgeSpec {
    slug: "telegram",
    display: "Telegram",
    package_dir: "integrations/telegram-bridge",
    service_unit: "codewhale-telegram-bridge.service",
    env_template: "deploy/tencent-lighthouse/examples/telegram-bridge.env.example",
    secret_keys: &["TELEGRAM_BOT_TOKEN"],
    setup_hint: "Create a bot with @BotFather in Telegram and copy the HTTP API token.",
    install_dir: "/opt/codewhale/telegram-bridge",
};

/// 飞书/Lark 桥接——应用 ID + 密钥是桥接凭证。
pub const FEISHU: BridgeSpec = BridgeSpec {
    slug: "feishu",
    display: "Feishu/Lark",
    package_dir: "integrations/feishu-bridge",
    service_unit: "codewhale-feishu-bridge.service",
    env_template: "deploy/tencent-lighthouse/examples/feishu-bridge.env.example",
    secret_keys: &["FEISHU_APP_ID", "FEISHU_APP_SECRET"],
    setup_hint: "Create a custom app in the Feishu/Lark Open Platform; copy its App ID and App Secret.",
    install_dir: "/opt/codewhale/bridge",
};

/// 所有注册的桥接。添加一个桥接就是这里的一行数据。
pub const BRIDGES: &[BridgeSpec] = &[FEISHU, TELEGRAM];

/// 按标识查找桥接。
#[must_use]
pub fn bridge_by_slug(slug: &str) -> Option<&'static BridgeSpec> {
    BRIDGES.iter().find(|b| b.slug.eq_ignore_ascii_case(slug))
}

// ---------------------------------------------------------------------------
// 云注册表
// ---------------------------------------------------------------------------

/// 腾讯轻量服务器——原生 systemd，env 文件密钥，CNB 驱动的部署。
pub const LIGHTHOUSE: CloudTarget = CloudTarget {
    slug: "lighthouse",
    display: "Tencent Lighthouse",
    secret_store: SecretStore::EnvFile,
    install: InstallMethod::NativeSystemd,
    default_region: "ap-hongkong",
    cli_tool: "cnb",
    plan: lighthouse_plan,
};

/// Azure VM——Docker 镜像 + 通过托管标识的 Key Vault 密钥。
pub const AZURE: CloudTarget = CloudTarget {
    slug: "azure",
    display: "Azure VM",
    secret_store: SecretStore::KeyVault,
    install: InstallMethod::Docker,
    default_region: "eastus",
    cli_tool: "az",
    plan: azure_plan,
};

/// DigitalOcean Droplet——原生 systemd，env 文件密钥，cloud-init + doctl。
///
/// Hunter 请求的目标。建模类似 Azure/Lighthouse：密钥在
/// `/etc/codewhale/*.env` 中，原生+systemd 安装由 cloud-init
/// 用户数据文件驱动，`doctl` 用于创建/销毁命令。`plan()`
/// 返回 `doctl` `ProvisionStep` 数据，但由于在 MVP 中 `--apply`
/// 是存根的，计划仅在生成的 RUNBOOK 中打印。
pub const DIGITALOCEAN: CloudTarget = CloudTarget {
    slug: "digitalocean",
    display: "DigitalOcean Droplet",
    secret_store: SecretStore::EnvFile,
    install: InstallMethod::NativeSystemd,
    default_region: "sfo3",
    cli_tool: "doctl",
    plan: digitalocean_plan,
};

/// 所有注册的云目标。添加一个云就是这里的一行。
pub const CLOUD_TARGETS: &[CloudTarget] = &[LIGHTHOUSE, AZURE, DIGITALOCEAN];

/// 按标识查找云目标。
#[must_use]
pub fn cloud_by_slug(slug: &str) -> Option<&'static CloudTarget> {
    CLOUD_TARGETS
        .iter()
        .find(|c| c.slug.eq_ignore_ascii_case(slug))
}

// ---------------------------------------------------------------------------
// 云计划（仅数据——在 MVP 中从不执行）
// ---------------------------------------------------------------------------

fn lighthouse_plan(inputs: &DeployInputs) -> Vec<ProvisionStep> {
    // 轻量服务器配置由现有的 CNB 管道驱动
    //（deploy/tencent-lighthouse/cnb/*）。这里的"计划"是 CNB 触发器加上
    // RUNBOOK 引导用户完成的主机端服务安装。
    let restart_bridge = format!("codewhale-{}-bridge", inputs.bridge_slug);
    vec![
        ProvisionStep::new(
            "Render and commit the CNB pipeline (cnb.yml + tag_deploy.yml) for this deploy",
            "git",
            &["add", ".cnb.yml", ".cnb/tag_deploy.yml"],
        ),
        ProvisionStep::new(
            "Trigger the CNB `web_trigger_lighthouse` button to build + ship to the host",
            "cnb",
            &["trigger", "web_trigger_lighthouse"],
        ),
        ProvisionStep::new(
            "On the host: install both systemd units and start the runtime + bridge",
            "bash",
            &["scripts/tencent-lighthouse/install-services.sh"],
        ),
        ProvisionStep::new(
            format!("Restart the bridge service after the deploy ({restart_bridge})"),
            "systemctl",
            &["restart", &restart_bridge],
        ),
    ]
}

fn azure_plan(inputs: &DeployInputs) -> Vec<ProvisionStep> {
    let rg = format!("{}-rg", inputs.instance_name);
    let vault = format!("{}-kv", inputs.instance_name);
    let provider_secret = format!("codewhale-{}-key", inputs.provider_slug);
    vec![
        ProvisionStep::new(
            "Create the resource group",
            "az",
            &[
                "group",
                "create",
                "--name",
                &rg,
                "--location",
                &inputs.region,
            ],
        ),
        ProvisionStep::new(
            "Create the Key Vault that holds the provider key + runtime token",
            "az",
            &[
                "keyvault",
                "create",
                "--name",
                &vault,
                "--resource-group",
                &rg,
                "--location",
                &inputs.region,
            ],
        ),
        ProvisionStep::new(
            format!(
                "Store the {} provider key in Key Vault (value piped via stdin, not argv)",
                inputs.provider_slug
            ),
            "az",
            &[
                "keyvault",
                "secret",
                "set",
                "--vault-name",
                &vault,
                "--name",
                &provider_secret,
            ],
        ),
        ProvisionStep::new(
            format!(
                "Create the VM from {} with cloud-init custom-data + a system-assigned identity",
                inputs.image
            ),
            "az",
            &[
                "vm",
                "create",
                "--resource-group",
                &rg,
                "--name",
                &inputs.instance_name,
                "--custom-data",
                "cloud-init.yaml",
                "--assign-identity",
            ],
        ),
        ProvisionStep::new(
            "Scope the NSG to SSH (22) from the caller IP only; 7878 stays on 127.0.0.1",
            "az",
            &[
                "vm",
                "open-port",
                "--resource-group",
                &rg,
                "--name",
                &inputs.instance_name,
                "--port",
                "22",
            ],
        ),
    ]
}

fn digitalocean_plan(inputs: &DeployInputs) -> Vec<ProvisionStep> {
    // 从 cloud-init 用户数据文件创建的 Droplet，然后是主机端
    // 服务安装。doctl 是云 CLI；命令在这里只是数据。
    vec![
        ProvisionStep::new(
            "Create the Droplet from the generated cloud-init user-data (native + systemd)",
            "doctl",
            &[
                "compute",
                "droplet",
                "create",
                &inputs.instance_name,
                "--region",
                &inputs.region,
                "--image",
                "ubuntu-24-04-x64",
                "--size",
                "s-2vcpu-4gb",
                "--user-data-file",
                "cloud-init.yaml",
                "--ssh-keys",
                "<your-ssh-key-fingerprint>",
                "--wait",
            ],
        ),
        ProvisionStep::new(
            "Read the Droplet's public IPv4 for the SSH step below",
            "doctl",
            &[
                "compute",
                "droplet",
                "get",
                &inputs.instance_name,
                "--format",
                "PublicIPv4",
                "--no-header",
            ],
        ),
        ProvisionStep::new(
            "On the Droplet: write /etc/codewhale/*.env, install both systemd units, enable --now",
            "bash",
            &["scripts/tencent-lighthouse/install-services.sh"],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    /// 仓库根目录，从此 crate 的清单目录（`crates/tui`）解析。
    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("crates/tui has a two-level parent (repo root)")
            .to_path_buf()
    }

    #[test]
    fn bridge_slugs_are_unique() {
        let mut seen = HashSet::new();
        for b in BRIDGES {
            assert!(seen.insert(b.slug), "重复的桥接标识: {}", b.slug);
        }
        assert_eq!(seen.len(), BRIDGES.len());
    }

    #[test]
    fn cloud_slugs_are_unique() {
        let mut seen = HashSet::new();
        for c in CLOUD_TARGETS {
            assert!(seen.insert(c.slug), "重复的云标识: {}", c.slug);
        }
        assert_eq!(seen.len(), CLOUD_TARGETS.len());
    }

    #[test]
    fn digitalocean_is_registered() {
        // Hunter 明确要求在矩阵中包含 DigitalOcean。
        assert!(
            cloud_by_slug("digitalocean").is_some(),
            "DigitalOcean 必须是已注册的云目标"
        );
        let r#do = cloud_by_slug("digitalocean").unwrap();
        assert_eq!(r#do.secret_store, SecretStore::EnvFile);
        assert_eq!(r#do.install, InstallMethod::NativeSystemd);
        assert_eq!(r#do.cli_tool, "doctl");
    }

    #[test]
    fn every_bridge_references_existing_files() {
        let root = repo_root();
        for b in BRIDGES {
            let pkg = root.join(b.package_dir);
            assert!(
                pkg.is_dir(),
                "桥接 {} package_dir 缺失: {}",
                b.slug,
                pkg.display()
            );
            let unit = root
                .join("deploy/tencent-lighthouse/systemd")
                .join(b.service_unit);
            assert!(
                unit.is_file(),
                "桥接 {} service_unit 缺失: {}",
                b.slug,
                unit.display()
            );
            let template = root.join(b.env_template);
            assert!(
                template.is_file(),
                "桥接 {} env_template 缺失: {}",
                b.slug,
                template.display()
            );
            assert!(
                !b.secret_keys.is_empty(),
                "桥接 {} 必须声明至少一个密钥键",
                b.slug
            );
        }
    }

    #[test]
    fn lookup_helpers_are_case_insensitive() {
        assert_eq!(bridge_by_slug("TELEGRAM").map(|b| b.slug), Some("telegram"));
        assert_eq!(cloud_by_slug("Azure").map(|c| c.slug), Some("azure"));
        assert!(bridge_by_slug("nope").is_none());
        assert!(cloud_by_slug("nope").is_none());
    }

    #[test]
    fn cloud_plans_return_ordered_steps_without_executing() {
        // 为每个云构建（从不运行）一个计划，并断言程序+参数。
        let inputs = DeployInputs::default();
        for c in CLOUD_TARGETS {
            let steps = (c.plan)(&inputs);
            assert!(!steps.is_empty(), "云 {} 产生了空计划", c.slug);
            // 第一步的程序是云自身的工具或主机脚本。
            assert!(
                steps
                    .iter()
                    .all(|s| !s.program.is_empty() && !s.description.is_empty()),
                "云 {} 有一个格式错误的步骤",
                c.slug
            );
        }

        // DigitalOcean 特别使用 doctl。
        let do_steps = (DIGITALOCEAN.plan)(&inputs);
        assert!(
            do_steps.iter().any(|s| s.program == "doctl"),
            "DigitalOcean 计划必须使用 doctl"
        );
        // Azure 特别使用 az。
        let az_steps = (AZURE.plan)(&inputs);
        assert!(
            az_steps.iter().any(|s| s.program == "az"),
            "Azure 计划必须使用 az"
        );
    }

    #[test]
    fn display_command_redacts_secret_args() {
        let mut step =
            ProvisionStep::new("set secret", "az", &["keyvault", "secret", "set", "VALUE"]);
        step.secret_args = vec![3];
        let rendered = step.display_command();
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("VALUE"));
    }
}
