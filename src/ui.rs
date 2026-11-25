use anyhow::{Result, bail};
use std::io::{IsTerminal, Write};

use crate::{api::Client, cfg, cmds, install, tun};

pub struct Controls {
    pub api: Option<String>,
    pub secret: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Action {
    Node(usize),
    Tun,
    Mode(&'static str),
    Quit,
}

fn parse_action(input: &str, count: usize) -> Option<Action> {
    match input.trim().to_ascii_lowercase().as_str() {
        "t" => Some(Action::Tun),
        "r" => Some(Action::Mode("rule")),
        "g" => Some(Action::Mode("global")),
        "d" => Some(Action::Mode("direct")),
        "q" => Some(Action::Quit),
        value => value
            .parse::<usize>()
            .ok()
            .filter(|i| *i > 0 && *i <= count)
            .map(|i| Action::Node(i - 1)),
    }
}

pub fn require_terminal() -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("interactive controls require a terminal; specify a node name instead");
    }
    Ok(())
}

impl Controls {
    fn enable_tun(&self, c: &Client) -> Result<()> {
        // A controller discovered in the working directory may differ from the managed core.
        // Only use automatic local authorization when its endpoint matches the managed config.
        let managed = install::directory(None)
            .ok()
            .and_then(|d| cfg::read_controller(&d.join("config.yaml")).ok());
        if self.api.is_none()
            && managed
                .as_ref()
                .is_some_and(|(base, _)| base == c.base_url())
        {
            tun::enable(None, None, self.secret.clone())
        } else {
            cmds::tun(c, Some(true))
        }
    }
}

/// Node indices refer to the displayed list; settings can be changed without leaving it.
pub fn choose_node(
    c: &Client,
    controls: &Controls,
    nodes: &[&str],
    show_nodes: bool,
) -> Result<Option<String>> {
    require_terminal()?;
    if show_nodes {
        for (i, node) in nodes.iter().enumerate() {
            println!("{:>3}. {node}", i + 1);
        }
    }
    loop {
        tun::status(c)?;
        println!("[t] Toggle TUN  [r] Rule  [g] Global  [d] Direct  [q] Exit");
        print!("Choose node [1-{}] or action: ", nodes.len());
        std::io::stdout().flush()?;
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input)? == 0 {
            return Ok(None);
        }
        let result = match parse_action(&input, nodes.len()) {
            Some(Action::Node(i)) => return Ok(Some(nodes[i].to_string())),
            Some(Action::Quit) => return Ok(None),
            Some(Action::Mode(mode)) => cmds::mode(c, Some(mode)),
            Some(Action::Tun) => {
                let config = c.configs()?;
                match config["tun"]["enable"].as_bool() {
                    Some(true) => cmds::tun(c, Some(false)),
                    Some(false) => controls.enable_tun(c),
                    None => {
                        println!("Controller did not report TUN support.");
                        continue;
                    }
                }
            }
            None => {
                println!("Enter a node number, t, r, g, d, or q.");
                continue;
            }
        };
        if let Err(error) = result {
            eprintln!("Action failed: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_distinguishes_nodes_tun_and_routing_modes() {
        assert_eq!(parse_action("2", 3), Some(Action::Node(1)));
        assert_eq!(parse_action(" T ", 3), Some(Action::Tun));
        assert_eq!(parse_action("r", 3), Some(Action::Mode("rule")));
        assert_eq!(parse_action("g", 3), Some(Action::Mode("global")));
        assert_eq!(parse_action("d", 3), Some(Action::Mode("direct")));
        assert_eq!(parse_action("q", 3), Some(Action::Quit));
        for input in ["0", "4", "-1", "tun", ""] {
            assert_eq!(parse_action(input, 3), None);
        }
    }
}
