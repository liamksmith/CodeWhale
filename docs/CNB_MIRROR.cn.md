# CNB Cool 镜像

`cnb.cool/codewhale.net/codewhale` 是本 GitHub 仓库的单向镜像，供 GitHub 访问缓慢或被屏蔽的网络用户使用（主要面向中国大陆）。镜像接收对 `main` 的每次推送、用于第一方发布工作的每个 `fix/*`、`rebrand/*` 和 `work/v*` 分支，以及每个 `v*` 发布标签。

## 来源

**GitHub 是唯一的规范来源。** 所有发布、标签和源代码起源于 `github.com/Hmbown/CodeWhale`。CNB 镜像是由 `Sync to CNB` 工作流维护的只读副本 — 它仅用于服务 GFW 屏蔽或 GitHub 连接缓慢的用户。

每个 CNB 发布包含 `codewhale-artifacts-sha256.txt` — CNB 构建的 Linux x64 二进制文件的 SHA256 清单，从 GitHub 上标记的相同源码提交生成。（CNB 从源码构建，因此这些校验和覆盖 CNB 构建的产物，而非 GitHub 的发布资产。）用以下方式验证下载的二进制文件：

```bash
# 对照 CNB 清单验证下载的 CNB 二进制文件
sha256sum -c codewhale-artifacts-sha256.txt
```

## 工作原理

镜像由 [`Sync to CNB`](../.github/workflows/sync-cnb.yml) GitHub Actions 工作流维护：

- **触发条件：** `push` 到 `main`、任何 `v*` 标签的 `push`、匹配 `work/v*` 的发布工作分支、匹配 `fix/*` 和 `rebrand/*` 的第一方修复和品牌重塑分支，或用于手动恢复的 `workflow_dispatch`。
- **认证：** HTTPS 基本认证，用户 `cnb`，密码为 `CNB_GIT_TOKEN` 仓库密钥。
- **范围：** 仅推送触发运行的 ref。标签推送精确推送该标签。分支推送镜像 `main`、第一方 `fix/*`/`rebrand/*` 分支或显式匹配的发布分支。其他功能分支和 dependabot ref 故意*不*镜像。
- **并发：** 运行通过 `cnb-sync` 并发组序列化，因此 `auto-tag.yml` 的连续 `main` 推送和标签推送不会相互竞争。
- **重试：** 每次推送最多重试三次，采用线性退避（5s、10s），之后工作流放弃。

CNB 流水线配置也在 GitHub 中受源码控制，位于 [`/.cnb.yml`](../.cnb.yml)。这是有意为之：同步工作流强制将 GitHub ref 镜像到 CNB，因此仅在 CNB 端创建的流水线文件将被覆盖。通过 GitHub PR 提交 `.cnb.yml` 更改，让单向镜像将其传送到 CNB。

## CNB 标签发布

当 CNB 收到 `v*` 标签时，根 `.cnb.yml` 标签流水线从源码构建 Linux x64 发布资产，并发布一个 CNB 发布，包含：

- `codewhale-linux-x64`
- `codewhale-tui-linux-x64`
- `codewhale-artifacts-sha256.txt`

这为能访问 CNB 但无法访问 GitHub 的用户提供了 CNB 原生的发布路径。GitHub 仍然是规范的 macOS/Windows 发布矩阵；CNB 标签流水线是面向中国的 Linux x64 回退方案。

## CNB Linux CI 和发布预检

第一方 `fix/*` 和 `rebrand/*` 分支被镜像到 CNB，以便重量级 Linux Rust 门禁在腾讯托管的 runner 上运行，而非 GitHub Actions：

- `./scripts/release/check-versions.sh`
- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets --locked`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo test --workspace --all-features --locked`
- `cargo build --release --locked -p codewhale-cli -p codewhale-tui`
- `node scripts/release/npm-wrapper-smoke.js`

匹配 `work/v*` 的发布分支还会运行 `./scripts/release/publish-crates.sh dry-run`。GitHub Actions 保留轻量的 drift/fmt 状态以及 CNB 无法替代的 macOS 和 Windows 作业。

## 发布后验证镜像

在 `release.yml` 对 `vX.Y.Z` 标签完成后，CNB 镜像应同时具有 `main` 上的新提交和新标签：

```bash
# 快速检查：新标签在 CNB 上存在吗？
git ls-remote https://cnb.cool/codewhale.net/codewhale.git \
    refs/tags/vX.Y.Z

# 快速检查：CNB 的 main 是否与 origin/main 在同一提交？
gh_main=$(git ls-remote https://github.com/Hmbown/CodeWhale.git refs/heads/main | awk '{print $1}')
cnb_main=$(git ls-remote https://cnb.cool/codewhale.net/codewhale.git refs/heads/main | awk '{print $1}')
test "$gh_main" = "$cnb_main" && echo "已同步" || echo "分歧: gh=$gh_main cnb=$cnb_main"
```

或直接检查工作流运行：

```bash
gh run list --workflow=sync-cnb.yml --repo Hmbown/CodeWhale --limit 5
```

如果发布标签的最近一次运行是 `success`，则镜像已捕获。如果是 `failure`，在引导用户使用镜像标签之前修复或重新运行镜像工作流。

## 手动回退

手动镜像修复仅限维护者。不要在远程 URL 中放置 PAT，也不要在面向贡献者的文档中发布强制推送配方。尽可能使用配置的 GitHub Actions 密钥和工作流调度路径。

### 手动重新触发工作流

如果工作流健康但恰好在发布运行时失败（例如临时 CNB 中断已恢复），无需推送任何内容即可重新触发：

```bash
gh workflow run sync-cnb.yml --repo Hmbown/CodeWhale
```

`workflow_dispatch` 针对工作流的默认分支（`main`）运行，因此这将把当前 `main` 同步到 CNB。要重新同步特定标签，使用上面的手动 `git push cnb` 路径。

## 轮换 `CNB_GIT_TOKEN`

如果工作流开始因认证错误失败且令牌已过期：

1. 登录 `cnb.cool` 并生成具有 `repo`（推送）范围的新个人访问令牌。
2. 更新 `CNB_GIT_TOKEN` 仓库密钥：
   ```bash
   gh secret set CNB_GIT_TOKEN --repo Hmbown/CodeWhale
   ```
3. 在最近的提交上重新触发工作流：
   ```bash
   gh workflow run sync-cnb.yml --repo Hmbown/CodeWhale
   ```
4. 通过 `gh run list --workflow=sync-cnb.yml` 确认运行成功。

## 二进制发布资产和 `codewhale update`

CNB 现在从受源码控制的 `.cnb.yml` 流水线为 `v*` 标签构建 Linux x64 资产。GitHub 仍然是规范的 macOS/Windows 发布矩阵。在 GitHub 被屏蔽的网络后的用户应使用以下路径之一：

- 从 CNB 镜像进行 **`cargo install`**：
  ```bash
  cargo install --git https://cnb.cool/codewhale.net/codewhale --tag vX.Y.Z codewhale-cli
  cargo install --git https://cnb.cool/codewhale.net/codewhale --tag vX.Y.Z codewhale-tui
  ```
  （两个二进制文件都是必需的 — 调度器和 TUI 分开发布；有关双二进制安装原理，参见 `AGENTS.md`。）
  需要 Linux 构建时依赖（Debian/Ubuntu 上的 `build-essential`、`pkg-config`、`libdbus-1-dev`）— 参见 [INSTALL.md](INSTALL.md#4-install-via-cargo-any-tier-1-rust-target)。

- **CNB 发布资产** 用于 Linux x64，当匹配的 CNB 标签流水线成功完成时。从 `vX.Y.Z` 的 CNB 发布下载 `codewhale-linux-x64`、`codewhale-tui-linux-x64` 和 `codewhale-artifacts-sha256.txt`，然后对照清单验证二进制文件。

- **`DEEPSEEK_TUI_RELEASE_BASE_URL`** 环境变量，如果存在发布资产的 CDN 镜像。npm 包装器安装程序和 `codewhale update` 读取此变量以重定向二进制下载。对于 `codewhale update`，还需设置 `DEEPSEEK_TUI_VERSION=X.Y.Z`，以便更新器可以在不联系 GitHub 的情况下标记镜像发布。所指向的目录必须包含 `codewhale-artifacts-sha256.txt` 和平台二进制文件；格式与 GitHub Release 资产目录匹配。

## 从 CNB 克隆

对于稳定安装，从以下地址克隆 `main` 或发布标签：

```bash
https://cnb.cool/codewhale.net/codewhale.git
```

镜像接收 `main`、发布标签和匹配的发布分支。当 CNB 工作流或凭据不健康时，GitHub 是回退方案。

CNB 部署按钮示例位于 `deploy/tencent-lighthouse/cnb/`。它们在复制到 `.cnb.yml` 和 `.cnb/tag_deploy.yml` 之前不会激活，因为实时部署作业需要 Lighthouse 部署密钥、目标主机和显式的 CNB 配额/计费策略。
