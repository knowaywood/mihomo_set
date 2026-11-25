use anyhow::{Context, Result, bail};
use serde_yaml_ng::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub fn resolve(api: Option<String>, secret: Option<String>) -> Result<crate::api::Client> {
    let explicit_api = api.or_else(|| std::env::var("MIHOMO_API").ok());
    let secret = secret.or_else(|| std::env::var("MIHOMO_SECRET").ok());
    for path in candidates() {
        if !path.try_exists()? {
            if std::env::var_os("MIHOMO_CONFIG").is_some() {
                bail!("selected configuration does not exist: {}", path.display());
            }
            continue;
        }
        let discovered = discover_controller(&path)?;
        let (base, saved_secret) = match discovered {
            Some(controller) => controller,
            None if std::env::var_os("MIHOMO_CONFIG").is_some() => {
                bail!(
                    "{} needs a string external-controller address",
                    path.display()
                );
            }
            None => continue,
        };
        let selected = explicit_api
            .as_deref()
            .map(normalize)
            .unwrap_or_else(|| base.clone());
        let inherited_secret = if selected == base { saved_secret } else { None };
        let mut c = crate::api::Client::new(selected.clone(), secret.clone().or(inherited_secret));
        if c.is_local() && selected == base {
            c.set_config_path(path.canonicalize()?);
        }
        return Ok(c);
    }
    Ok(crate::api::Client::new(
        normalize(explicit_api.as_deref().unwrap_or("127.0.0.1:9090")),
        secret,
    ))
}

pub(crate) fn client_for_file(path: &Path, secret: Option<String>) -> Result<crate::api::Client> {
    let (base, saved_secret) = read_controller(path)?;
    let mut c = crate::api::Client::new(
        base,
        secret
            .or_else(|| std::env::var("MIHOMO_SECRET").ok())
            .or(saved_secret),
    );
    if c.is_local() {
        c.set_config_path(path.canonicalize()?);
    }
    Ok(c)
}

pub(crate) fn read_controller(path: &Path) -> Result<(String, Option<String>)> {
    discover_controller(path)?.with_context(|| {
        format!(
            "{} needs a string external-controller address",
            path.display()
        )
    })
}

fn discover_controller(path: &Path) -> Result<Option<(String, Option<String>)>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let value = parse(&text).with_context(|| format!("invalid YAML in {}", path.display()))?;
    let base = match value.get("external-controller") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(base)) if base.trim().is_empty() => return Ok(None),
        Some(Value::String(base)) => base,
        Some(_) => bail!(
            "{}: external-controller must be a YAML string address",
            path.display()
        ),
    };
    let secret = match value.get("secret") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            if s.is_empty() {
                None
            } else {
                Some(s.clone())
            }
        }
        Some(_) => bail!(
            "{}: secret must be a YAML string (quote numeric secrets)",
            path.display()
        ),
    };
    Ok(Some((normalize(base), secret)))
}

pub(crate) fn selection_cache_enabled(path: &Path) -> Result<bool> {
    let config = parse(&fs::read_to_string(path)?)?;
    Ok(config["profile"]["store-selected"].as_bool() == Some(true))
}

fn parse(text: &str) -> Result<Value> {
    let mut value: Value = serde_yaml_ng::from_str(text)?;
    value.apply_merge()?;
    if !value.is_mapping() {
        bail!("configuration root must be a YAML mapping");
    }
    Ok(value)
}

fn candidates() -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os("MIHOMO_CONFIG") {
        // An explicitly selected missing or invalid file must not silently fall back.
        return vec![PathBuf::from(path)];
    }
    let mut paths = vec![PathBuf::from("config.yaml")];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for name in [
            ".config/mihomo/config.yaml",
            ".config/mihomo/config.yml",
            ".config/clash/config.yaml",
            ".config/clash/config.yml",
        ] {
            paths.push(home.join(name));
        }
    }
    for name in [
        "/etc/mihomo/config.yaml",
        "/etc/mihomo/config.yml",
        "/etc/clash/config.yaml",
    ] {
        paths.push(name.into());
    }
    paths
}

fn normalize(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    let full = if base.starts_with("http://") || base.starts_with("https://") {
        base.to_owned()
    } else {
        format!("http://{base}")
    };
    let (scheme, rest) = full.split_once("://").unwrap();
    let authority = rest.split('/').next().unwrap_or("");
    let authority = if authority.is_empty() {
        "127.0.0.1:9090".to_owned()
    } else if authority.starts_with(':') {
        format!("127.0.0.1{authority}")
    } else if authority == "0.0.0.0" {
        "127.0.0.1".into()
    } else if let Some(port) = authority.strip_prefix("0.0.0.0:") {
        format!("127.0.0.1:{port}")
    } else {
        authority.to_owned()
    };
    format!("{scheme}://{authority}")
}

fn merge(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Mapping(target), Value::Mapping(patch)) => {
            for (key, value) in patch {
                if let Some(old) = target.get_mut(&key) {
                    merge(old, value);
                } else {
                    target.insert(key, value);
                }
            }
        }
        (target, value) => *target = value,
    }
}

/// Prepare a complete replacement before touching the live API. Commit only after API success.
pub(crate) struct Update {
    path: PathBuf,
    temp: PathBuf,
    _lock: File,
}
impl Update {
    pub(crate) fn prepare(path: &Path, patch: &serde_json::Value) -> Result<Self> {
        let path = path.canonicalize()?;
        let lock = private_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(append_suffix(&path, ".mset.lock"))?;
        lock.try_lock()
            .context("another command is saving this configuration")?;
        let original = fs::read_to_string(&path)?;
        let mut value = parse(&original)?;
        merge(&mut value, serde_yaml_ng::to_value(patch)?);
        let text = serde_yaml_ng::to_string(&value)?;
        // Keep the exact previous file, including comments, before rewriting YAML.
        atomic_write(&append_suffix(&path, ".bak"), original.as_bytes())?;
        let temp = append_suffix(&path, &format!(".{}.tmp", std::process::id()));
        write_temp(&temp, text.as_bytes())?;
        Ok(Self {
            path,
            temp,
            _lock: lock,
        })
    }
    pub(crate) fn commit(self) -> Result<()> {
        fs::rename(&self.temp, &self.path)
            .context("runtime changed, but saving config.yaml failed")?;
        Ok(())
    }
}
impl Drop for Update {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.temp);
    }
}
fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
fn write_temp(path: &Path, data: &[u8]) -> Result<()> {
    let mut file = private_options().write(true).create_new(true).open(path)?;
    if let Err(e) = file.write_all(data).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(e.into());
    }
    Ok(())
}
fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let temp = append_suffix(path, &format!(".{}.tmp", std::process::id()));
    write_temp(&temp, data)?;
    if let Err(e) = fs::rename(&temp, path) {
        let _ = fs::remove_file(temp);
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn yaml_handles_comments_escaped_secrets_and_merge_keys() {
        let text = "defaults: &defaults\n  external-controller: '0.0.0.0:9090' # comment\n<<: *defaults\nsecret: \"a\\\"b\\\\c#d\" # comment\ntun: {enable: true}\n";
        let value = parse(text).unwrap();
        assert_eq!(value["external-controller"].as_str(), Some("0.0.0.0:9090"));
        assert_eq!(value["secret"].as_str(), Some("a\"b\\c#d"));
        assert_eq!(
            normalize(value["external-controller"].as_str().unwrap()),
            "http://127.0.0.1:9090"
        );
        assert!(parse("tun: [broken").is_err());
    }
    #[test]
    fn automatic_discovery_skips_controllerless_files_but_explicit_reads_are_strict() {
        let dir = std::env::temp_dir().join(format!("mset-discovery-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        for text in [
            "mixed-port: 7890\n",
            "external-controller: null\n",
            "external-controller: ''\n",
        ] {
            fs::write(&path, text).unwrap();
            assert!(discover_controller(&path).unwrap().is_none());
            assert!(read_controller(&path).is_err());
        }
        fs::write(
            &path,
            "external-controller: 127.0.0.1:9090 # comment\nsecret: 'abc'\n",
        )
        .unwrap();
        assert_eq!(
            discover_controller(&path).unwrap().unwrap().0,
            "http://127.0.0.1:9090"
        );
        fs::write(&path, "external-controller: 9090\n").unwrap();
        assert!(discover_controller(&path).is_err());
        fs::write(&path, "external-controller: [broken\n").unwrap();
        assert!(discover_controller(&path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn persistent_patch_preserves_dns_nodes_and_other_tun_settings() {
        let dir = std::env::temp_dir().join(format!("mset-config-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        let original = "# keep in backup\nexternal-controller: 127.0.0.1:9090\nsecret: 'a#b'\nmode: rule\ntun: {enable: false, device: Meta, mtu: 1400}\ndns: {enable: true}\nproxies: [{name: HK, type: direct}]\n";
        fs::write(&path, original).unwrap();
        let patch = serde_json::json!({"mode":"global", "tun":{"enable":true,"auto-route":true}});
        let pending = Update::prepare(&path, &patch).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        drop(pending); // failed API: no file change
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        Update::prepare(&path, &patch).unwrap().commit().unwrap();
        let saved = parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["mode"].as_str(), Some("global"));
        assert_eq!(saved["tun"]["device"].as_str(), Some("Meta"));
        assert_eq!(saved["tun"]["mtu"].as_u64(), Some(1400));
        assert_eq!(saved["tun"]["enable"].as_bool(), Some(true));
        assert_eq!(saved["dns"]["enable"].as_bool(), Some(true));
        assert_eq!(saved["proxies"][0]["name"].as_str(), Some("HK"));
        assert_eq!(
            fs::read_to_string(append_suffix(&path, ".bak")).unwrap(),
            original
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
