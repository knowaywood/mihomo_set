use anyhow::{Context, Result, bail};
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::{
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
use crate::daemon;
use crate::{api::Client, cfg, cmds, install};

pub fn enable(dir: Option<PathBuf>, api: Option<String>, secret: Option<String>) -> Result<()> {
    // An explicit endpoint/config may belong to another machine or supervisor.
    // Never authorize or restart a local binary for such a controller.
    if api.is_some()
        || std::env::var_os("MIHOMO_API").is_some()
        || (dir.is_none() && std::env::var_os("MIHOMO_CONFIG").is_some())
    {
        let c = cfg::resolve(api, secret)?;
        return enable_running(&c);
    }
    let explicit_dir = dir.is_some();
    let dir = install::directory(dir)?;
    let path = dir.join("config.yaml");
    let c = if explicit_dir {
        cfg::client_for_file(&path, secret)?
    } else {
        cfg::resolve(None, secret)?
    };
    let managed_config = path
        .canonicalize()
        .ok()
        .is_some_and(|path| c.config_path() == Some(path.as_path()));
    let local = c.is_local() && managed_config;
    // Authentication, network, and protocol errors must not trigger privilege escalation.
    let config = c.configs()?;
    if is_enabled(&config) && device_is_ready_or_unverifiable(&c, &config) {
        return enable_running(&c);
    }
    if !local {
        return enable_running(&c);
    }
    #[cfg(not(target_os = "linux"))]
    {
        return enable_running(&c);
    }
    #[cfg(target_os = "linux")]
    {
        daemon::prepare_tun(Some(dir))?;
        // Background spawn completion does not imply that the REST API is ready.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("Mihomo API startup timed out; inspect mihomo.log");
            }
            match c.configs_timeout(remaining.min(Duration::from_secs(2))) {
                Ok(_) => break,
                Err(e) if !crate::errors::retryable(&e) || Instant::now() >= deadline => {
                    return Err(e).context("Mihomo API did not become ready after restart");
                }
                Err(_) => thread::sleep(Duration::from_millis(200)),
            }
        }
        enable_running(&c)
    }
}

fn is_enabled(config: &serde_json::Value) -> bool {
    config
        .get("tun")
        .and_then(|t| t.get("enable"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

fn enable_running(c: &Client) -> Result<()> {
    let config = c.configs()?;
    if is_enabled(&config) && device_is_ready_or_unverifiable(c, &config) {
        c.persist_patch(&serde_json::json!({"tun":{"enable":true}}))?;
        print_status(c, &config);
        return Ok(());
    }
    if config.get("tun").is_none() {
        bail!("this controller does not report TUN support");
    }
    if is_enabled(&config) {
        println!("TUN is configured on but the local device is missing or down; recreating it...");
        c.patch_configs_runtime(serde_json::json!({"tun":{"enable":false}}))?;
    }
    cmds::tun(c, Some(true))
}

fn device_is_ready_or_unverifiable(c: &Client, config: &serde_json::Value) -> bool {
    if !c.is_local() {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        local_device_ready(config, std::path::Path::new("/sys/class/net"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        true
    }
}

#[cfg(target_os = "linux")]
fn local_device_ready(config: &serde_json::Value, interfaces: &std::path::Path) -> bool {
    let Some(device) = config["tun"]["device"].as_str().filter(|d| !d.is_empty()) else {
        return false;
    };
    if device == "." || device == ".." || device.contains('/') || device.contains('\\') {
        return false;
    }
    let path = interfaces.join(device);
    let flags = |name| {
        std::fs::read_to_string(path.join(name))
            .ok()
            .and_then(|f| parse_flags(&f))
    };
    flags("tun_flags").is_some_and(|f| f & 1 != 0) && flags("flags").is_some_and(|f| f & 1 != 0)
}

/// Verify local Linux device creation before reporting/persisting success.
pub(crate) fn verify_enabled(c: &Client, config: &serde_json::Value) -> Result<serde_json::Value> {
    if !is_enabled(config) {
        bail!("TUN did not remain enabled; configuration was not saved");
    }
    if device_is_ready_or_unverifiable(c, config) {
        return Ok(config.clone());
    }
    let mut current = config.clone();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if !is_enabled(&current) {
            bail!("TUN did not remain enabled; configuration was not saved");
        }
        if device_is_ready_or_unverifiable(c, &current) {
            return Ok(current);
        }
        if std::time::Instant::now() >= deadline {
            bail!(
                "TUN is configured on, but no matching local TUN device is UP; check mihomo.log, /dev/net/tun, permissions, and network namespace. Configuration was not saved"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        current = c.configs()?;
    }
}

/// Configuration and device observations are reported separately: neither proves routing works.
pub(crate) fn status(c: &Client) -> Result<()> {
    let config = c.configs()?;
    print_status(c, &config);
    Ok(())
}

pub(crate) fn print_status(c: &Client, config: &serde_json::Value) {
    print!(
        "{}",
        render_status(config, c.is_local(), std::path::Path::new("/sys/class/net"))
    );
}

fn render_status(config: &serde_json::Value, local: bool, interfaces: &std::path::Path) -> String {
    let mode = config["mode"].as_str().unwrap_or("unknown");
    let enabled = match config["tun"]["enable"].as_bool() {
        Some(true) => "on",
        Some(false) => "off",
        None => "unknown",
    };
    let device = config["tun"]["device"]
        .as_str()
        .filter(|name| !name.is_empty());
    let mut text = format!("Mode: {mode}\nTUN configured: {enabled}\n");
    let Some(device) = device else {
        text.push_str("TUN device: unknown (controller did not report a device name)\n");
        return text;
    };
    if !local {
        text.push_str(&format!(
            "TUN device: {device} (remote controller; device not checked)\n"
        ));
        return text;
    }
    #[cfg(target_os = "linux")]
    {
        // Interface names from a remote API must never become arbitrary filesystem paths.
        if device == "." || device == ".." || device.contains('/') || device.contains('\\') {
            text.push_str("TUN device: unknown (invalid interface name)\n");
            return text;
        }
        let path = interfaces.join(device);
        match std::fs::read_to_string(path.join("tun_flags")) {
            Ok(flags) => match parse_flags(&flags) {
                Some(flags) if flags & 1 != 0 => {
                    let state = std::fs::read_to_string(path.join("flags"))
                        .ok()
                        .and_then(|f| parse_flags(&f))
                        .map(|f| if f & 1 != 0 { "UP" } else { "DOWN" })
                        .unwrap_or("state unknown");
                    text.push_str(&format!(
                        "TUN device: {device} (Linux TUN, {state}; ownership/routes not verified)\n"
                    ));
                }
                Some(_) => text.push_str(&format!("TUN device: {device} (not a TUN interface)\n")),
                None => text.push_str(&format!(
                    "TUN device: {device} (unrecognized device flags)\n"
                )),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if path.try_exists().ok() == Some(false) {
                    text.push_str(&format!(
                        "TUN device: {device} (not found in local network namespace)\n"
                    ));
                } else {
                    text.push_str(&format!("TUN device: {device} (TUN type not confirmed)\n"));
                }
            }
            Err(_) => text.push_str(&format!(
                "TUN device: {device} (device inspection unavailable)\n"
            )),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = interfaces;
        text.push_str(&format!(
            "TUN device: {device} (device inspection currently supports Linux only)\n"
        ));
    }
    text
}

#[cfg(target_os = "linux")]
fn parse_flags(text: &str) -> Option<u32> {
    let text = text.trim();
    u32::from_str_radix(text.strip_prefix("0x").unwrap_or(text), 16).ok()
}

#[cfg(test)]
mod status_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn verification_rejects_disabled_runtime_without_network_access() {
        let c = Client::new("http://example.invalid".into(), None);
        assert!(verify_enabled(&c, &json!({"tun":{"enable":false}})).is_err());
    }

    #[test]
    fn remote_status_never_inspects_local_device() {
        let config = json!({"mode": "rule", "tun": {"enable": true, "device": "Meta"}});
        let text = render_status(&config, false, std::path::Path::new("/missing"));
        assert!(text.contains("TUN configured: on"));
        assert!(text.contains("remote controller; device not checked"));
    }

    #[test]
    fn missing_api_fields_are_unknown_not_off() {
        let text = render_status(&json!({}), true, std::path::Path::new("/missing"));
        assert!(text.contains("TUN configured: unknown"));
        assert!(text.contains("did not report a device name"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn distinguishes_real_tun_tap_missing_and_invalid_devices() {
        let root = std::env::temp_dir().join(format!("mset-tun-status-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Meta")).unwrap();
        let mut config = json!({"mode": "rule", "tun": {"enable": true, "device": "Meta"}});
        std::fs::write(root.join("Meta/tun_flags"), "0x1001\n").unwrap();
        std::fs::write(root.join("Meta/flags"), "0x1003\n").unwrap();
        let text = render_status(&config, true, &root);
        assert!(local_device_ready(&config, &root));
        assert!(text.contains("Linux TUN, UP"));
        assert!(text.contains("ownership/routes not verified"));
        std::fs::write(root.join("Meta/flags"), "0x1002\n").unwrap();
        assert!(!local_device_ready(&config, &root));
        std::fs::write(root.join("Meta/flags"), "0x1003\n").unwrap();
        std::fs::write(root.join("Meta/tun_flags"), "0x1002\n").unwrap();
        assert!(!local_device_ready(&config, &root));
        assert!(render_status(&config, true, &root).contains("not a TUN interface"));
        config["tun"]["device"] = json!("missing");
        assert!(render_status(&config, true, &root).contains("not found"));
        config["tun"]["device"] = json!("../Meta");
        assert!(render_status(&config, true, &root).contains("invalid interface name"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
