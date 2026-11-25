# mset

English | [简体中文](docs/README.zh-CN.md)

A command-line tool for installing and managing Mihomo. Rust is required to build from source, but is not needed to run a compiled binary.

## Install mset

Download the archive matching your system and architecture from this repository's Releases page. Rust is not required.

| System | Architecture | Filename suffix |
| --- | --- | --- |
| Linux | Intel/AMD 64-bit | `linux-x86_64.tar.gz` |
| Linux | ARM64 | `linux-arm64.tar.gz` |
| macOS | Intel | `macos-x86_64.tar.gz` |
| macOS | Apple Silicon | `macos-arm64.tar.gz` |

Linux release binaries are built with musl to avoid depending on the target computer's glibc version.

### Install as a global command (no Rust required)

Set `version` to an existing release tag, and choose `platform` from the table above (for example, `macos-arm64` for Apple Silicon).

Run this in Bash or Zsh on Linux/macOS:

```bash
repo='knowaywood/mihomo_set'
version='v0.1.0'
platform='linux-x86_64'
archive="mset-${version}-${platform}"

# Download the tool from GitHub Releases
curl --fail --location --output "${archive}.tar.gz" \
  "https://github.com/${repo}/releases/download/${version}/${archive}.tar.gz" &&
tar -xzf "${archive}.tar.gz" &&
sudo install -m 755 "${archive}/mset" /usr/local/bin/mset &&
mset --version
```

After installation, `mset` is available from any directory. Each release also includes `SHA256SUMS`. On Linux, download it alongside the archive and run `sha256sum --check SHA256SUMS --ignore-missing` to verify the downloaded archive.

Without sudo, replace the `sudo install` step with:

```bash
mkdir -p "$HOME/.local/bin"
install -m 755 "${archive}/mset" "$HOME/.local/bin/mset"
export PATH="$HOME/.local/bin:$PATH"
mset --version
```

To keep that PATH setting across new terminals, add the `export PATH` line to `~/.bashrc` for Bash or `~/.zshrc` for Zsh.

### Install from local source (Rust required)

From the project directory:

```bash
cargo install --path . --locked
mset --version
```

Cargo installs the tool in `$CARGO_HOME/bin`, or `$HOME/.cargo/bin` by default. Make sure that directory is on your PATH.

If you prefer not to install it, run `cargo build --release --locked` and replace `mset` in the commands below with `./target/release/mset`.

## Download, configure, and start Mihomo

Automatic installation supports Linux and macOS on x86_64 and ARM64. It downloads the matching core from the official MetaCubeX/mihomo GitHub Releases. Access to GitHub is required; sudo is not needed for setup.

```bash
# Download the latest stable core and create a configuration with a YAML subscription
mset setup --subscription 'https://example.com/subscription'

# Start in the background, then choose a node from the interactive menu
mset start
mset use

# Or select a node by name
mset use 'node-name'
```

Subscriptions must use Mihomo/Clash-compatible YAML: either a full configuration containing `proxies` or a proxy-provider file. Generic base64 and V2Ray subscriptions are not supported. The subscription is used as a proxy provider; its DNS settings, rules, and proxy groups are not imported. Mihomo downloads the subscription on first use and handles subsequent updates.

The default installation directory is `$HOME/.config/mihomo`, containing `mihomo` and `config.yaml`. The default proxy port is `7890`, and the control API listens on `127.0.0.1:9090`. LAN access, system proxy configuration, and TUN are not enabled. The generated rule sends traffic to the `PROXY` group, which initially selects `DIRECT`; select a proxy node to use it. Without a subscription, setup generates a direct-only configuration that you can edit before starting.

The application that needs the proxy should use `127.0.0.1:7890`; `mset` does not configure the system proxy automatically.

Optional settings:

```bash
# Specify a release, installation directory, proxy port, and API secret
mset setup --release v1.19.34 --dir ./mihomo-local --port 7891 \
  --secret 'your-secret' --subscription 'https://example.com/subscription'
mset start --dir ./mihomo-local

# Tell management commands where to find a configuration in a custom directory
export MIHOMO_CONFIG="$PWD/mihomo-local/config.yaml"
mset now
```

`setup` preserves existing binaries and configuration files. Use a different directory to install another copy. Failed downloads can be retried. `start` validates the configuration before launching in the background; it does not configure startup at boot. Configuration files are created with owner-only read/write permissions.

If `--secret` is omitted, setup reads `MIHOMO_SECRET`; if neither is provided, it creates a configuration with an empty secret. The control API listens on localhost by default. Management commands automatically discover the API address and secret from the configuration in the default directory.

## Common management commands

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

`mset test` and `mset best` default to the `PROXY` group. To use another group, run `mset test -g Media` or `mset best -g Media`. If `PROXY` does not exist in your configuration, specify the actual group name with `-g`.

Runtime changes made through the API are not written back to the configuration file by this tool. For persistent changes, edit `config.yaml` and reload it.

## Node selection

Run `mset use` to open the interactive menu. There is no need to run `proxies` first or specify `-g`.
The tool prefers the `PROXY` group created by setup. If there is only one manually selectable group, it uses that group automatically; otherwise, it asks you to choose a group first. Enter `q` to cancel. In a noninteractive environment, specify a node name.

`mset use 'node-name'` locates the group automatically. If multiple groups contain the node and none is named `PROXY`, use `-g` to specify the group.
`mset use 3` selects the third node in the last saved latency test ranking; numbers entered in the interactive menu refer to the current menu instead.
You can also run `mset best` to test nodes and select the fastest automatically.

## Background process management

```bash
mset start     # Start in the background; do not launch a duplicate instance
mset stop      # Request a graceful shutdown and wait up to 10 seconds
mset restart   # Wait for shutdown, validate the configuration, and start again
```

For a custom installation directory, pass the same `--dir` to all three commands.
Logs are written to `mihomo.log` in the installation directory, and the process record is stored in `mihomo.pid`.
The background process runs independently of the terminal and continues after it closes. Startup at boot is not configured.
The start command detects immediate startup failures, but does not guarantee that subscriptions have finished loading.

`stop` and `restart` only manage instances launched and recorded by the current background-capable `mset start`. They do not stop Mihomo instances started manually or managed by systemd or other tools.
If you are upgrading from the earlier foreground version, stop the old process with Ctrl+C in its original terminal before starting the new version.
If shutdown times out, the process record is retained and restart does not launch a second instance.

## Publish a release

`.github/workflows/release.yml` builds, runs unit and process lifecycle tests, and packages binaries for all four platforms on branch pushes, pull requests, and manual runs.
Only a pushed `v*` tag creates a GitHub Release and uploads the four archives plus `SHA256SUMS`. Versions containing `-` are marked as prereleases. Each archive includes both the English and Chinese READMEs.

1. Update `version` in `Cargo.toml`, run `cargo check` to update `Cargo.lock`, and commit the code and workflow.
2. Push the commit, then create and push a tag matching the package version. For the current version, `0.1.0`:

   ```bash
   git push
   git tag -a v0.1.0 -m 'Release v0.1.0'
   git push origin v0.1.0
   ```

3. Check `CI and Release` in the repository's Actions tab. After it succeeds, download the packages from Releases.

The workflow uses the built-in `GITHUB_TOKEN`; no additional PAT is required. Repository or organization policies must allow Actions to run and grant the publishing job `contents: write` permission.
The tag must match the version in `Cargo.toml`, or the workflow fails before building. Manual runs validate and package the project without publishing a release.
The release job can be rerun to upload replacement assets with the same filenames. For a new version, use a new version number and tag.
