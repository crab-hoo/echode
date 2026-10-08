//! Parent side: spawn `echode-plugin-host` and exchange blocks with it.

use crate::CMD_SHUTDOWN;
use crate::region::{MappedRegion, RegionError};
use crate::sync::WaitError;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static SHM_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct SpawnOptions<'a> {
    pub host_bin: &'a Path,
    pub plugin: Option<&'a Path>,
    /// Passed through as `ECHODE_TEST_FAULT` (`crash` or `hang`).
    pub fault: Option<&'a str>,
}

pub struct PluginHost {
    region: MappedRegion,
    child: Child,
    seq: u64,
    shm_path: PathBuf,
    stderr: Arc<Mutex<String>>,
    closed: bool,
}

#[derive(Debug)]
pub enum SessionError {
    Region(RegionError),
    Io(std::io::Error),
    Timeout,
    HostExited { status: ExitStatus, stderr: String },
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Region(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::Timeout => f.write_str("plugin process did not answer before the deadline"),
            Self::HostExited { status, stderr } => {
                write!(f, "plugin process exited ({status})")?;
                if !stderr.is_empty() {
                    write!(f, ": {stderr}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for SessionError {}

impl From<RegionError> for SessionError {
    fn from(error: RegionError) -> Self {
        match error {
            RegionError::Wait(WaitError::Timeout) => Self::Timeout,
            other => Self::Region(other),
        }
    }
}

impl From<std::io::Error> for SessionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl PluginHost {
    pub fn spawn(options: SpawnOptions<'_>) -> Result<Self, SessionError> {
        let n = SHM_SEQ.fetch_add(1, Ordering::Relaxed);
        let shm_path = std::env::temp_dir().join(format!("echode-shm-{}-{n}", std::process::id()));
        let region = MappedRegion::create(&shm_path)?;

        let mut command = Command::new(options.host_bin);
        command
            .arg("--shm")
            .arg(&shm_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(plugin) = options.plugin {
            command.arg("--plugin").arg(plugin);
        }
        if let Some(fault) = options.fault {
            command.env("ECHODE_TEST_FAULT", fault);
        }
        let mut child = command.spawn()?;
        let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
        let stderr = Arc::new(Mutex::new(String::new()));
        let stderr_thread = Arc::clone(&stderr);
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr_pipe.read_to_string(&mut text);
            if let Ok(mut slot) = stderr_thread.lock() {
                slot.push_str(&text);
            }
        });

        let mut host = Self {
            region,
            child,
            seq: 0,
            shm_path,
            stderr,
            closed: false,
        };
        match host.wait_until_ready(Duration::from_secs(5)) {
            Ok(()) => Ok(host),
            Err(error) => {
                host.force_stop();
                Err(error)
            }
        }
    }

    pub fn process(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        timeout: Duration,
    ) -> Result<(), SessionError> {
        self.exchange(crate::CMD_PROCESS, input, output, timeout)
    }

    pub fn shutdown(&mut self) -> Result<(), SessionError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        if self.child.try_wait()?.is_none() {
            let frames = [0.0f32; 1];
            let mut left = [0.0f32; 1];
            let mut right = [0.0f32; 1];
            let input: [&[f32]; 2] = [&frames, &frames];
            let mut output: [&mut [f32]; 2] = [&mut left, &mut right];
            // One sample is enough to carry the shutdown command. Ignore a hang;
            // the process is killed below.
            let _ = self.exchange(
                CMD_SHUTDOWN,
                &input,
                &mut output,
                Duration::from_millis(200),
            );
            if self.child.try_wait()?.is_none() {
                self.force_stop();
            }
        }
        let _ = std::fs::remove_file(&self.shm_path);
        Ok(())
    }

    fn exchange(
        &mut self,
        command: u32,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        timeout: Duration,
    ) -> Result<(), SessionError> {
        self.seq += 1;
        match self
            .region
            .request(self.seq, input, output, command, timeout)
        {
            Ok(()) => Ok(()),
            Err(RegionError::Wait(WaitError::Timeout)) => self.timeout_or_exit(),
            Err(error) => Err(error.into()),
        }
    }

    fn wait_until_ready(&mut self, timeout: Duration) -> Result<(), SessionError> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Err(self.exited(status));
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(SessionError::Timeout);
            }
            let slice = remaining.min(Duration::from_millis(20));
            match self.region.wait_ready(slice) {
                Ok(()) => return Ok(()),
                Err(RegionError::Wait(WaitError::Timeout)) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn timeout_or_exit(&mut self) -> Result<(), SessionError> {
        if let Some(status) = self.child.try_wait()? {
            Err(self.exited(status))
        } else {
            Err(SessionError::Timeout)
        }
    }

    fn exited(&self, status: ExitStatus) -> SessionError {
        let stderr = self
            .stderr
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default();
        SessionError::HostExited { status, stderr }
    }

    fn force_stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
