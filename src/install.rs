use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) fn directory(dir: Option<PathBuf>) -> Result<PathBuf> {
    let path = match dir {
        Some(p) => p,
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is unset; use --dir")?)
            .join(".config/mihomo"),
    };
    Ok(if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    })
}

fn platform(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("linux-amd64-compatible"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        ("macos", "x86_64") => Ok("darwin-amd64-compatible"),
        ("macos", "aarch64") => Ok("darwin-arm64"),
        _ => bail!("automatic installation does not support {os}/{arch}"),
    }
}

fn config(subscription: Option<&str>, port: u16, secret: &str) -> Result<String> {
    if port == 9090 {
        bail!("proxy port must differ from controller port 9090");
    }
    let mut text = format!(
        "mixed-port: {port}\nallow-lan: false\nmode: rule\nlog-level: info\nprofile:\n  store-selected: true\nexternal-controller: 127.0.0.1:9090\nsecret: {}\n",
        serde_json::to_string(secret)?
    );
    if let Some(url) = subscription {
        if !url.starts_with("https://") && !url.starts_with("http://") {
            bail!("subscription must be an HTTP(S) Mihomo/Clash YAML URL");
        }
        text.push_str(&format!("proxy-providers:\n  subscription:\n    type: http\n    url: {}\n    path: ./providers/subscription.yaml\n    interval: 3600\nproxy-groups:\n  - name: PROXY\n    type: select\n    use: [subscription]\n    proxies: [DIRECT]\nrules:\n  - MATCH,PROXY\n", serde_json::to_string(url)?));
    } else {
        text.push_str("proxies: []\nrules:\n  - MATCH,DIRECT\n");
    }
    Ok(text)
}

// create_new prevents clobbering an existing installation or configuration.
fn write_new(path: &Path, data: &[u8], executable: bool) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if executable { 0o700 } else { 0o600 });
    }
    #[cfg(not(unix))]
    let _ = executable;
    let mut file = options.open(path).with_context(|| {
        format!(
            "cannot create {} (existing files are preserved)",
            path.display()
        )
    })?;
    if let Err(e) = file.write_all(data).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(e.into());
    }
    Ok(())
}

pub fn setup(
    dir: Option<PathBuf>,
    subscription: Option<String>,
    release: Option<String>,
    port: u16,
    secret: Option<String>,
) -> Result<()> {
    let target = platform(std::env::consts::OS, std::env::consts::ARCH)?;
    let dir = directory(dir)?;
    let secret = secret
        .or_else(|| std::env::var("MIHOMO_SECRET").ok())
        .unwrap_or_default();
    let text = config(subscription.as_deref(), port, &secret)?;
    let binary = dir.join("mihomo");
    let cfg = dir.join("config.yaml");
    if binary.try_exists()? || cfg.try_exists()? {
        bail!(
            "{} already contains mihomo or config.yaml; use a different --dir; nothing was overwritten",
            dir.display()
        );
    }
    let endpoint = match release {
        Some(tag) => format!(
            "https://api.github.com/repos/MetaCubeX/mihomo/releases/tags/{}",
            urlencoding::encode(&tag)
        ),
        None => "https://api.github.com/repos/MetaCubeX/mihomo/releases/latest".into(),
    };
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build();
    println!("Looking up official Mihomo release for {target}...");
    let info: Value = agent
        .get(&endpoint)
        .set("User-Agent", "mset")
        .call()
        .context("cannot query GitHub release; check network connectivity")?
        .into_json()?;
    let tag = info["tag_name"].as_str().context("release has no tag")?;
    let name = format!("mihomo-{target}-{tag}.gz");
    let asset = info["assets"]
        .as_array()
        .context("release has no assets")?
        .iter()
        .find(|a| a["name"].as_str() == Some(&name))
        .with_context(|| format!("release has no {name}"))?;
    let url = asset["browser_download_url"]
        .as_str()
        .context("asset has no URL")?;
    if !url.starts_with("https://github.com/MetaCubeX/mihomo/releases/download/") {
        bail!("unexpected official download URL");
    }
    println!("Downloading {name}...");
    let response = agent.get(url).call().context("failed to download Mihomo")?;
    let mut bytes = Vec::new();
    GzDecoder::new(response.into_reader())
        .take(256 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .context("invalid or incomplete gzip archive")?;
    if bytes.is_empty() || bytes.len() > 256 * 1024 * 1024 {
        bail!("invalid binary size");
    }
    fs::create_dir_all(&dir)?;
    write_new(&binary, &bytes, true)?;
    if let Err(e) = write_new(&cfg, text.as_bytes(), false) {
        let _ = fs::remove_file(&binary);
        return Err(e);
    }
    println!("Installed {tag} in {}", dir.display());
    println!("Start with: mset start --dir {:?}", dir);
    if subscription.is_none() {
        println!("No subscription supplied: configuration uses DIRECT only.");
    }
    Ok(())
}

/// Authorize only the selected Mihomo binary; never run a shell or elevate mset itself.
pub(crate) fn grant_tun(dir: Option<PathBuf>) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        let binary = directory(dir)?
            .join("mihomo")
            .canonicalize()
            .context("Mihomo is not installed in this directory; run mset setup first")?;
        if !binary.is_file() {
            bail!("Mihomo binary is not a regular file");
        }
        // Resolve HOME as the invoking user before asking sudo to run setcap.
        let mut command = if unsafe { libc::geteuid() } == 0 {
            Command::new("setcap")
        } else {
            let mut command = Command::new("sudo");
            command.arg("--").arg("setcap");
            command
        };
        println!(
            "Granting TUN permissions to {} (sudo may ask for your password)...",
            binary.display()
        );
        let status = command.arg("cap_net_admin,cap_net_raw=+ep").arg(&binary).status()
            .context("cannot run setcap; ensure setcap (libcap2-bin on Debian/Ubuntu) and sudo are installed")?;
        if !status.success() {
            bail!(
                "TUN authorization failed ({status}); check sudo permissions and filesystem support for file capabilities"
            );
        }
        println!("TUN permissions granted.");
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = dir;
        bail!(
            "automatic TUN authorization supports Linux only; on macOS the core must be started with administrator privileges"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn platform_selection() {
        assert_eq!(
            platform("linux", "x86_64").unwrap(),
            "linux-amd64-compatible"
        );
        assert_eq!(platform("macos", "aarch64").unwrap(), "darwin-arm64");
        assert!(platform("windows", "x86_64").is_err());
    }
    #[test]
    fn subscription_escaping_and_ports() {
        let text = config(
            Some("https://example.org/sub?token=\"x\"\nother"),
            7890,
            "a\nb",
        )
        .unwrap();
        assert!(text.contains("secret: \"a\\nb\""));
        assert!(text.contains("\\\"x\\\"\\nother"));
        assert!(config(Some("file:///tmp/sub"), 7890, "").is_err());
        assert!(config(None, 9090, "").is_err());
        assert!(config(None, 7890, "").unwrap().contains("MATCH,DIRECT"));
    }
    #[test]
    fn existing_files_are_preserved() {
        let path = std::env::temp_dir().join(format!("mset-preserve-{}", std::process::id()));
        write_new(&path, b"original", false).unwrap();
        assert!(write_new(&path, b"replacement", false).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        fs::remove_file(path).unwrap();
    }
}
