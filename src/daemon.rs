use anyhow::{Context, Result, bail};
use std::{path::PathBuf, time::Duration};

pub fn start(dir: Option<PathBuf>, timeout: u64) -> Result<()> {
    run(dir, Action::Start(timeout))
}
pub fn stop(dir: Option<PathBuf>) -> Result<()> {
    run(dir, Action::Stop)
}
pub fn restart(dir: Option<PathBuf>, timeout: u64) -> Result<()> {
    run(dir, Action::Restart(timeout))
}
/// Prepare only a local, mset-managed instance; remote controllers never call this.
pub(crate) fn prepare_tun(dir: Option<PathBuf>) -> Result<()> {
    run(dir, Action::PrepareTun)
}

enum Action {
    Start(u64),
    Stop,
    Restart(u64),
    PrepareTun,
}

#[cfg(not(unix))]
fn run(_: Option<PathBuf>, _: Action) -> Result<()> {
    bail!("background process management currently supports Linux and macOS only")
}

#[cfg(unix)]
fn run(dir: Option<PathBuf>, action: Action) -> Result<()> {
    use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};
    let dir = crate::install::directory(dir)?;
    if !dir.is_dir() {
        if matches!(action, Action::Stop) {
            println!("Mihomo is not running.");
            return Ok(());
        }
        bail!("installation missing; run mset setup first");
    }
    let dir = dir.canonicalize()?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join("mset.lock"))?;
    lock.try_lock()
        .context("another mset lifecycle command is running; retry after it finishes")?;
    match action {
        Action::PrepareTun => unix::prepare_tun(&dir),
        Action::Start(timeout) => unix::start(&dir, Duration::from_secs(timeout)),
        Action::Stop => unix::stop(&dir),
        Action::Restart(timeout) => {
            unix::validate(&dir, Duration::from_secs(timeout))?;
            unix::stop(&dir)?;
            unix::start(&dir, Duration::from_secs(timeout))
        }
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use serde_json::{Value, json};
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        os::unix::{fs::OpenOptionsExt, process::CommandExt},
        path::Path,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    // Record a process birth identity as well as PID, so stale records do not target reused PIDs.
    fn identity(pid: u32) -> Result<Option<String>> {
        if pid <= 1 || pid > i32::MAX as u32 {
            bail!("invalid managed PID");
        }
        #[cfg(target_os = "linux")]
        {
            let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
                Ok(s) => s,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            let (_, fields) = stat.rsplit_once(") ").context("invalid process stat")?;
            let fields: Vec<_> = fields.split_whitespace().collect();
            if matches!(fields.first(), Some(&"Z") | Some(&"X")) {
                return Ok(None);
            }
            let birth = fields.get(19).context("missing process birth time")?;
            let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
            Ok(Some(format!("{}:{birth}", boot.trim())))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let output = Command::new("ps")
                .args([
                    "-ww",
                    "-p",
                    &pid.to_string(),
                    "-o",
                    "lstart=",
                    "-o",
                    "args=",
                ])
                .output()?;
            if !output.status.success() {
                // Distinguish missing PID from permission errors before treating it as stale.
                if unsafe { libc::kill(pid as i32, 0) } == -1
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    return Ok(None);
                }
                bail!("cannot inspect managed process {pid}");
            }
            let text = String::from_utf8(output.stdout)?;
            if text.trim().is_empty() {
                return Ok(None);
            }
            Ok(Some(text.trim().to_owned()))
        }
    }

    fn managed(dir: &Path) -> Result<Option<(u32, String)>> {
        let text = match fs::read_to_string(dir.join("mihomo.pid")) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let record: Value = serde_json::from_str(&text)
            .context("invalid mihomo.pid; refusing to signal an unknown process")?;
        let pid = record["pid"]
            .as_u64()
            .and_then(|p| u32::try_from(p).ok())
            .context("invalid managed PID")?;
        let saved = record["identity"]
            .as_str()
            .context("missing process identity")?;
        if identity(pid)?.as_deref() == Some(saved) {
            Ok(Some((pid, saved.to_owned())))
        } else {
            fs::remove_file(dir.join("mihomo.pid"))?;
            Ok(None)
        }
    }

    pub(super) fn prepare_tun(dir: &Path) -> Result<()> {
        let process = managed(dir)?.context("no local mset-managed Mihomo is running; run mset start first (external processes are not restarted automatically)")?;
        #[cfg(target_os = "linux")]
        {
            {
                let (pid, _) = &process;
                // The PID/birth record already identifies the child launched by mset.
                // /proc/PID/exe may be unreadable after file capabilities are granted.
                let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
                let caps = status
                    .lines()
                    .find_map(|l| l.strip_prefix("CapEff:"))
                    .context("cannot determine Mihomo network permissions")?;
                let caps = u64::from_str_radix(caps.trim(), 16)?;
                if network_capabilities(caps) {
                    return Ok(());
                }
            }
            // Authorize before stopping: cancellation or sudo failure leaves the old process running.
            crate::install::grant_tun(Some(dir.to_path_buf()))?;
            stop(dir)?;
            start(dir, Duration::from_secs(30))?;
            let (pid, _) = managed(dir)?.context("Mihomo did not start")?;
            let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
            let caps = status
                .lines()
                .find_map(|l| l.strip_prefix("CapEff:"))
                .context("cannot determine Mihomo network permissions")?;
            if !network_capabilities(u64::from_str_radix(caps.trim(), 16)?) {
                bail!(
                    "Mihomo still lacks TUN permissions after restart; check filesystem mount flags and container capability limits"
                );
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = process;
            // Do not elevate an entire core silently on platforms without Linux file capabilities.
            bail!(
                "automatic TUN authorization currently supports Linux only; start Mihomo with administrator privileges on this platform"
            )
        }
    }

    #[cfg(target_os = "linux")]
    fn network_capabilities(caps: u64) -> bool {
        const REQUIRED: u64 = (1 << 12) | (1 << 13); // CAP_NET_ADMIN | CAP_NET_RAW
        caps & REQUIRED == REQUIRED
    }

    #[cfg(all(test, target_os = "linux"))]
    mod capability_tests {
        use super::*;
        #[test]
        fn requires_both_network_capabilities() {
            assert!(!network_capabilities(0));
            assert!(!network_capabilities(1 << 12));
            assert!(!network_capabilities(1 << 13));
            assert!(network_capabilities((1 << 12) | (1 << 13)));
        }
    }

    pub(super) fn validate(dir: &Path, timeout: Duration) -> Result<()> {
        let binary = dir.join("mihomo");
        let config = dir.join("config.yaml");
        if !binary.is_file() || !config.is_file() {
            bail!(
                "installation incomplete in {}; run mset setup first",
                dir.display()
            );
        }
        crate::cfg::read_controller(&config)?;
        let mut check = Command::new(&binary)
            .arg("-t")
            .arg("-d")
            .arg(dir)
            .arg("-f")
            .arg(&config)
            .stdin(Stdio::null())
            .spawn()
            .context("cannot run Mihomo configuration check")?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = check.try_wait()? {
                if status.success() {
                    return Ok(());
                }
                bail!(
                    "Mihomo configuration check failed ({status}); fix {} before starting/restarting",
                    config.display()
                );
            }
            if Instant::now() >= deadline {
                let _ = check.kill();
                let _ = check.wait();
                bail!(
                    "configuration check timed out after {} seconds for {}; existing running core was not stopped",
                    timeout.as_secs(),
                    config.display()
                );
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    pub(super) fn start(dir: &Path, timeout: Duration) -> Result<()> {
        if let Some((pid, _)) = managed(dir)? {
            wait_ready(dir, pid, timeout)?;
            println!("Mihomo is already running and API ready (PID {pid}).");
            return Ok(());
        }
        validate(dir, timeout)?;
        let binary = dir.join("mihomo");
        let config = dir.join("config.yaml");
        let log_path = dir.join("mihomo.log");
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&log_path)?;
        let mut command = Command::new(&binary);
        command
            .arg("-d")
            .arg(dir)
            .arg("-f")
            .arg(config)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        // A new session disconnects the child from the calling terminal.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().context("cannot launch Mihomo")?;
        let result = (|| -> Result<()> {
            thread::sleep(Duration::from_millis(500));
            if let Some(status) = child.try_wait()? {
                bail!(
                    "Mihomo exited during startup ({status}); see {}",
                    log_path.display()
                );
            }
            let birth = identity(child.id())?.context("Mihomo disappeared during startup")?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(dir.join("mihomo.pid"))?;
            let record = json!({"pid": child.id(), "identity": birth}).to_string();
            if let Err(e) = file
                .write_all(record.as_bytes())
                .and_then(|_| file.sync_all())
            {
                drop(file);
                let _ = fs::remove_file(dir.join("mihomo.pid"));
                return Err(e.into());
            }
            Ok(())
        })();
        if let Err(e) = result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        if let Err(error) = wait_ready(dir, child.id(), timeout) {
            if child.try_wait()?.is_some() {
                let _ = managed(dir)?; // Clear a dead process record.
                return Err(error.context(format!(
                    "Mihomo exited during startup; inspect {}",
                    log_path.display()
                )));
            }
            return Err(error.context(format!("Mihomo process {} remains running but is not ready; inspect {} or run mset stop --dir {:?}", child.id(), log_path.display(), dir)));
        }
        println!(
            "Mihomo started in background and API ready (PID {}). Log: {}",
            child.id(),
            log_path.display()
        );
        Ok(())
    }

    fn wait_ready(dir: &Path, pid: u32, timeout: Duration) -> Result<()> {
        let c = crate::cfg::client_for_file(&dir.join("config.yaml"), None)?;
        let deadline = Instant::now() + timeout;
        let mut last_error: Option<anyhow::Error> = None;
        loop {
            if identity(pid)?.is_none() {
                bail!("Mihomo process {pid} exited before API readiness");
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let message = format!(
                    "controller API startup timed out after {} seconds",
                    timeout.as_secs()
                );
                if let Some(error) = last_error {
                    return Err(error).context(message);
                }
                bail!("{message}");
            }
            match c.configs_timeout(remaining.min(Duration::from_secs(2))) {
                Ok(_) => return Ok(()),
                Err(error) if !crate::errors::retryable(&error) => {
                    return Err(error).context("controller API readiness check failed");
                }
                Err(error) if Instant::now() >= deadline => {
                    return Err(error).context(format!(
                        "controller API startup timed out after {} seconds",
                        timeout.as_secs()
                    ));
                }
                Err(error) => {
                    last_error = Some(error);
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    pub(super) fn stop(dir: &Path) -> Result<()> {
        let Some((pid, birth)) = managed(dir)? else {
            println!("No mset-managed Mihomo is running.");
            return Ok(());
        };
        if unsafe { libc::kill(pid as i32, libc::SIGTERM) } == -1 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ESRCH) {
                return Err(e).context("cannot stop Mihomo");
            }
        }
        // TUN teardown can take tens of seconds; wait for actual exit rather than
        // forcing callers to insert a sleep between stop and start.
        let deadline = Instant::now() + Duration::from_secs(60);
        while identity(pid)?.as_deref() == Some(&birth) {
            if Instant::now() >= deadline {
                bail!(
                    "Mihomo did not stop within 60 seconds; PID record retained; restart cancelled"
                );
            }
            thread::sleep(Duration::from_millis(100));
        }
        fs::remove_file(dir.join("mihomo.pid"))?;
        println!("Mihomo stopped (PID {pid}).");
        Ok(())
    }
}
