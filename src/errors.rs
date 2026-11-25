use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ApiKind {
    Authentication,
    Forbidden,
    NotFound,
    Server,
    Network,
    Dns,
    Timeout,
    InvalidAddress,
    Protocol,
    Http,
}

#[derive(Debug)]
pub struct ApiError {
    pub kind: ApiKind,
    pub status: Option<u16>,
    pub operation: String,
    pub detail: String,
}
impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.operation)?;
        if let Some(status) = self.status {
            write!(f, "HTTP {status}; ")?;
        }
        let hint = match self.kind {
            ApiKind::Authentication => "authentication failed; check --secret / MIHOMO_SECRET",
            ApiKind::Forbidden => "access denied by controller; check its permissions",
            ApiKind::NotFound => {
                "API endpoint or resource not found; check the controller/version and group/node name"
            }
            ApiKind::Server => "Mihomo controller failed; inspect mihomo.log",
            ApiKind::Network => {
                "cannot connect to controller; run mset start and check --api / MIHOMO_API"
            }
            ApiKind::Dns => "controller hostname lookup failed; check its address and DNS",
            ApiKind::Timeout => "controller request timed out; check reachability and mihomo.log",
            ApiKind::InvalidAddress => {
                "invalid controller URL; use http://host:port or https://host:port"
            }
            ApiKind::Protocol => {
                "invalid controller response; check that the address points to the Mihomo API (and TLS/proxy settings)"
            }
            ApiKind::Http => "controller rejected the request; check its arguments",
        };
        write!(f, "{hint}")?;
        if !self.detail.is_empty() {
            write!(f, " ({})", self.detail)?;
        }
        Ok(())
    }
}
impl Error for ApiError {}

pub fn operation(method: &str, path: &str) -> String {
    // Never print query strings, credentials, or full URLs from transport errors.
    format!("{method} {}", path.split('?').next().unwrap_or(path))
}

pub fn request_error(method: &str, path: &str, error: ureq::Error) -> anyhow::Error {
    let operation = operation(method, path);
    let (kind, status, detail) = match error {
        ureq::Error::Status(code, response) => {
            use std::io::Read;
            let mut body = String::new();
            let _ = response.into_reader().take(8192).read_to_string(&mut body);
            let message = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["message"].as_str().map(str::to_owned))
                .unwrap_or_default();
            let detail = message
                .chars()
                .take(300)
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            let kind = match code {
                401 => ApiKind::Authentication,
                403 => ApiKind::Forbidden,
                404 => ApiKind::NotFound,
                500..=599 => ApiKind::Server,
                _ => ApiKind::Http,
            };
            (kind, Some(code), detail)
        }
        ureq::Error::Transport(error) => {
            let mut source = error.source();
            let mut timeout = false;
            while let Some(cause) = source {
                if let Some(io) = cause.downcast_ref::<std::io::Error>() {
                    timeout |= matches!(
                        io.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    );
                }
                source = cause.source();
            }
            let kind = if timeout {
                ApiKind::Timeout
            } else {
                match error.kind() {
                    ureq::ErrorKind::Dns => ApiKind::Dns,
                    ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::ProxyConnect => {
                        ApiKind::Network
                    }
                    ureq::ErrorKind::InvalidUrl
                    | ureq::ErrorKind::UnknownScheme
                    | ureq::ErrorKind::InvalidProxyUrl => ApiKind::InvalidAddress,
                    _ => ApiKind::Protocol,
                }
            };
            (kind, None, String::new())
        }
    };
    ApiError {
        kind,
        status,
        operation,
        detail,
    }
    .into()
}

pub fn retryable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ApiError>().is_some_and(|error| {
        matches!(
            error.kind,
            ApiKind::Network | ApiKind::Timeout | ApiKind::Dns | ApiKind::Server
        )
    })
}

pub fn print(error: &anyhow::Error) {
    eprintln!("Error: {error:#}");
    if let Some(io) = error.downcast_ref::<std::io::Error>() {
        match io.kind() {
            std::io::ErrorKind::PermissionDenied => eprintln!(
                "Hint: check ownership and permissions of the configuration, core binary, and state directory."
            ),
            std::io::ErrorKind::NotFound => eprintln!(
                "Hint: check the referenced path/program; use mset setup if the core has not been installed."
            ),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auth_is_never_retried_and_queries_are_hidden() {
        let error: anyhow::Error = ApiError {
            kind: ApiKind::Authentication,
            status: Some(401),
            operation: operation("GET", "/configs?secret=hidden"),
            detail: String::new(),
        }
        .into();
        assert!(!retryable(&error));
        assert!(error.to_string().contains("MIHOMO_SECRET"));
        assert!(!error.to_string().contains("hidden"));
    }
}
