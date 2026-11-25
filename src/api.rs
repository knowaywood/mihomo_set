use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::io::Read;
use urlencoding::encode;

pub struct Client {
    base: String,
    secret: Option<String>,
    agent: ureq::Agent,
    config_path: Option<std::path::PathBuf>,
}

impl Client {
    pub fn new(base: String, secret: Option<String>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(5))
            .build();
        Self {
            base: base.trim_end_matches('/').to_string(),
            secret,
            agent,
            config_path: None,
        }
    }

    pub(crate) fn config_path(&self) -> Option<&std::path::Path> {
        self.config_path.as_deref()
    }

    pub(crate) fn set_config_path(&mut self, path: std::path::PathBuf) {
        self.config_path = Some(path);
    }

    pub(crate) fn persist_patch(&self, body: &Value) -> Result<()> {
        if let Some(path) = &self.config_path {
            crate::cfg::Update::prepare(path, body)
                .and_then(|update| update.commit())
                .map_err(|error| {
                    anyhow!(
                        "runtime changed, but could not save {}: {error:#}",
                        path.display()
                    )
                })?;
            println!("Saved to {}", path.display());
        } else {
            println!(
                "Runtime only: no matching local configuration file (remote files are not modified)."
            );
        }
        Ok(())
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base
    }

    pub(crate) fn is_local(&self) -> bool {
        let host = self
            .base
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(&self.base);
        let authority = host.split('/').next().unwrap_or("");
        if authority.contains('@') {
            return false;
        }
        authority == "localhost"
            || authority.starts_with("localhost:")
            || authority == "127.0.0.1"
            || authority.starts_with("127.0.0.1:")
            || authority == "[::1]"
            || authority.starts_with("[::1]:")
    }

    fn req(&self, method: &str, path: &str) -> ureq::Request {
        let mut r = self
            .agent
            .request(method, &format!("{}{}", self.base, path));
        if let Some(s) = &self.secret {
            r = r.set("Authorization", &format!("Bearer {}", s));
        }
        r
    }

    fn json_req(&self, method: &str, path: &str) -> ureq::Request {
        self.req(method, path)
            .timeout(std::time::Duration::from_secs(60))
    }

    pub fn get_json(&self, path: &str) -> Result<Value> {
        self.get_json_timeout(path, std::time::Duration::from_secs(60))
    }

    fn get_json_timeout(&self, path: &str, timeout: std::time::Duration) -> Result<Value> {
        let response = self
            .req("GET", path)
            .timeout(timeout)
            .call()
            .map_err(|e| crate::errors::request_error("GET", path, e))?;
        response.into_json().map_err(|error| {
            let timeout = matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            );
            crate::errors::ApiError {
                kind: if timeout {
                    crate::errors::ApiKind::Timeout
                } else {
                    crate::errors::ApiKind::Protocol
                },
                status: None,
                operation: crate::errors::operation("GET", path),
                detail: if timeout {
                    "while reading response body"
                } else {
                    "expected valid JSON"
                }
                .into(),
            }
            .into()
        })
    }

    pub fn put_json(&self, path: &str, body: Value) -> Result<()> {
        self.json_req("PUT", path)
            .send_json(body)
            .map(|_| ())
            .map_err(|e| crate::errors::request_error("PUT", path, e))
    }
    pub fn patch_json(&self, path: &str, body: Value) -> Result<()> {
        self.json_req("PATCH", path)
            .send_json(body)
            .map(|_| ())
            .map_err(|e| crate::errors::request_error("PATCH", path, e))
    }
    pub fn post_json(&self, path: &str) -> Result<()> {
        self.json_req("POST", path)
            .send_json(json!({}))
            .map(|_| ())
            .map_err(|e| crate::errors::request_error("POST", path, e))
    }
    pub fn delete(&self, path: &str) -> Result<()> {
        self.json_req("DELETE", path)
            .call()
            .map(|_| ())
            .map_err(|e| crate::errors::request_error("DELETE", path, e))
    }
    pub fn stream(&self, path: &str) -> Result<Box<dyn Read + Send>> {
        let response = self
            .req("GET", path)
            .call()
            .map_err(|e| crate::errors::request_error("GET", path, e))?;
        Ok(Box::new(response.into_reader()))
    }

    pub fn configs(&self) -> Result<Value> {
        self.configs_timeout(std::time::Duration::from_secs(5))
    }

    pub(crate) fn configs_timeout(&self, timeout: std::time::Duration) -> Result<Value> {
        let config = self.get_json_timeout("/configs", timeout)?;
        if config.get("mode").and_then(Value::as_str).is_none() {
            return Err(crate::errors::ApiError {
                kind: crate::errors::ApiKind::Protocol,
                status: None,
                operation: "GET /configs".into(),
                detail: "response is not a Mihomo configuration".into(),
            }
            .into());
        }
        Ok(config)
    }

    pub fn patch_configs(&self, body: Value) -> Result<()> {
        let pending = self
            .config_path
            .as_ref()
            .map(|path| crate::cfg::Update::prepare(path, &body))
            .transpose()?;
        self.patch_json("/configs", body)?;
        if let Some(update) = pending {
            update.commit()?;
            if let Some(path) = &self.config_path {
                println!("Saved to {}", path.display());
            }
        } else {
            println!(
                "Runtime only: no matching local configuration file (remote files are not modified)."
            );
        }
        Ok(())
    }

    pub(crate) fn patch_configs_runtime(&self, body: Value) -> Result<()> {
        self.patch_json("/configs", body)
    }

    pub fn proxies(&self) -> Result<Value> {
        self.get_json("/proxies")
    }

    pub fn providers(&self) -> Result<Value> {
        self.get_json("/providers/proxies")
    }

    pub fn select(&self, group: &str, node: &str) -> Result<()> {
        if let Some(path) = &self.config_path {
            if !crate::cfg::selection_cache_enabled(path)? {
                crate::cfg::Update::prepare(path, &json!({"profile":{"store-selected":true}}))?
                    .commit()?;
                if let Err(error) = self.reload("") {
                    if let Err(rollback) = crate::cfg::Update::prepare(
                        path,
                        &json!({"profile":{"store-selected":false}}),
                    )
                    .and_then(|update| update.commit())
                    {
                        return Err(anyhow!(
                            "could not activate node caching: {error:#}; also could not restore configuration: {rollback:#}"
                        ));
                    }
                    return Err(anyhow!(
                        "could not activate node caching; configuration restored, node was not changed: {error:#}"
                    ));
                }
            }
        }
        self.put_json(
            &format!("/proxies/{}", encode(group)),
            json!({ "name": node }),
        )
    }

    pub fn rules(&self) -> Result<Value> {
        self.get_json("/rules")
    }

    pub fn conns(&self) -> Result<Value> {
        self.get_json("/connections")
    }

    pub fn close_all_conns(&self) -> Result<()> {
        self.delete("/connections")
    }

    pub fn close_conn(&self, id: &str) -> Result<()> {
        self.delete(&format!("/connections/{}", encode(id)))
    }

    pub fn reload(&self, path: &str) -> Result<()> {
        self.put_json(
            "/configs?force=true",
            json!({ "path": path, "payload": "" }),
        )
    }

    pub fn update_geo(&self) -> Result<()> {
        self.post_json("/configs/geo")
    }

    pub fn version(&self) -> Result<Value> {
        self.get_json("/version")
    }

    pub fn delay(
        &self,
        node: &str,
        provider: Option<&str>,
        url: &str,
        timeout: u64,
    ) -> Result<u64> {
        let path = match provider {
            Some(p) => format!(
                "/providers/proxies/{}/{}/healthcheck?timeout={}&url={}",
                encode(p),
                encode(node),
                timeout,
                encode(url)
            ),
            None => format!(
                "/proxies/{}/delay?timeout={}&url={}",
                encode(node),
                timeout,
                encode(url)
            ),
        };
        let v = self.get_json(&path)?;
        v.get("delay")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("no delay data"))
    }

    pub fn group_delay(&self, group: &str, url: &str, timeout: u64) -> Result<Vec<(String, u64)>> {
        let path = format!(
            "/group/{}/delay?timeout={}&url={}",
            encode(group),
            timeout,
            encode(url)
        );
        let v = self.get_json(&path)?;
        let mut list: Vec<(String, u64)> = v
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, d)| d.as_u64().map(|n| (k.clone(), n)))
                    .collect()
            })
            .unwrap_or_default();
        list.sort_by_key(|(_, d)| *d);
        Ok(list)
    }

    pub fn all_delays(
        &self,
        nodes: &[(String, Option<String>)],
        url: &str,
        timeout: u64,
    ) -> Vec<(String, Option<u64>)> {
        use std::sync::Mutex;
        let results: Mutex<Vec<(String, Option<u64>)>> = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for (node, provider) in nodes {
                let c = &self;
                let results = &results;
                s.spawn(move || {
                    let d = c.delay(node, provider.as_deref(), url, timeout).ok();
                    results.lock().unwrap().push((node.clone(), d));
                });
            }
        });
        let mut v = results.into_inner().unwrap_or_default();
        v.sort_by(|a, b| match (a.1, b.1) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.0.cmp(&b.0),
        });
        v
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;
    #[test]
    fn local_inspection_is_only_for_loopback_controllers() {
        for url in [
            "http://localhost:9090",
            "http://127.0.0.1:9090",
            "http://[::1]:9090",
        ] {
            assert!(Client::new(url.into(), None).is_local());
        }
        for url in [
            "http://192.168.1.2:9090",
            "http://localhost.example:9090",
            "http://localhost:9090@example.org",
        ] {
            assert!(!Client::new(url.into(), None).is_local());
        }
    }
}
