# mset

[English](../README.md) | 简体中文

Mihomo 命令行管理工具。编译需要 Rust；部署编译产物不需要 Rust。

## 安装 mset

在仓库 Releases 页面下载符合系统和架构的压缩包，无需安装 Rust：

| 系统 | 架构 | 文件后缀 |
| --- | --- | --- |
| Linux | Intel/AMD 64 位 | `linux-x86_64.tar.gz` |
| Linux | ARM64 | `linux-arm64.tar.gz` |
| macOS | Intel | `macos-x86_64.tar.gz` |
| macOS | Apple Silicon | `macos-arm64.tar.gz` |

Linux 发布包使用 musl 构建，避免依赖目标电脑的 glibc 版本。

### 安装为全局工具（无需 Rust）

将 `version` 设置为已经发布的版本标签，并根据上表选择 `platform`，例如 Apple Silicon 使用 `macos-arm64`。

在 Linux/macOS 的 Bash 或 Zsh 中执行：

```bash
repo='knowaywood/mihomo_set'
version='v0.1.0'
platform='linux-x86_64'
archive="mset-${version}-${platform}"

# 从 GitHub Release 下载工具
curl --fail --location --output "${archive}.tar.gz" \
  "https://github.com/${repo}/releases/download/${version}/${archive}.tar.gz" &&
tar -xzf "${archive}.tar.gz" &&
sudo install -m 755 "${archive}/mset" /usr/local/bin/mset &&
mset --version
```

安装后，在任意目录都能直接运行 `mset`。发布页同时提供 `SHA256SUMS`，Linux 用户可将其下载到压缩包所在目录，执行 `sha256sum --check SHA256SUMS --ignore-missing` 校验已下载的压缩包。

没有 sudo 权限时，将上面的 `sudo install` 步骤替换为：

```bash
mkdir -p "$HOME/.local/bin"
install -m 755 "${archive}/mset" "$HOME/.local/bin/mset"
export PATH="$HOME/.local/bin:$PATH"
mset --version
```

要让新终端也能找到命令，请把 `export PATH` 这一行加入对应的 shell 配置：Bash 使用 `~/.bashrc`，Zsh 使用 `~/.zshrc`。

### 从本地源码安装（需要 Rust）

在项目目录执行：

```bash
cargo install --path . --locked
mset --version
```

Cargo 会把工具安装到 `$CARGO_HOME/bin`，默认是 `$HOME/.cargo/bin`，请确保该目录已加入 PATH。

如果不想安装，也可以执行 `cargo build --release --locked`，并把下文命令中的 `mset` 换成 `./target/release/mset`。

## 下载、配置并启动 Mihomo

自动安装支持 Linux/macOS 的 x86_64、ARM64，从 MetaCubeX/mihomo 官方 GitHub Release 下载适配内核。需要能访问 GitHub；不需要 sudo。

```bash
# 下载最新稳定版并生成配置，接入 Mihomo/Clash YAML 订阅
mset setup --subscription 'https://example.com/subscription'

# 后台运行，启动后即可继续使用管理命令
mset start
mset use  # 自动识别代理组，显示节点菜单，输入序号选择
# 或直接指定名称：
mset use '节点名'
```

订阅必须是 Mihomo/Clash 兼容的 YAML（完整配置中的 proxies 或代理集合），不支持通用 base64、V2Ray 订阅。订阅通过 proxy-provider 导入节点，不导入订阅中的 DNS、规则和代理组；首次启动和后续更新由 Mihomo 下载。

默认目录是 `$HOME/.config/mihomo`，内含 `mihomo` 和 `config.yaml`。默认代理端口 7890、控制 API 为 `127.0.0.1:9090`，不开放局域网访问、不接管系统代理、不启用 TUN。配置规则将流量交给 `PROXY` 组（初始选择 DIRECT），请主动选择节点。未提供订阅时生成纯直连配置，可手动编辑后启动。

需要使用代理的应用应设置代理地址为 `127.0.0.1:7890`；`mset` 不会自动修改系统代理。

可选参数：

```bash
# 指定版本、目录、代理端口和控制密钥
mset setup --release v1.19.34 --dir ./mihomo-local --port 7891 \
  --secret 'your-secret' --subscription 'https://example.com/subscription'
mset start --dir ./mihomo-local

# 自定义目录的配置需显式告知后续管理命令
export MIHOMO_CONFIG="$PWD/mihomo-local/config.yaml"
mset now
```

`setup` 不覆盖已有内核或配置；需要重新安装时使用新目录。下载失败可以重试。`start` 先运行内核配置校验，再后台启动；不会设置开机自启。配置文件以仅当前用户可读写的权限创建。

`--secret` 未提供时读取 `MIHOMO_SECRET`，否则生成空密钥配置。控制 API 默认仅监听本机。普通管理命令可以自动读取默认目录中的控制地址和密钥。

## 常用管理命令

```bash
mset --help
mset now
mset test
mset best
mset mode rule
mset set mixed-port 7890
mset config
mset reload
```

`mset test` 和 `mset best` 默认使用 `PROXY` 代理组，无需填写 `-g PROXY`。需要其他组时使用 `mset test -g Media` 或 `mset best -g Media`。若配置中没有 `PROXY`，请通过 `-g` 指定实际组名。

通过 API 修改的运行参数不会由本工具写回配置文件。持久修改请编辑 `config.yaml` 后重新加载。

## 简化节点选择

运行 `mset use` 即可进入交互菜单，不必先执行 `proxies` 或指定 `-g`。
默认优先使用 setup 创建的 PROXY 组；只有一个可手动切换的组时自动使用该组；
存在多个其他组时先让用户选组。输入 q 取消。非交互环境请指定节点名。

`mset use '节点名'` 会自动定位组；存在多个候选且没有 PROXY 时，使用 `-g` 明确指定。
`mset use 3` 保留原含义：选择上次测速排名第三的节点；菜单中的序号则对应当前菜单。
也可运行 `mset best` 测速并自动选节点。

## 后台进程管理

```bash
mset start     # 后台启动，已运行时不会重复启动
mset stop      # 正常关闭 Mihomo，最多等待 10 秒
mset restart   # 等待旧进程退出，再重新校验配置、后台启动
```

自定义安装目录时，三个命令均需使用相同的 `--dir`。
日志写入安装目录的 `mihomo.log`，进程记录为 `mihomo.pid`。
后台进程脱离终端，关闭终端后继续运行；不提供开机自启。
启动命令会检测立即退出的启动错误，但不保证订阅已加载完成。
`stop` 和 `restart` 只管理由新版 `mset start` 启动并记录的实例，
不会关闭手动启动或 systemd 等其他工具管理的 Mihomo。
旧版前台启动的进程请先在原终端 Ctrl+C 关闭，再使用新版启动。
停止超时会保留进程记录，restart 不会继续启动第二个实例。

## 发布新版本

`.github/workflows/release.yml` 会在分支推送、PR 和手动触发时执行四个平台的构建、单元测试、进程生命周期测试和打包。
只有推送 `v*` 标签才自动创建 GitHub Release，并上传四个压缩包及 `SHA256SUMS`；包含 `-` 的版本号标记为预发布。每个压缩包均包含中英文 README。

1. 更新 `Cargo.toml` 的 `version`，执行 `cargo check` 更新 `Cargo.lock`，提交代码及工作流。
2. 推送提交，再创建并推送与包版本一致的标签。例如当前版本 `0.1.0`：

   ```bash
   git push
   git tag -a v0.1.0 -m 'Release v0.1.0'
   git push origin v0.1.0
   ```

3. 在 GitHub 的 Actions 页面查看 `CI and Release`，全部通过后在 Releases 页面下载安装包。

使用内置 `GITHUB_TOKEN`，无需额外配置 PAT。仓库或组织需要允许 Actions 运行及发布作业的 `contents: write` 权限。
标签必须与 `Cargo.toml` 版本一致，否则流程会提前失败。手动触发只做验证与打包，不发布 Release。
发布作业可重跑以重新上传同名附件。需要发布新版本时使用新版本号和新标签。
