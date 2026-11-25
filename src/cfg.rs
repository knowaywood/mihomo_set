use std::path::PathBuf;

pub fn resolve(api: Option<String>, secret: Option<String>) -> (String, Option<String>) {
    let mut base = api.or_else(|| std::env::var("MIHOMO_API").ok());
    let mut secret = secret.or_else(|| std::env::var("MIHOMO_SECRET").ok());
    if (base.is_none() || secret.is_none())
        && let Some((b, s)) = from_config()
    {
        base = base.or(Some(b));
        secret = secret.or(s);
    }
    (
        normalize(&base.unwrap_or_else(|| "127.0.0.1:9090".into())),
        secret,
    )
}

fn from_config() -> Option<(String, Option<String>)> {
    for path in candidates() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut ec = None;
        let mut secret = None;
        for line in text.lines() {
            let t = line.trim();
            if let Some(v) = t.strip_prefix("external-controller:") {
                ec = Some(unquote(v));
            } else if let Some(v) = t.strip_prefix("secret:") {
                let s = unquote(v);
                secret = if s.is_empty() { None } else { Some(s) };
            }
        }
        if let Some(b) = ec
            && !b.is_empty()
        {
            return Some((b, secret));
        }
    }
    None
}

fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("MIHOMO_CONFIG") {
        v.push(PathBuf::from(p));
    }
    v.push(PathBuf::from("config.yaml"));
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        v.push(home.join(".config/mihomo/config.yaml"));
        v.push(home.join(".config/mihomo/config.yml"));
        v.push(home.join(".config/clash/config.yaml"));
        v.push(home.join(".config/clash/config.yml"));
    }
    v.push(PathBuf::from("/etc/mihomo/config.yaml"));
    v.push(PathBuf::from("/etc/mihomo/config.yml"));
    v.push(PathBuf::from("/etc/clash/config.yaml"));
    v
}

fn unquote(s: &str) -> String {
    let s = s.trim().trim_end_matches(',').trim();
    s.trim_matches('"').trim_matches('\'').to_string()
}

fn normalize(base: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    let full = if b.starts_with("http://") || b.starts_with("https://") {
        b.to_string()
    } else {
        format!("http://{}", b)
    };
    let (scheme, rest) = full.split_once("://").unwrap_or(("http", full.as_str()));
    let mut host = rest.split('/').next().unwrap_or("").to_string();
    if host.is_empty() {
        host = "127.0.0.1:9090".to_string();
    }
    if host.starts_with(':') {
        host = format!("127.0.0.1{}", host);
    }
    host = host.replace("0.0.0.0", "127.0.0.1");
    format!("{}://{}", scheme, host)
}
