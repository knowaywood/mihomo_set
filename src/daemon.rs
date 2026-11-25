use anyhow::{Context, Result, bail};
use std::path::PathBuf;

pub fn start(dir: Option<PathBuf>) -> Result<()> {
    run(dir, Action::Start)
}
pub fn stop(dir: Option<PathBuf>) -> Result<()> {
    run(dir, Action::Stop)
}
pub fn restart(dir: Option<PathBuf>) -> Result<()> {
    run(dir, Action::Restart)
}
enum Action {
    Start,
    Stop,
    Restart,
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
        Action::Start => unix::start(&dir),
        Action::Stop => unix::stop(&dir),
        Action::Restart => {
            unix::stop(&dir)?;
            unix::start(&dir)
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

    pub(super) fn start(dir: &Path) -> Result<()> {
        if let Some((pid, _)) = managed(dir)? {
            println!("Mihomo is already running (PID {pid}).");
            return Ok(());
        }
        let binary = dir.join("mihomo");
        let config = dir.join("config.yaml");
        if !binary.is_file() || !config.is_file() {
            bail!("installation incomplete; run mset setup first");
        }
        let checked = Command::new(&binary)
            .arg("-t")
            .arg("-d")
            .arg(dir)
            .arg("-f")
            .arg(&config)
            .status()
            .context("cannot run Mihomo configuration check")?;
        if !checked.success() {
            bail!("Mihomo configuration check failed ({checked})");
        }
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
        println!(
            "Mihomo started in background (PID {}). Log: {}",
            child.id(),
            log_path.display()
        );
        Ok(())
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
        let deadline = Instant::now() + Duration::from_secs(10);
        while identity(pid)?.as_deref() == Some(&birth) {
            if Instant::now() >= deadline {
                bail!(
                    "Mihomo did not stop within 10 seconds; PID record retained; restart cancelled"
                );
            }
            thread::sleep(Duration::from_millis(100));
        }
        fs::remove_file(dir.join("mihomo.pid"))?;
        println!("Mihomo stopped (PID {pid}).");
        Ok(())
    }
}
