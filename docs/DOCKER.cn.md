# Docker

CodeWhale 每次发布都会向 GitHub Container Registry 推送一个多架构 Linux 镜像。

```bash
docker pull ghcr.io/hmbown/codewhale:latest
```

## 快速开始

使用 Docker 管理的数据卷运行已发布的镜像：

```bash
docker volume create codewhale-home

docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:latest
```

使用固定版本的发布标签以确保可复现的安装：

```bash
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:vX.Y.Z
```

将 `vX.Y.Z` 替换为 [GitHub Releases](https://github.com/Hmbown/CodeWhale/releases) 中的标签。

## 默认镜像约定

`ghcr.io/hmbown/codewhale:latest` 以及语义化版本标签是保守的运行时镜像：

- 容器以非 root 用户 `codewhale` 运行，UID/GID 为 `1000:1000`
- 镜像不授予免密码 `sudo` 权限
- 镜像用于对挂载的工作区运行 CodeWhale，而非在运行时修改基础操作系统
- 用户状态应存储在挂载到 `/home/codewhale/.codewhale` 的卷中

这些默认设置是经过深思熟虑的。请继续使用它们以获得最小的信任边界。如果项目需要 `apt-get`、编译器工具链、Node/Python 包管理器、自定义 CA 证书或其他类似主机的 Docker 内设置，请构建一个显式的工具箱镜像，而不是更改默认镜像约定。

## 可选工具箱/自定义镜像

仓库中包含一个示例文件
[`docs/examples/Dockerfile.toolbox`](examples/Dockerfile.toolbox)，它在官方镜像基础上扩展了免密码 `sudo` 和常用开发包。
当你需要可复现的项目环境时，使用固定的 CodeWhale 标签构建它：

```bash
docker build -f docs/examples/Dockerfile.toolbox \
  --build-arg CODEWHALE_IMAGE=ghcr.io/hmbown/codewhale:vX.Y.Z \
  --build-arg TOOLBOX_PACKAGES="git openssh-client curl build-essential pkg-config python3 python3-pip nodejs npm" \
  -t codewhale-toolbox:my-project .
```

仅在一次性测试时使用 `latest`。对于共享项目，请保持 `CODEWHALE_IMAGE` 值固定，并像审查其他开发环境变更一样审查新增的软件包。

使用相同的工作区和状态挂载运行工具箱镜像：

```bash
docker volume create codewhale-my-project-home

docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-my-project-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -w /workspace \
  codewhale-toolbox:my-project
```

在此可选镜像内部，CodeWhale 可以使用诸如 `sudo apt-get update` 和 `sudo apt-get install -y <package>` 的命令。对于可复现的容器，建议将这些软件包预先烘焙到工具箱 Dockerfile 中，而不是让长期运行的容器产生漂移。

不要将 API 密钥、SSH 私钥或其他机密信息烘焙到自定义镜像中。在运行时传入 API 密钥，并谨慎挂载任何 SSH 材料，最好是只读挂载，且仅在需要的项目中使用。

### Compose 工具箱模板

如果你倾向于使用可复现的 `docker compose` 入口点，可以使用
[`docs/examples/compose.toolbox.yml`](examples/compose.toolbox.yml)。它从 [`docs/examples/Dockerfile.toolbox`](examples/Dockerfile.toolbox) 构建工具箱镜像，并明确项目状态卷：

```bash
CODEWHALE_IMAGE=ghcr.io/hmbown/codewhale:vX.Y.Z \
CODEWHALE_TOOLBOX_IMAGE=codewhale-toolbox:my-project \
CODEWHALE_HOME_VOLUME=codewhale-my-project-home \
CODEWHALE_WORKSPACE="$PWD" \
docker compose -f docs/examples/compose.toolbox.yml run --rm codewhale
```

对每个需要独立工具链或独立 `.codewhale` 状态的项目，使用不同的 `CODEWHALE_TOOLBOX_IMAGE` 和 `CODEWHALE_HOME_VOLUME`。Compose 文件还展示了 SSH 材料和本地 CA 证书的可选只读挂载；除非项目需要，否则保持这些挂载被注释掉。

## 多个独立项目

每个项目使用一个命名状态卷，以避免会话、配置、技能、记忆和离线队列在工作区之间泄露：

```bash
project="$(basename "$PWD")"
image="codewhale-toolbox:${project}"
docker volume create "codewhale-${project}-home"

docker run --rm -it \
  --name "codewhale-${project}" \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v "codewhale-${project}-home:/home/codewhale/.codewhale" \
  -v "$PWD:/workspace" \
  -w /workspace \
  "$image"
```

对于工具链不同的项目，构建不同的工具箱标签，例如 `codewhale-toolbox:frontend` 和 `codewhale-toolbox:backend`。issue #2217 中讨论的独立启动器方案可以基于此约定构建，但它被有意地置于核心 Docker 镜像之外。

## 项目引导脚本

CodeWhale 不会自动执行 `.codewhale/setup.sh` 或旧版 `.deepseek/setup.sh`。如果你保留了这些文件作为本地项目配方，请显式运行它们。对于共享团队设置，建议使用已提交的项目脚本或工具箱 Dockerfile，以便环境可以被审查和重建。

例如，在启动 CodeWhale 之前运行一个已提交的引导脚本：

```bash
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-my-project-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -w /workspace \
  --entrypoint bash \
  codewhale-toolbox:my-project \
  -lc './scripts/bootstrap-dev.sh && exec codewhale'
```

对于需要 `sudo` 的引导脚本，请使用工具箱镜像。默认镜像无法提升权限。

## 自定义 CA 证书和代理

对于企业代理、开发侧载工具或自签名内部服务，建议将受信任的 CA 证书烘焙到自定义工具箱镜像中：

```dockerfile
USER root
COPY docker/certs/*.crt /usr/local/share/ca-certificates/
RUN update-ca-certificates
USER codewhale
```

所有复制到 `/usr/local/share/ca-certificates/` 的文件必须使用 `.crt` 扩展名。请勿将私有 CA 材料放入公共镜像。

对于仅限本地的运行，可以只读挂载证书并在容器启动时更新信任存储：

```bash
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-my-project-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -v "$PWD/docker/certs:/usr/local/share/ca-certificates/local:ro" \
  -w /workspace \
  --entrypoint bash \
  codewhale-toolbox:my-project \
  -lc 'sudo update-ca-certificates && exec codewhale'
```

此 CA 工作流需要可选工具箱镜像，因为默认镜像不包含免密码 `sudo`。

## 本地构建

从检出的代码本地构建镜像：

```bash
docker build -t codewhale .
```

然后使用相同的 Docker 管理数据卷运行它：

```bash
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -w /workspace \
  codewhale
```

Docker Hub 发布未配置；GHCR 是受支持的预构建镜像仓库。

## 环境变量

| 变量                   | 必需   | 描述                                             |
|-----------------------|--------|--------------------------------------------------|
| `DEEPSEEK_API_KEY`    | 是     | DeepSeek API 密钥                                |
| `DEEPSEEK_BASE_URL`   | 否     | 自定义 API 基础 URL（例如 `https://api.deepseek.com`） |
| `DEEPSEEK_NO_COLOR`   | 否     | 设置为 `1` 以禁用终端颜色输出                     |

## 卷

挂载 `/home/codewhale/.codewhale` 以在容器重启之间持久化会话、配置、技能、记忆和离线队列。镜像还保留 `/home/codewhale/.deepseek` 以实现旧版兼容。Docker 管理的命名卷是最安全的默认选项，因为 Docker 创建的卷拥有容器可写的所有权：

```bash
-v codewhale-home:/home/codewhale/.codewhale
```

如果不挂载此卷，容器每次启动都是全新的。

如果你改用绑载现有主机目录，镜像以非 root 用户 `codewhale` 运行，UID/GID 为 `1000:1000`。挂载的目录必须可由该用户写入，否则在 `.codewhale/tasks` 下创建运行时目录时启动可能失败。在 Linux 主机上，要么使用上述命名卷，要么显式准备绑载目录：

```bash
mkdir -p ~/.codewhale
sudo chown -R 1000:1000 ~/.codewhale

docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v ~/.codewhale:/home/codewhale/.codewhale \
  ghcr.io/hmbown/codewhale:latest
```

该 `chown` 操作会更改主机 `~/.codewhale` 目录的所有权。如果你不希望容器 UID 拥有你的本地配置，请跳过此步骤，改用命名卷。

## 非交互式/管道使用

当 stdin 不是 TTY 时，`codewhale` 会降级到调度器的单次执行模式（`codewhale -c "…"`）。通过管道将提示词传入 stdin：

```bash
echo "Explain the Cargo.toml in structured English." | \
  docker run --rm -i -e DEEPSEEK_API_KEY ghcr.io/hmbown/codewhale:latest
```

## 本地构建

```bash
# 单平台（你的主机架构）
docker build -t codewhale .

# 多平台（需要一个支持仿真的构建器）
docker buildx create --use
docker buildx build --platform linux/amd64,linux/arm64 -t codewhale .
```

## Devcontainer

仓库中包含一个 [`.devcontainer/devcontainer.json`](../.devcontainer/devcontainer.json) 配置，用于 VS Code / GitHub Codespaces。它预装了 Rust 工具链、rust-analyzer 和 `codewhale` 二进制文件。在 devcontainer 中打开仓库即可获得一个开箱即用的开发环境。

## 发布状态

Docker 镜像发布是发布关卡的一部分。镜像被发布到 GHCR，支持 `linux/amd64` 和 `linux/arm64`，带有语义化版本标签以及 `latest`。
