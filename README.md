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
version='v0.1.1'
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
mset status
```

`setup` preserves existing binaries and configuration files. Use a different directory to install another copy. Failed downloads can be retried. `start` validates the configuration before launching in the background; it does not configure startup at boot. Configuration files are created with owner-only read/write permissions.

If `--secret` is omitted, setup reads `MIHOMO_SECRET`; if neither is provided, it creates a configuration with an empty secret. The control API listens on localhost by default. Management commands automatically discover the API address and secret from the configuration in the default directory.

## Common management commands

```bash
mset --help
mset status
mset test
mset best
mset mode rule
mset set mixed-port 7890
mset config
mset reload
```

`mset test` and `mset best` default to the `PROXY` group. To use another group, run `mset test -g Media` or `mset best -g Media`. If `PROXY` does not exist in your configuration, specify the actual group name with `-g`.

When the controller matches a local configuration file, `tun on/off`, `mode`, and `set` changes are saved to that file and survive `reload` and `restart`. Other YAML fields are preserved. Each save backs up the exact previous file to `<config>.bak`, writes with owner-only permissions, and atomically replaces the configuration. YAML formatting, comments, and anchor layout may be normalized; the previous text is available in the backup. API failures do not commit file changes.

Remote controllers or controllers without a matching local file remain runtime-only and print a notice. Explicitly reloading a different file uses that file's settings. Configuration is parsed as YAML, including quoted/escaped secrets, inline comments, and merge keys; malformed YAML reports an error instead of silently using another controller. Automatic discovery skips files with no enabled `external-controller` (for example, an old file containing only `mixed-port`) and continues to the next candidate. An explicit `MIHOMO_CONFIG` or `--dir` configuration is validated strictly.

## Node selection

Run `mset use` to open the interactive menu. There is no need to run `proxies` first or specify `-g`.
The tool prefers the `PROXY` group created by setup. If there is only one manually selectable group, it uses that group automatically; otherwise, it asks you to choose a group first. Enter `q` to cancel. In a noninteractive environment, specify a node name.

`mset use 'node-name'` locates the group automatically. If multiple groups contain the node and none is named `PROXY`, use `-g` to specify the group.
`mset use 3` selects the third node in the last saved latency test ranking; numbers entered in the interactive menu refer to the current menu instead.
You can also run `mset best` to test nodes and select the fastest automatically.

Node selections use Mihomo's native `profile.store-selected` cache and are restored on reload/restart while the node still exists. Setup enables this by default. For an existing matching local configuration without caching, the first selection enables it and reloads the configuration once before changing the node. Remote controllers need their own selection cache configured.

### Test results and status information

```bash
mset test       # Show latency rankings, routing mode, and TUN/device status, then exit
mset use 3      # Select the third node from the last test ranking
mset tun on    # Enable TUN
mset tun off   # Disable TUN
mset mode rule # Use rule-based routing (also supports global/direct)
```

`mset test` always includes mode and TUN information, without additional flags or input prompts. It returns to the shell immediately after displaying results. The former `-i`/`--interactive` option has been removed.

`mset use` without a node still opens the node menu, where `t` toggles TUN and `r/g/d` switches routing modes. TUN is a traffic capture method; rule/global/direct controls routing. Disabling TUN leaves HTTP/SOCKS available and does not configure the system proxy. In global mode, traffic follows the `GLOBAL` group's selection.

## Background process management

```bash
mset start     # Start in the background; do not launch a duplicate instance
mset stop      # Request a graceful shutdown and wait up to 60 seconds
mset restart   # Wait for shutdown, validate the configuration, and start again
```

For a custom installation directory, pass the same `--dir` to all three commands.
Logs are written to `mihomo.log` in the installation directory, and the process record is stored in `mihomo.pid`.
The background process runs independently of the terminal and continues after it closes. Startup at boot is not configured.
The start command checks both process liveness and API readiness before reporting success. The default startup timeout is 30 seconds; use `mset start --timeout 60` or `mset restart --timeout 60` to change it. An API failure reports the cause and log path; a still-running process keeps its PID record so `mset stop` can manage it. API readiness does not guarantee that subscriptions have finished loading.

`stop` and `restart` only manage instances launched and recorded by the current background-capable `mset start`. They do not stop Mihomo instances started manually or managed by systemd or other tools.
If you are upgrading from the earlier foreground version, stop the old process with Ctrl+C in its original terminal before starting the new version.
Shutdown waits for the process to exit, for up to 60 seconds, to allow time for TUN cleanup. It returns as soon as the process exits; no manual sleep is needed between stop and start. If shutdown times out, the process record is retained and restart does not launch a second instance.

## TUN control

With Mihomo running, use a single command:

```bash
mset tun on    # Authorize and restart if needed, then enable TUN
mset tun off   # Disable TUN
mset tun       # Show TUN configuration and device inspection
```

Use `mset status` to show mode, TUN/device status, and the current node of each group. For example:

```text
Mode: rule
TUN configured: on
TUN device: Meta (Linux TUN, UP; ownership/routes not verified)
```

For a loopback API on Linux, the tool reads the named device's kernel TUN flags and interface UP flag. A missing device is reported separately even when configuration says TUN is on. Missing API fields remain unknown. Remote APIs and other operating systems show configuration without claiming that a local device was verified. Loopback forwarding into a container may use a different network namespace; a missing host device does not prove that the remote core failed. Device existence does not establish ownership or prove that traffic is routed through it. No devices or routes are removed by these checks.

On Linux, `tun on` checks the running core's network capabilities. For a local instance managed by `mset start`, it automatically runs sudo/setcap and restarts the core only if permissions are missing, waits for the API, and enables TUN with automatic routing and interface detection. If permissions already exist, it enables TUN without restarting. If TUN is already configured on and the matching local device is a TUN interface marked UP, it keeps the existing device. If the device is missing, down, or the wrong type, it attempts to recreate it. Sudo may prompt for your password on first use.

For a custom installation directory, run `mset tun on --dir ./mihomo-local`. The command reads the controller address and secret from that directory's configuration.

Automatic authorization requires `setcap`, a filesystem supporting file capabilities, and access to the TUN device. Containers must allow the required capabilities. An explicit `--api`, `MIHOMO_API`, or a custom `MIHOMO_CONFIG` without `--dir` uses API-only control and never authorizes or restarts a local core. External supervisors are not managed automatically. macOS still requires the core to be started with administrator privileges; automatic authorization/restart is Linux-only.

For a matching local configuration, the TUN switch is saved to `config.yaml` and survives reloads and restarts. On local Linux, enabling TUN waits for a matching TUN interface to become UP; if this fails, the command reports an error, resets the runtime switch to off, and does not save the failed enable. Remote and non-Linux controllers cannot be verified using local Linux device inspection, so their device status remains unverified.

Existing DNS and device settings are preserved; this command does not add DNS hijacking or configure a resolver. An UP device still does not prove route ownership or end-to-end connectivity. Check `mihomo.log` if traffic is not routed as expected.

## Errors and verification

API errors identify authentication failures (401), denied access (403), missing resources (404), server errors (5xx), connection failures, timeouts, and malformed responses. Authentication, connection, or protocol errors from `tun on` never trigger local authorization or a restart. Invalid YAML and file permission errors include relevant configuration/path context. Request query strings and credentials are omitted from API error diagnostics.

Run the automated checks after building:

```bash
cargo test --locked
cargo build --release --locked
python3 tests/configuration.py
python3 tests/lifecycle.py
```

For real Linux TUN testing, `tests/tun_system.py` requires a real Mihomo binary (default `/usr/local/bin/mihomo`), `/dev/net/tun`, iproute2, and user/network/mount namespaces. It runs in a fresh namespace, uses no subscription or external network, and checks TUN type/UP state, route selection, native node caching, reload/restart, and link/IPv4/IPv6 route/rule restoration after disabling TUN or stopping the core:

```bash
unshare --user --map-root-user --net --mount --propagation private sh -eu -c \
  'mount -t sysfs sysfs /sys; MSET_ISOLATED_TEST=1 python3 tests/tun_system.py'
```

This verifies local routing and teardown in isolation; it does not measure external proxy connectivity or prove cleanup for every kernel/network configuration. It does not delete host routes or devices.

## Publish a release

`.github/workflows/release.yml` builds, runs unit and process lifecycle tests, and packages binaries for all four platforms on branch pushes, pull requests, and manual runs.
Only a pushed `v*` tag creates a GitHub Release and uploads the four archives plus `SHA256SUMS`. Versions containing `-` are marked as prereleases. Each archive includes both the English and Chinese READMEs.

1. Update `version` in `Cargo.toml`, run `cargo check` to update `Cargo.lock`, and commit the code and workflow.
2. Push the commit, then create and push a tag matching the package version. For the current version, `0.1.1`:

   ```bash
   git push
   git tag -a v0.1.1 -m 'Release v0.1.1'
   git push origin v0.1.1
   ```

3. Check `CI and Release` in the repository's Actions tab. After it succeeds, download the packages from Releases.

The workflow uses the built-in `GITHUB_TOKEN`; no additional PAT is required. Repository or organization policies must allow Actions to run and grant the publishing job `contents: write` permission.
The tag must match the version in `Cargo.toml`, or the workflow fails before building. Manual runs validate and package the project without publishing a release.
The release job can be rerun to upload replacement assets with the same filenames. For a new version, use a new version number and tag.
