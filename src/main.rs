mod api;
mod cfg;
mod cmds;
mod daemon;
mod errors;
mod install;
mod state;
mod tun;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "mset",
    version,
    about = "A tiny CLI controller for mihomo (RESTful API)",
    after_help = "Examples:\n  mset proxies               list proxy groups\n  mset proxies GLOBAL        list nodes in a group\n  mset use 3                 select the 3rd node of the last test\n  mset use HK-01             select node by name (auto group)\n  mset use 2 -g Media        select in a specific group\n  mset status                mode, TUN status and current nodes\n  mset test                  test PROXY group (sorted by delay)\n  mset best                  select the fastest node in PROXY\n  mset test -g Media         test a specific group\n  mset delay HK-01           test a single node\n  mset mode global           switch running mode\n  mset close baidu           close connections matching keyword"
)]
struct Cli {
    /// Controller address, e.g. http://127.0.0.1:9090 (auto-detected from config file; env MIHOMO_API)
    #[arg(long, global = true)]
    api: Option<String>,
    /// API secret (env MIHOMO_SECRET)
    #[arg(long, global = true)]
    secret: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum TunState {
    On,
    Off,
}

#[derive(Subcommand)]
enum Cmd {
    /// Download Mihomo and create a local configuration (never overwrites files)
    Setup {
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
        /// Mihomo/Clash YAML subscription URL (not a base64 subscription)
        #[arg(long)]
        subscription: Option<String>,
        /// Official release tag, e.g. v1.19.34; default: latest stable
        #[arg(long)]
        release: Option<String>,
        #[arg(long, default_value_t = 7890, value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
    },
    /// Validate configuration and start Mihomo in the background
    Start {
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
        /// Seconds to wait for the controller API to become ready
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
    },
    /// Stop the background Mihomo managed by mset
    Stop {
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },
    /// Stop and restart the background Mihomo
    Restart {
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
        /// Seconds to wait for the new controller API to become ready
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
    },
    /// List proxy groups (or show a group/node detail)
    Proxies {
        group: Option<String>,
        /// Also list plain proxies
        #[arg(long)]
        all: bool,
    },
    /// Select a node; omit NODE for an interactive menu
    Use {
        node: Option<String>,
        /// Target proxy group (auto-detected when omitted)
        #[arg(short, long)]
        group: Option<String>,
    },
    /// Show mode, TUN/device status, and the current node of each group
    Status,
    /// Test a single node's delay
    Delay {
        node: String,
        #[arg(long, default_value = "http://www.gstatic.com/generate_204")]
        url: String,
        #[arg(long, default_value_t = 5000)]
        timeout: u64,
    },
    /// Test nodes by delay (default: PROXY; -g to choose another group)
    Test {
        #[arg(short, long, default_value = "PROXY")]
        group: Option<String>,
        #[arg(long, default_value = "http://www.gstatic.com/generate_204")]
        url: String,
        #[arg(long, default_value_t = 5000)]
        timeout: u64,
    },
    /// Test and select the fastest node (default: PROXY; -g for another group)
    Best {
        #[arg(short, long, default_value = "PROXY")]
        group: Option<String>,
        #[arg(long, default_value = "http://www.gstatic.com/generate_204")]
        url: String,
        #[arg(long, default_value_t = 5000)]
        timeout: u64,
    },
    /// Show or switch TUN at runtime (requires a privileged Mihomo process)
    Tun {
        #[arg(value_enum)]
        state: Option<TunState>,
        /// Local installation directory for automatic TUN authorization
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },
    /// Show or switch running mode (rule|global|direct)
    Mode { mode: Option<String> },
    /// Patch config: log-level | allow-lan | port | socks-port | mixed-port
    Set { key: String, value: String },
    /// List routing rules
    Rules,
    /// Show active connections
    Conns,
    /// Close connections (optionally filtered by keyword)
    Close { keyword: Option<String> },
    /// Reload config (optionally a specific path)
    Reload { path: Option<String> },
    /// Update Geo databases
    Geo,
    /// Stream live logs (Ctrl+C to stop)
    Logs {
        #[arg(default_value = "info")]
        level: String,
    },
    /// Show running config summary
    Config,
    /// Show mihomo version
    Version,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        errors::print(&e);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.cmd {
        Cmd::Setup {
            dir,
            subscription,
            release,
            port,
        } => {
            return install::setup(dir, subscription, release, port, cli.secret);
        }
        Cmd::Tun {
            state: Some(TunState::On),
            dir,
        } => {
            return tun::enable(dir, cli.api, cli.secret);
        }
        Cmd::Start { dir, timeout } => return daemon::start(dir, timeout),
        Cmd::Stop { dir } => return daemon::stop(dir),
        Cmd::Restart { dir, timeout } => return daemon::restart(dir, timeout),
        _ => {}
    }
    let controls = ui::Controls {
        api: cli.api.clone(),
        secret: cli.secret.clone(),
    };
    let c = cfg::resolve(cli.api, cli.secret)?;
    match cli.cmd {
        Cmd::Setup { .. } | Cmd::Start { .. } | Cmd::Stop { .. } | Cmd::Restart { .. } => {
            unreachable!()
        }
        Cmd::Proxies { group, all } => cmds::proxies(&c, group.as_deref(), all),
        Cmd::Use { node, group } => {
            cmds::use_node(&c, group.as_deref(), node.as_deref(), &controls)
        }
        Cmd::Status => cmds::status(&c),
        Cmd::Delay { node, url, timeout } => cmds::delay(&c, &node, &url, timeout),
        Cmd::Test {
            group,
            url,
            timeout,
        } => {
            cmds::test(&c, group.as_deref(), &url, timeout, false)?;
            tun::status(&c)?;
            println!("TUN: mset tun on/off | Mode: mset mode rule/global/direct");
            Ok(())
        }
        Cmd::Best {
            group,
            url,
            timeout,
        } => cmds::test(&c, group.as_deref(), &url, timeout, true),
        Cmd::Tun { state, dir } => {
            if let Some(dir) = dir {
                let path = install::directory(Some(dir))?.join("config.yaml");
                let c = cfg::client_for_file(&path, controls.secret.clone())?;
                return cmds::tun(&c, state.map(|s| matches!(s, TunState::On)));
            }
            cmds::tun(&c, state.map(|s| matches!(s, TunState::On)))
        }
        Cmd::Mode { mode } => cmds::mode(&c, mode.as_deref()),
        Cmd::Set { key, value } => cmds::set(&c, &key, &value),
        Cmd::Rules => cmds::rules(&c),
        Cmd::Conns => cmds::conns(&c),
        Cmd::Close { keyword } => cmds::close(&c, keyword.as_deref()),
        Cmd::Reload { path } => cmds::reload(&c, path.as_deref()),
        Cmd::Geo => cmds::geo(&c),
        Cmd::Logs { level } => cmds::logs(&c, &level),
        Cmd::Config => cmds::config(&c),
        Cmd::Version => cmds::version(&c),
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn test_and_best_default_to_proxy() {
        for command in ["test", "best"] {
            let cli = Cli::try_parse_from(["mset", command]).unwrap();
            let group = match cli.cmd {
                Cmd::Test { group, .. } | Cmd::Best { group, .. } => group,
                _ => unreachable!(),
            };
            assert_eq!(group.as_deref(), Some("PROXY"));
        }
    }

    #[test]
    fn test_and_best_allow_explicit_group() {
        for command in ["test", "best"] {
            let cli = Cli::try_parse_from(["mset", command, "-g", "Media"]).unwrap();
            let group = match cli.cmd {
                Cmd::Test { group, .. } | Cmd::Best { group, .. } => group,
                _ => unreachable!(),
            };
            assert_eq!(group.as_deref(), Some("Media"));
        }
    }
    #[test]
    fn tun_accepts_status_on_off_and_rejects_invalid_state() {
        assert!(matches!(
            Cli::try_parse_from(["mset", "tun"]).unwrap().cmd,
            Cmd::Tun { state: None, .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["mset", "tun", "on"]).unwrap().cmd,
            Cmd::Tun {
                state: Some(TunState::On),
                ..
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["mset", "tun", "off"]).unwrap().cmd,
            Cmd::Tun {
                state: Some(TunState::Off),
                ..
            }
        ));
        assert!(Cli::try_parse_from(["mset", "tun", "invalid"]).is_err());
    }
    #[test]
    fn duplicate_commands_are_removed_and_tun_on_keeps_custom_directory() {
        assert!(Cli::try_parse_from(["mset", "now"]).is_err());
        assert!(Cli::try_parse_from(["mset", "tun", "grant"]).is_err());
        let cli = Cli::try_parse_from(["mset", "tun", "on", "--dir", "./custom core"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Tun {
                state: Some(TunState::On),
                dir: Some(_)
            }
        ));
    }
    #[test]
    fn test_needs_no_interactive_flag() {
        assert!(matches!(
            Cli::try_parse_from(["mset", "test"]).unwrap().cmd,
            Cmd::Test { .. }
        ));
        assert!(Cli::try_parse_from(["mset", "test", "-i"]).is_err());
        assert!(Cli::try_parse_from(["mset", "test", "--interactive"]).is_err());
    }
}
