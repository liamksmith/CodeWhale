# CodeWhale 发布操作手册

本操作手册是发布 Rust crate、GitHub 发布资产和 `codewhale` npm 包装器的权威指南。

当前打包说明：
- `codewhale-tui` 是当前交付给用户的正式运行时 crate。
- `codewhale-app-server` 是一个支持库 crate。交付的入口点是 `codewhale app-server`；不要添加或发布独立的 app-server 二进制文件。

## 规范发布目标

- 最终用户 crate：
  - `codewhale-tui`
  - `codewhale-cli`
- 从此工作区发布的支持 crate：
  - `codewhale-secrets`
  - `codewhale-config`
  - `codewhale-protocol`
  - `codewhale-state`
  - `codewhale-agent`
  - `codewhale-execpolicy`
  - `codewhale-hooks`
  - `codewhale-mcp`
  - `codewhale-tools`
  - `codewhale-core`
  - `codewhale-app-server`
  - `codewhale-workflow`

## 版本协调

- Rust crate 继承 [Cargo.toml](../Cargo.toml) 中的共享工作区版本。
- 内部路径依赖版本应与共享工作区版本匹配；当工作区版本变动后，过时的旧版本固定是发布阻塞项。
- npm 包装器版本位于 [npm/codewhale/package.json](../npm/codewhale/package.json)。
- `codewhaleBinaryVersion` 控制 npm 包装器下载哪个 GitHub 发布二进制文件。
- 允许仅打包的 npm 发布：
  - 升级 npm 包版本
  - 保持 `codewhaleBinaryVersion` 固定到先前发布的 Rust 二进制文件
  - 在 `npm publish` 之前重新运行 `npm pack` 冒烟检查

## 发布源时机

在创建公开的 `vX.Y.Z` 标签之前冻结源代码。版本升级不是发布本身；它是标签之前的最后一个源代码准备提交。不要在 `vX.Y.Z` 存在后继续合并相同版本的功能/修复 PR 并假设发布工作流会采纳它们。不会的：标签是发布锚点。

在打标签之前，验证实时队列和现有锚点：

```bash
gh issue list --repo Hmbown/CodeWhale --milestone "vX.Y.Z" --state open
gh pr list --repo Hmbown/CodeWhale --state open --limit 100
git ls-remote origin refs/heads/main refs/tags/vX.Y.Z
gh release view vX.Y.Z --repo Hmbown/CodeWhale
./scripts/release/check-published.sh X.Y.Z
```

如果已存在相同版本的标签但没有 GitHub Release 且没有任何发布内容，则停下来有意识地选择：

- 发布完全相同的已标记 SHA，将后续提交留给下一个补丁版本；
- 将后续工作升级到下一个补丁版本并标记后续 SHA；或
- 仅在获得明确的维护者批准后，在确认没有任何包、GitHub Release、镜像或安装器消费者将其视为公开后，删除/重新创建未发布的标签。

不要作为普通 PR 合并或里程碑清理工作的一部分隐式删除、移动或重新创建发布标签。

## 飞行前检查

在打标签之前，从仓库根目录运行以下命令：

```bash
./scripts/release/check-versions.sh   # 检查工作区、npm、lockfile 之间的版本差异
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo publish --dry-run --locked --allow-dirty -p codewhale-tui
./scripts/release/publish-crates.sh dry-run
```

`check-versions.sh` 也在每次推送/PR 的 CI 中运行（`.github/workflows/ci.yml` 中的 `versions` 作业），因此 `Cargo.toml`、各 crate 清单、`npm/codewhale/package.json` 和 `Cargo.lock` 之间的差异在发布之前就会被捕获，而不是在发布时才发现。

源代码控制的 CNB 流水线为 `fix/*`、`rebrand/*`、`work/v*` 和 `main` 分支镜像了重量级 Linux 版本/格式/检查/clippy/测试/npm 冒烟门禁。GitHub Actions 保持廉价的差异/格式状态以及 macOS 和 Windows 覆盖，而 CNB 承担 Linux 工作。

`publish-crates.sh dry-run` 对没有未发布工作区依赖的 crate 执行完整的 `cargo publish --dry-run`，并对依赖工作区 crate 执行打包飞行前检查。这避免了 crates.io 尚不包含新工作区版本时出现假阴性，同时在发布前验证包内容。

对于 npm 包装器验证，构建两个交付的二进制文件并运行跨平台冒烟测试。这将 npm 包装器打包，安装到干净的临时项目中，通过 HTTP 提供本地发布资产，并检查调度器到 TUI 路径（`codewhale doctor --help`）和直接 TUI 入口点（`codewhale-tui --help`）。

```bash
cargo build --release --locked -p codewhale-cli -p codewhale-tui
node scripts/release/npm-wrapper-smoke.js
```

设置 `DEEPSEEK_TUI_KEEP_SMOKE_DIR=1` 以保留临时打包/安装目录供检查。

要在本地也运行 `npm run release:check`，在启动服务器之前使用完整的资产矩阵夹具重新生成本地资产目录：

```bash
DEEPSEEK_TUI_PREPARE_ALL_ASSETS=1 node scripts/release/prepare-local-release-assets.js
cd npm/codewhale
DEEPSEEK_TUI_VERSION=X.Y.Z DEEPSEEK_TUI_RELEASE_BASE_URL=http://127.0.0.1:8123/ npm run release:check
```

将 `DEEPSEEK_TUI_VERSION` 设置为该本地运行要验证的 npm 包版本。

CNB 工作流运行 Linux tarball 安装 + 委托入口点冒烟测试；GitHub Actions 保持 macOS 和 Windows 冒烟覆盖。

发布后，证明发布在两个注册表中都可见：

```bash
./scripts/release/check-published.sh X.Y.Z
```

在该命令看到 npm 上的 `codewhale@X.Y.Z` 和 crates.io 上的每个 `codewhale-*` crate 都为 `X.Y.Z` 之前，不要标记 Rust 发布完成。对于罕见的仅 npm 打包发布，使用 `--allow-npm-binary-mismatch` 运行，并在发布说明中明确说明没有新的 Rust 二进制版本交付。

## 合并后分支清理

在发布或临时集成分支落地后，在修剪任何内容之前运行分支清理辅助脚本：

```bash
./scripts/release/branch-hygiene.sh --release-branch codex/vX.Y.Z
```

默认模式是试运行。它报告当前检出分支、main 引用、本地和远程发布提示、安全可删除的本地或远程分支、为贡献者工作保留的分支以及仍需要人工决策的分支。在运行 `--prune --yes` 之前查看该报告，仅在确认远程分支可以安全删除时才添加 `--prune-remote`。

当您从 fork 工作且规范发布引用位于上游远程而非 `origin` 时，使用 `--remote upstream`。

更改辅助脚本后验证其本身：

```bash
bash scripts/release/branch-hygiene.test.sh
bash scripts/release/ensure-release-on-main.test.sh
```

这些脚本固定为 LF 行尾，以便在 Windows 检出下通过 Bash 运行相同的命令。

## Rust Crate 发布

发布到 crates.io 的 **手动** 操作——没有自动化的 `crates-publish` GitHub 工作流。操作员从配置了 `cargo login` 的开发者工作站运行 `scripts/release/` 中的辅助脚本。

发布提交必须在推送任何 `vX.Y.Z` 标签之前落地到 `main`。不要标记仅发布分支。针对 `main` 打开发布 PR，让必需的审查和 CI 完成，合并它，然后明确标记从 `main` 可达的最终源代码提交。这使得 GitHub 能够自动处理 `Closes #N` 行，并将发布 PR 显示为已合并。标签发布工作流对标签推送和手动调度运行 `scripts/release/ensure-release-on-main.sh`，并在资产发布之前使仅分支发布源失败。

1. 编写 CHANGELOG 条目，然后运行 `./scripts/release/prepare-release.sh X.Y.Z`——它会升级每个包含版本的文件（工作区 + crate 固定 + npm 包装器 + README 安装标签），刷新 lockfile 和生成的文件，并运行 `check-versions.sh`。
2. 在本地运行 `./scripts/release/publish-crates.sh dry-run`；必须是干净的。
3. 在打标签之前将发布 PR 合并到 `main`。在相同版本队列冻结且 `main` 处于预期的源 SHA 之后，使用手动 **创建发布标签** 工作流或从开发者机器签名本地标签推送从 `main` 创建 `vX.Y.Z`。有关 `RELEASE_TAG_PAT` / 手动发布调度注意事项，请参阅下面的 npm 包装器发布部分。
4. 使用 `./scripts/release/publish-crates.sh publish` 按以下顺序发布 crate：
   - `codewhale-mcp`
   - `codewhale-protocol`
   - `codewhale-release`
   - `codewhale-secrets`
   - `codewhale-state`
   - `codewhale-workflow`
   - `codewhale-execpolicy`
   - `codewhale-hooks`
   - `codewhale-tools`
   - `codewhale-config`
   - `codewhale-agent`
   - `codewhale-tui`
   - `codewhale-core`
   - `codewhale-app-server`
   - `codewhale-cli`
5. 等待每个已发布的 crate 版本出现在 crates.io 上，然后再发布依赖项。

发布辅助脚本对于重新运行是幂等的：已发布的 crate 版本将被跳过。

## GitHub Release 资产

`.github/workflows/release.yml` 构建以下二进制文件：

- Linux x64/arm64、macOS x64/arm64 和 Windows x64 的 `codewhale-*` CLI 二进制文件
- 相同目标矩阵的 `codewhale-tui-*` TUI 二进制文件
- 相同目标矩阵的 `codew-*` 快捷方式二进制文件
- Windows npm 启动器的 `codewhale.bat`

发布作业还上传 `codewhale-artifacts-sha256.txt`。npm 安装器和发布验证脚本都依赖该校验和清单。面向 npm 的规范资产列表位于 `npm/codewhale/scripts/artifacts.js`。

在任何 Cargo 或 npm 发布之前，证明公开的 GitHub Release 资产属于您要发布的标签提交：

```bash
./scripts/release/verify-release-assets.sh X.Y.Z
```

该门禁比较本地和远程 `vX.Y.Z` 标签 SHA，确认成功的 `Release` 工作流运行使用了该 SHA，然后针对公开的 GitHub 资产 URL 运行 npm 包装器的发布检查。如果发布缺少面向 npm 的资产、校验和清单遗漏了必需的二进制文件或资产早于匹配的发布工作流运行，npm 检查将失败。如果命令失败，请重新运行或修复 `release.yml`；不要针对过时资产发布 Cargo 或 npm。

## npm 包装器发布

**npm 发布步骤是手动的。** `release.yml` 不再运行 `npm publish`，因为 npm 账户每次发布都需要 2FA OTP，且尚未配置绕过 2FA 的自动化令牌。GitHub Release 流程保持完全自动化；只有 npm 包装器发布需要开发者在配置了 `npm login` 和验证器应用的工作站上操作。

### 步骤

1. 在 [npm/codewhale/package.json](../npm/codewhale/package.json) 中设置 npm 包版本以匹配工作区 `Cargo.toml`。CI 的版本差异守卫将在标签之前捕获不匹配。
2. 将 `codewhaleBinaryVersion` 设置为应提供二进制文件的 GitHub 发布标签。
3. 将版本升级推送到 `main`。在发布源冻结后，从 `main` 创建匹配的 `vX.Y.Z` 标签；`release.yml` 然后构建二进制矩阵并起草 GitHub Release。
4. **等待 GitHub Release 完成**，包含完整的面向 npm 的二进制矩阵加上 `codewhale-artifacts-sha256.txt`。npm 的 `prepublishOnly` 钩子（`scripts/verify-release-assets.js`）要求每个资产都存在。
5. 从仓库根目录运行公开资产新鲜度门禁：

```bash
./scripts/release/verify-release-assets.sh X.Y.Z
```

对于罕见的仅 npm 打包发布，其中 npm 包版本有意指向较旧的 Rust 二进制文件，添加 `--allow-npm-binary-mismatch` 并在发布说明中明确说明没有新的二进制版本交付。

6. 从开发者机器确认 npm 认证并手动发布包装器：

```bash
npm whoami
cd npm/codewhale
npm publish --access public
# （您将被提示从验证器输入 npm OTP）
npm view codewhale@X.Y.Z version codewhaleBinaryVersion --json
cd ../..
./scripts/release/check-published.sh X.Y.Z
```

如果 `npm whoami` 或 `npm publish` 报告 `E401`、`ENEEDAUTH` 或 OTP/登录失败，不要编辑包内容。运行：

```bash
npm login
npm whoami
cd npm/codewhale
npm publish --access public
```

完成登录或 OTP 提示后重新运行相同的 `npm publish --access public` 命令。包的 `prepublishOnly` 钩子在每次发布尝试之前重新运行发布资产门禁，因此认证失败不会在重试时意外跳过资产验证。

不要发布 `npm/deepseek-tui`；它只是已弃用的兼容元数据。

### 为什么不自动化？

- `release.yml` 的旧 `publish-npm` 作业使用了 `secrets.NPM_TOKEN`，但 npm 的默认 2FA 策略意味着发布令牌必须是启用了"绕过 2FA 进行令牌认证"的自动化令牌，或者是账户级别的 2FA 禁用状态。我们两者都没有配置。
- 独立的 `publish-npm.yml` 和 `crates-publish.yml` 工作流已被移除；没有残留的惰性自动化管道。未来转向 npm 可信发布（OIDC）时将在那时重新引入专用工作流。

### 如果以后修复了令牌

要重新启用自动化发布：配置启用"绕过 2FA 进行令牌认证"的 npm 自动化令牌（或通过 OIDC 设置 npm 可信发布），在仓库上存储相应的密钥，并将 `publish-npm` 作业重新添加到 `release.yml`（或专用工作流），同时还原本节的"手动"框架。

## CNB Cool 镜像

每次推送到 `main`、`fix/*`、`rebrand/*`、`work/v*` 和每个 `v*` 标签都会通过 `Sync to CNB` 工作流镜像到 `cnb.cool/codewhale.net/codewhale`，以便在 GitHub 被屏蔽的网络后面的用户可以获取源代码，并让 CNB 运行重量级 Linux CI 通道。在发布标签后，**在声明发布已交付之前验证镜像已捕获它**：

```bash
git ls-remote https://cnb.cool/codewhale.net/codewhale.git refs/tags/vX.Y.Z
```

如果发布标签的工作流失败，手动回退方案记录在 [docs/CNB_MIRROR.md](CNB_MIRROR.md)（一次性 `git remote add cnb …`，然后 `git push cnb vX.Y.Z`）。

## 恢复和回滚

- 面向用户的回滚：
  - npm：`npm install -g codewhale@X.Y.Z`
  - Cargo：`cargo install codewhale-cli --version X.Y.Z --locked --force` 和 `cargo install codewhale-tui --version X.Y.Z --locked --force`
  - 手动资产：从 `https://github.com/Hmbown/CodeWhale/releases/tag/vX.Y.Z` 下载二进制文件或平台归档以及匹配的 `codewhale-artifacts-sha256.txt` 或 `codewhale-bundles-sha256.txt` 清单
  - 工作区文件：使用 `/restore list [N]` 和 `/restore <N>` 进行 side-git 快照；这不会更改已安装的二进制版本或重写对话历史
  - 保持 [docs/INSTALL.md](INSTALL.md#roll-back-to-a-previous-release) 与这些命令同步
- Crate 发布部分完成：
  - 重新运行 `./scripts/release/publish-crates.sh publish`
  - 已发布的 crate 版本将被跳过
- GitHub 资产缺失或校验和清单不完整：
  - 修复 `.github/workflows/release.yml`
  - 在 `npm publish` 之前重新标记或上传更正的资产
- 仅 npm 打包问题：
  - 仅升级 npm 包版本
  - 保持 `codewhaleBinaryVersion` 在最后已知良好的 Rust 发布版本
  - 重新打包并重新发布包装器
- 错误的 npm 发布无法覆盖：
  - 使用更正的元数据或安装逻辑发布新的 npm 版本
- CNB 镜像发布标签失败：
  - 通过 `gh run list --workflow=sync-cnb.yml` 检查运行
  - 使用 `gh workflow run sync-cnb.yml` 重新触发，或按照 [docs/CNB_MIRROR.md](CNB_MIRROR.md#manual-fallback) 手动推送标签
