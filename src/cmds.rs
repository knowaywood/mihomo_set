use crate::api::Client;
use anyhow::{Result, anyhow, bail};
use colored::{ColoredString, Colorize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use unicode_width::UnicodeWidthStr;

fn pad(s: &str, w: usize) -> String {
    let len = s.width();
    if len >= w {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(w - len))
    }
}

fn lpad(s: &str, w: usize) -> String {
    let len = s.width();
    if len >= w {
        s.to_string()
    } else {
        format!("{}{}", " ".repeat(w - len), s)
    }
}

fn delay_text(d: u64) -> ColoredString {
    let s = format!("{}ms", d);
    match d {
        0 => s.as_str().dimmed(),
        1..=200 => s.as_str().green(),
        201..=500 => s.as_str().yellow(),
        _ => s.as_str().red(),
    }
}

fn display_delay(d: Option<u64>) -> ColoredString {
    match d {
        None => "-".dimmed(),
        Some(0) => "timeout".red(),
        Some(d) => delay_text(d),
    }
}

fn last_delay(info: Option<&Value>) -> Option<u64> {
    info?
        .get("history")?
        .as_array()?
        .last()?
        .get("delay")?
        .as_u64()
}

fn load_proxies(c: &Client) -> Result<(Map<String, Value>, HashMap<String, String>)> {
    let v = c.proxies()?;
    let mut map = v
        .get("proxies")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| anyhow!("unexpected response"))?;
    let mut providers: HashMap<String, String> = HashMap::new();
    if let Ok(pv) = c.providers()
        && let Some(list) = pv.get("providers").and_then(Value::as_object)
    {
        for (pname, p) in list {
            if let Some(nodes) = p.get("proxies").and_then(Value::as_array) {
                for node in nodes {
                    if node.get("all").is_some() {
                        continue;
                    }
                    if let Some(name) = node.get("name").and_then(Value::as_str)
                        && !map.contains_key(name)
                    {
                        map.insert(name.to_string(), node.clone());
                        providers.insert(name.to_string(), pname.clone());
                    }
                }
            }
        }
    }
    Ok((map, providers))
}

fn group_nodes(info: &Value) -> Vec<&str> {
    info.get("all")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{}B", b)
    } else {
        format!("{:.1}{}", v, UNITS[i])
    }
}

pub fn proxies(c: &Client, group: Option<&str>, all: bool) -> Result<()> {
    match group {
        Some(g) => show_group(c, g),
        None => list_groups(c, all),
    }
}

fn list_groups(c: &Client, all: bool) -> Result<()> {
    let (map, _) = load_proxies(c)?;
    let mut any = false;
    for (name, info) in &map {
        if info.get("all").is_none() {
            continue;
        }
        any = true;
        let typ = info.get("type").and_then(Value::as_str).unwrap_or("");
        let now = info.get("now").and_then(Value::as_str).unwrap_or("-");
        let count = info
            .get("all")
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0);
        let d = last_delay(map.get(now));
        println!(
            "{} [{}] {:>3} nodes  now: {}  {}",
            pad(name, 24),
            typ,
            count,
            now.cyan(),
            display_delay(d)
        );
    }
    if !any {
        println!("no proxy groups found");
    }
    if all {
        let mut has_leaf = false;
        for (name, info) in &map {
            if info.get("all").is_some() {
                continue;
            }
            if !has_leaf {
                println!();
                has_leaf = true;
            }
            let typ = info.get("type").and_then(Value::as_str).unwrap_or("");
            let d = last_delay(Some(info));
            println!("{} [{}] {}", pad(name, 24), typ, display_delay(d));
        }
    }
    Ok(())
}

fn show_group(c: &Client, g: &str) -> Result<()> {
    let (map, _) = load_proxies(c)?;
    let info = map
        .get(g)
        .ok_or_else(|| anyhow!("not found: {} (run `mset proxies` to list)", g))?;
    let typ = info.get("type").and_then(Value::as_str).unwrap_or("");
    let now = info.get("now").and_then(Value::as_str).unwrap_or("");
    match info.get("all").and_then(Value::as_array) {
        Some(_) => {
            println!("{} [{}]  now: {}", g, typ, now.cyan());
            let nodes = group_nodes(info);
            let w = nodes.iter().map(|s| s.width()).max().unwrap_or(0).min(48);
            for (i, n) in nodes.iter().enumerate() {
                let mark = if *n == now { "*".cyan() } else { " ".normal() };
                let d = last_delay(map.get(*n));
                println!("{}{:>3}. {} {}", mark, i + 1, pad(n, w), display_delay(d));
            }
        }
        None => {
            let udp = info.get("udp").and_then(Value::as_bool).unwrap_or(false);
            let d = last_delay(Some(info));
            println!("{} [{}]  UDP: {}  delay: {}", g, typ, udp, display_delay(d));
        }
    }
    Ok(())
}

fn resolve_target(nodes: &[&str], node: &str, gname: &str) -> Result<String> {
    if let Ok(idx) = node.parse::<usize>() {
        if idx == 0 || idx > nodes.len() {
            bail!("index out of range, {} has {} nodes", gname, nodes.len());
        }
        return Ok(nodes[idx - 1].to_string());
    }
    if !nodes.contains(&node) {
        bail!(
            "node {} not in group {}, run `mset proxies {}` to list",
            node,
            gname,
            gname
        );
    }
    Ok(node.to_string())
}

fn selector_groups<'a>(map: &'a Map<String, Value>, node: Option<&str>) -> Vec<&'a str> {
    map.iter()
        .filter(|(_, info)| {
            info.get("type").and_then(Value::as_str) == Some("Selector")
                && node.is_none_or(|n| group_nodes(info).contains(&n))
        })
        .map(|(name, _)| name.as_str())
        .collect()
}

fn default_group<'a>(groups: &[&'a str]) -> Option<&'a str> {
    if groups.contains(&"PROXY") {
        Some("PROXY")
    } else if groups.len() == 1 {
        Some(groups[0])
    } else {
        None
    }
}

fn prompt_choice(label: &str, choices: &[&str]) -> Result<String> {
    if choices.is_empty() {
        bail!("no available {label}");
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "interactive selection requires a terminal; specify a node name and, if needed, -g <group>"
        );
    }
    for (i, name) in choices.iter().enumerate() {
        println!("{:>3}. {}", i + 1, name);
    }
    loop {
        print!("Select {label} [1-{}], q to cancel: ", choices.len());
        std::io::stdout().flush()?;
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input)? == 0 || input.trim() == "q" {
            bail!("selection cancelled");
        }
        if let Ok(i) = input.trim().parse::<usize>()
            && i > 0
            && i <= choices.len()
        {
            return Ok(choices[i - 1].to_string());
        }
        println!("Please enter a number from the list.");
    }
}

fn smart_select(c: &Client, map: &Map<String, Value>, node: &str) -> Result<()> {
    let groups = selector_groups(map, Some(node));
    let group = default_group(&groups).ok_or_else(|| {
        anyhow!(
            "cannot uniquely select a group for {node}; use -g <group> (candidates: {})",
            groups.join(", ")
        )
    })?;
    c.select(group, node)?;
    println!("{} → {}", group, node.green().bold());
    // In global mode, also point GLOBAL at the selected group when it is a member.
    if group != "GLOBAL"
        && map
            .get("GLOBAL")
            .is_some_and(|g| group_nodes(g).contains(&group))
    {
        c.select("GLOBAL", group)?;
        println!("GLOBAL → {}", group.green().bold());
    }
    Ok(())
}

pub fn use_node(c: &Client, group: Option<&str>, node: Option<&str>) -> Result<()> {
    let (map, _) = load_proxies(c)?;
    let Some(node) = node else {
        let groups = selector_groups(&map, None);
        let selected_group = match group.or_else(|| default_group(&groups)) {
            Some(g) => g.to_string(),
            None => prompt_choice("group", &groups)?,
        };
        let info = map
            .get(&selected_group)
            .ok_or_else(|| anyhow!("group not found: {selected_group}"))?;
        if info.get("type").and_then(Value::as_str) != Some("Selector") {
            bail!("{selected_group} does not support manual selection");
        }
        println!(
            "{} (current: {})",
            selected_group,
            info["now"].as_str().unwrap_or("-")
        );
        let target = prompt_choice("node", &group_nodes(info))?;
        if group.is_some() {
            c.select(&selected_group, &target)?;
            println!("{selected_group} → {target}");
        } else {
            // Preserve the group chosen in the menu, including for shared nodes.
            c.select(&selected_group, &target)?;
            println!("{selected_group} → {target}");
            if selected_group != "GLOBAL"
                && map
                    .get("GLOBAL")
                    .is_some_and(|g| group_nodes(g).contains(&selected_group.as_str()))
            {
                c.select("GLOBAL", &selected_group)?;
            }
        }
        return Ok(());
    };
    match group {
        Some(gname) => {
            let info = map.get(gname).ok_or_else(|| {
                anyhow!(
                    "proxy group not found: {} (run `mset proxies` to list all)",
                    gname
                )
            })?;
            let nodes = group_nodes(info);
            if nodes.is_empty() {
                bail!("{} is not a proxy group", gname);
            }
            let target = resolve_target(&nodes, node, gname)?;
            c.select(gname, &target)?;
            println!("{} \u{2192} {}", gname, target.as_str().green().bold());
        }
        None => {
            if let Ok(idx) = node.parse::<usize>() {
                let ranking = crate::state::load_ranking().ok_or_else(|| {
                    anyhow!("no saved test ranking; run `mset test` first, or use a node name")
                })?;
                if idx == 0 || idx > ranking.len() {
                    bail!(
                        "index out of range, last test ranked {} nodes",
                        ranking.len()
                    );
                }
                let name = &ranking[idx - 1];
                println!("{} \u{2192} {}", idx, name);
                smart_select(c, &map, name)?;
            } else {
                smart_select(c, &map, node)?;
            }
        }
    }
    Ok(())
}

pub fn current(c: &Client) -> Result<()> {
    let (map, _) = load_proxies(c)?;
    for (name, info) in &map {
        if info.get("all").is_none() {
            continue;
        }
        let now = info.get("now").and_then(Value::as_str).unwrap_or("-");
        let d = last_delay(map.get(now));
        println!("{} → {}  {}", pad(name, 24), now.cyan(), display_delay(d));
    }
    Ok(())
}

pub fn delay(c: &Client, node: &str, url: &str, timeout: u64) -> Result<()> {
    let (map, providers) = load_proxies(c)?;
    if !map.contains_key(node) {
        bail!("proxy not found: {} (run `mset proxies` to list)", node);
    }
    let d = c.delay(node, providers.get(node).map(String::as_str), url, timeout)?;
    println!("{} {}", pad(node, 24), delay_text(d));
    Ok(())
}

const BUILTIN: [&str; 5] = ["DIRECT", "REJECT", "REJECT-DROP", "PASS", "COMPATIBLE"];

pub fn test(c: &Client, group: Option<&str>, url: &str, timeout: u64, best: bool) -> Result<()> {
    let (map, providers) = load_proxies(c)?;
    let Some(gname) = group else {
        let leaves: Vec<(String, Option<String>)> = map
            .iter()
            .filter(|(_, info)| info.get("all").is_none())
            .filter(|(n, _)| !BUILTIN.contains(&n.as_str()))
            .map(|(n, _)| (n.clone(), providers.get(n).cloned()))
            .collect();
        let now = map
            .get("GLOBAL")
            .and_then(|g| g.get("now"))
            .and_then(Value::as_str)
            .unwrap_or("");
        println!("testing all proxies ({} nodes) ...", leaves.len());
        let list = c.all_delays(&leaves, url, timeout);
        let w = list
            .iter()
            .map(|(n, _)| n.width())
            .max()
            .unwrap_or(0)
            .min(48);
        for (i, (n, d)) in list.iter().enumerate() {
            let mark = if n == now { "*".cyan() } else { " ".normal() };
            println!("{}{:>3}. {} {}", mark, i + 1, pad(n, w), display_delay(*d));
        }
        let ranking: Vec<String> = list.iter().map(|(n, _)| n.clone()).collect();
        crate::state::save_ranking(&ranking)?;
        println!("(switch with: mset use <index>)");
        if best {
            let winner = list
                .iter()
                .find(|(n, d)| d.is_some() && !BUILTIN.contains(&n.as_str()))
                .map(|(n, _)| n.clone())
                .ok_or_else(|| anyhow!("all nodes failed the test"))?;
            println!("fastest: {}", winner);
            smart_select(c, &map, &winner)?;
        }
        return Ok(());
    };
    let info = map.get(gname).ok_or_else(|| {
        anyhow!(
            "proxy group not found: {} (run `mset proxies` to list)",
            gname
        )
    })?;
    let nodes = group_nodes(info);
    let now = info.get("now").and_then(Value::as_str).unwrap_or("");
    println!("testing {} ({} nodes) ...", gname, nodes.len());
    let list = c.group_delay(gname, url, timeout)?;
    let w = list
        .iter()
        .map(|(n, _)| n.width())
        .max()
        .unwrap_or(0)
        .min(48);
    for (i, (n, d)) in list.iter().enumerate() {
        let mark = if n == now { "*".cyan() } else { " ".normal() };
        println!("{}{:>3}. {} {}", mark, i + 1, pad(n, w), delay_text(*d));
    }
    let failed = nodes
        .iter()
        .filter(|n| !list.iter().any(|(x, _)| x == **n))
        .count();
    if failed > 0 {
        println!("({} nodes timed out/failed)", failed);
    }
    let ranking: Vec<String> = list.iter().map(|(n, _)| n.clone()).collect();
    crate::state::save_ranking(&ranking)?;
    println!("(switch with: mset use <index>)");
    if best {
        let winner = list
            .iter()
            .map(|(n, _)| n)
            .find(|n| !BUILTIN.contains(&n.as_str()))
            .ok_or_else(|| anyhow!("no successful node in the test"))?;
        println!("fastest: {}", winner);
        c.select(gname, winner)?;
        println!("{} → {}", gname, winner.green().bold());
    }
    Ok(())
}

pub fn mode(c: &Client, m: Option<&str>) -> Result<()> {
    match m {
        None => {
            let v = c.configs()?;
            let cur = v.get("mode").and_then(Value::as_str).unwrap_or("?");
            println!("current mode: {}", cur);
        }
        Some(m) => {
            let m = m.to_lowercase();
            if !matches!(m.as_str(), "rule" | "global" | "direct") {
                bail!("mode must be rule / global / direct");
            }
            c.patch_configs(json!({ "mode": m }))?;
            println!("mode switched to {}", m.as_str().green().bold());
        }
    }
    Ok(())
}

fn parse_bool(s: &str) -> Result<bool> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => bail!("expected boolean: true/false/1/0/yes/no/on/off"),
    }
}

pub fn set(c: &Client, key: &str, value: &str) -> Result<()> {
    let key = key.to_lowercase();
    let body = match key.as_str() {
        "mode" => {
            let m = value.to_lowercase();
            if !matches!(m.as_str(), "rule" | "global" | "direct") {
                bail!("mode must be rule / global / direct");
            }
            json!({ "mode": m })
        }
        "log-level" => {
            let m = value.to_lowercase();
            if !matches!(
                m.as_str(),
                "debug" | "info" | "warning" | "error" | "silent"
            ) {
                bail!("log-level must be debug / info / warning / error / silent");
            }
            json!({ "log-level": m })
        }
        "allow-lan" => json!({ "allow-lan": parse_bool(value)? }),
        "port" | "socks-port" | "mixed-port" | "redir-port" | "tproxy-port" => {
            let p: u16 = value
                .parse()
                .map_err(|_| anyhow!("port must be a number 0-65535"))?;
            let mut o = Map::new();
            o.insert(key.clone(), json!(p));
            Value::Object(o)
        }
        _ => bail!(
            "unsupported key: {}\nsupported: mode, log-level, allow-lan, port, socks-port, mixed-port, redir-port, tproxy-port",
            key
        ),
    };
    c.patch_configs(body)?;
    println!("set {} = {}", key.as_str().green(), value);
    Ok(())
}

pub fn rules(c: &Client) -> Result<()> {
    let v = c.rules()?;
    let list = v
        .get("rules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let w = list
        .iter()
        .map(|r| {
            r.get("payload")
                .and_then(Value::as_str)
                .unwrap_or("")
                .width()
        })
        .max()
        .unwrap_or(0)
        .min(40);
    for r in &list {
        let t = r.get("type").and_then(Value::as_str).unwrap_or("");
        let p = r.get("payload").and_then(Value::as_str).unwrap_or("");
        let tgt = r.get("proxy").and_then(Value::as_str).unwrap_or("");
        println!("{} {} → {}", pad(t, 14), pad(p, w), tgt.cyan());
    }
    println!("{} rules total", list.len());
    Ok(())
}

fn conn_dest(m: &Value) -> String {
    let host = m.get("host").and_then(Value::as_str).unwrap_or("");
    let ip = m.get("destinationIP").and_then(Value::as_str).unwrap_or("");
    let port = m
        .get("destinationPort")
        .and_then(Value::as_str)
        .unwrap_or("");
    if host.is_empty() {
        format!("{}:{}", ip, port)
    } else {
        format!("{}:{}", host, port)
    }
}

fn conn_chain(conn: &Value) -> String {
    conn.get("chains")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .rev()
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default()
}

pub fn conns(c: &Client) -> Result<()> {
    let v = c.conns()?;
    let up = v.get("uploadTotal").and_then(Value::as_u64).unwrap_or(0);
    let down = v.get("downloadTotal").and_then(Value::as_u64).unwrap_or(0);
    let list = v
        .get("connections")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    println!(
        "connections: {}   upload total: {}   download total: {}",
        list.len(),
        fmt_bytes(up),
        fmt_bytes(down)
    );
    println!(
        "{} {} {} {} {} {} {}",
        pad("NET", 4),
        pad("SOURCE", 22),
        pad("DEST", 34),
        pad("CHAIN", 20),
        pad("RULE", 16),
        lpad("UP", 9),
        lpad("DOWN", 9)
    );
    for conn in &list {
        let m = conn.get("metadata").cloned().unwrap_or_default();
        let net = m.get("network").and_then(Value::as_str).unwrap_or("");
        let src = format!(
            "{}:{}",
            m.get("sourceIP").and_then(Value::as_str).unwrap_or(""),
            m.get("sourcePort").and_then(Value::as_str).unwrap_or("")
        );
        let rule = conn
            .get("rulePayload")
            .and_then(Value::as_str)
            .unwrap_or("");
        let up = conn.get("upload").and_then(Value::as_u64).unwrap_or(0);
        let down = conn.get("download").and_then(Value::as_u64).unwrap_or(0);
        println!(
            "{} {} {} {} {} {} {}",
            pad(net, 4),
            pad(&src, 22),
            pad(&conn_dest(&m), 34),
            pad(&conn_chain(conn), 20),
            pad(rule, 16),
            lpad(&fmt_bytes(up), 9),
            lpad(&fmt_bytes(down), 9)
        );
    }
    Ok(())
}

pub fn close(c: &Client, kw: Option<&str>) -> Result<()> {
    match kw {
        None => {
            c.close_all_conns()?;
            println!("all connections closed");
        }
        Some(kw) => {
            let v = c.conns()?;
            let list = v
                .get("connections")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let k = kw.to_lowercase();
            let mut n = 0;
            for conn in &list {
                let m = conn.get("metadata").cloned().unwrap_or_default();
                let hay = format!(
                    "{} {} {} {}",
                    m.get("host").and_then(Value::as_str).unwrap_or(""),
                    m.get("destinationIP").and_then(Value::as_str).unwrap_or(""),
                    m.get("process").and_then(Value::as_str).unwrap_or(""),
                    conn_chain(conn)
                )
                .to_lowercase();
                if hay.contains(&k)
                    && let Some(id) = conn.get("id").and_then(Value::as_str)
                {
                    c.close_conn(id)?;
                    n += 1;
                }
            }
            println!("closed {} connections", n);
        }
    }
    Ok(())
}

pub fn reload(c: &Client, path: Option<&str>) -> Result<()> {
    c.reload(path.unwrap_or(""))?;
    println!("config reloaded");
    Ok(())
}

pub fn geo(c: &Client) -> Result<()> {
    c.update_geo()?;
    println!("Geo databases updated");
    Ok(())
}

pub fn logs(c: &Client, level: &str) -> Result<()> {
    println!("live logs (level={}, Ctrl+C to stop)", level);
    let reader = c.stream(&format!("/logs?level={}", level))?;
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
            let t = v.get("type").and_then(Value::as_str).unwrap_or("log");
            let p = v.get("payload").and_then(Value::as_str).unwrap_or("");
            let tag = match t {
                "error" => t.red(),
                "warning" => t.yellow(),
                "info" => t.cyan(),
                _ => t.normal(),
            };
            println!("[{}] {}", tag, p);
        }
    }
    Ok(())
}

pub fn config(c: &Client) -> Result<()> {
    let v = c.configs()?;
    for key in [
        "port",
        "socks-port",
        "mixed-port",
        "redir-port",
        "tproxy-port",
        "allow-lan",
        "mode",
        "log-level",
        "ipv6",
    ] {
        if let Some(val) = v.get(key)
            && !val.is_null()
        {
            println!("{}: {}", key, val);
        }
    }
    Ok(())
}

pub fn version(c: &Client) -> Result<()> {
    let v = c.version()?;
    let ver = v.get("version").and_then(Value::as_str).unwrap_or("?");
    println!("mihomo {}", ver);
    Ok(())
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn detects_selectable_groups_without_global() {
        let map = json!({
            "PROXY": {"type": "Selector", "all": ["HK", "DIRECT"]},
            "AUTO": {"type": "URLTest", "all": ["HK"]}
        })
        .as_object()
        .unwrap()
        .clone();
        let groups = selector_groups(&map, Some("HK"));
        assert_eq!(default_group(&groups), Some("PROXY"));
        assert!(selector_groups(&map, Some("missing")).is_empty());
    }

    #[test]
    fn avoids_guessing_between_unrelated_groups() {
        assert_eq!(default_group(&["Media", "Work"]), None);
        assert_eq!(default_group(&["Work"]), Some("Work"));
        assert_eq!(default_group(&["GLOBAL", "PROXY"]), Some("PROXY"));
    }
}
