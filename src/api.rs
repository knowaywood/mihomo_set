use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::io::Read;
use urlencoding::encode;

pub struct Client {
    base: String,
    secret: Option<String>,
    agent: ureq::Agent,
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
        }
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

    fn status_err(code: u16, resp: ureq::Response) -> anyhow::Error {
        let text = resp.into_string().unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("HTTP {}", code));
        anyhow!("{}", msg)
    }

    pub fn get_json(&self, path: &str) -> Result<Value> {
        match self.json_req("GET", path).call() {
            Ok(r) => Ok(r.into_json()?),
            Err(ureq::Error::Status(code, r)) => Err(Self::status_err(code, r)),
            Err(e) => Err(anyhow!("{}", e)),
        }
    }

    pub fn put_json(&self, path: &str, body: Value) -> Result<()> {
        match self.json_req("PUT", path).send_json(body) {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, r)) => Err(Self::status_err(code, r)),
            Err(e) => Err(anyhow!("{}", e)),
        }
    }

    pub fn patch_json(&self, path: &str, body: Value) -> Result<()> {
        match self.json_req("PATCH", path).send_json(body) {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, r)) => Err(Self::status_err(code, r)),
            Err(e) => Err(anyhow!("{}", e)),
        }
    }

    pub fn post_json(&self, path: &str) -> Result<()> {
        match self.json_req("POST", path).send_json(json!({})) {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, r)) => Err(Self::status_err(code, r)),
            Err(e) => Err(anyhow!("{}", e)),
        }
    }

    pub fn delete(&self, path: &str) -> Result<()> {
        match self.json_req("DELETE", path).call() {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, r)) => Err(Self::status_err(code, r)),
            Err(e) => Err(anyhow!("{}", e)),
        }
    }

    pub fn stream(&self, path: &str) -> Result<Box<dyn Read + Send>> {
        match self.req("GET", path).call() {
            Ok(r) => Ok(Box::new(r.into_reader())),
            Err(ureq::Error::Status(code, r)) => Err(Self::status_err(code, r)),
            Err(e) => Err(anyhow!("{}", e)),
        }
    }

    pub fn configs(&self) -> Result<Value> {
        self.get_json("/configs")
    }

    pub fn patch_configs(&self, body: Value) -> Result<()> {
        self.patch_json("/configs", body)
    }

    pub fn proxies(&self) -> Result<Value> {
        self.get_json("/proxies")
    }

    pub fn providers(&self) -> Result<Value> {
        self.get_json("/providers/proxies")
    }

    pub fn select(&self, group: &str, node: &str) -> Result<()> {
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
