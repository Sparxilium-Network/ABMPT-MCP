//! Supervision of one Java server child process.

use std::{
    collections::VecDeque,
    path::Path,
    process::Stdio,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use regex::Regex;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
    sync::{mpsc, watch},
};

use crate::logs::{self, Level};

static DONE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"Done \([0-9.,]+s\)! For help, type "help""#).unwrap());

const RECENT_LINES: usize = 300;
const BOOT_LINES: usize = 20_000;

pub struct ProcHandle {
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    kill_tx: mpsc::Sender<()>,
    exited: watch::Receiver<Option<String>>,
    ready: watch::Receiver<bool>,
    /// Every output line until the server reported it finished booting.
    pub boot_lines: Arc<Mutex<Vec<String>>>,
    /// Ring buffer of the latest output lines.
    pub recent: Arc<Mutex<VecDeque<String>>>,
    pub started: Instant,
}

pub struct SpawnSpec<'a> {
    pub java: &'a Path,
    pub dir: &'a Path,
    pub jar: &'a Path,
    pub memory: &'a str,
}

fn pump<R: AsyncRead + Unpin + Send + 'static>(
    r: R,
    ready_tx: Arc<watch::Sender<bool>>,
    boot: Arc<Mutex<Vec<String>>>,
    recent: Arc<Mutex<VecDeque<String>>>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(raw)) = lines.next_line().await {
            let line = crate::util::strip_colors(&raw);
            {
                let mut rc = recent.lock().unwrap();
                if rc.len() >= RECENT_LINES {
                    rc.pop_front();
                }
                rc.push_back(line.clone());
            }
            if !*ready_tx.borrow() {
                {
                    let mut b = boot.lock().unwrap();
                    if b.len() < BOOT_LINES {
                        b.push(line.clone());
                    }
                }
                if DONE_RE.is_match(&line) {
                    let _ = ready_tx.send(true);
                }
            }
        }
    });
}

impl ProcHandle {
    /// Start the server. `on_exit` runs once the process has ended (before waiters wake).
    pub fn spawn(
        spec: SpawnSpec<'_>,
        on_exit: impl FnOnce(String) + Send + 'static,
    ) -> Result<Arc<Self>> {
        let mut cmd = Command::new(spec.java);
        cmd.current_dir(spec.dir)
            .arg(format!("-Xms{}", spec.memory))
            .arg(format!("-Xmx{}", spec.memory))
            .arg("-Dterminal.jline=false")
            .arg("-Dterminal.ansi=false")
            .arg("-jar")
            .arg(spec.jar)
            .arg("--nogui")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("cannot start java ({})", spec.java.display()))?;

        let (ready_tx, ready_rx) = watch::channel(false);
        let ready_tx = Arc::new(ready_tx);
        let (exit_tx, exit_rx) = watch::channel(None);
        let (kill_tx, mut kill_rx) = mpsc::channel::<()>(1);
        let boot_lines = Arc::new(Mutex::new(Vec::new()));
        let recent = Arc::new(Mutex::new(VecDeque::new()));

        if let Some(out) = child.stdout.take() {
            pump(out, ready_tx.clone(), boot_lines.clone(), recent.clone());
        }
        if let Some(err) = child.stderr.take() {
            pump(err, ready_tx.clone(), boot_lines.clone(), recent.clone());
        }
        let stdin = child.stdin.take();

        tokio::spawn(async move {
            let status = tokio::select! {
                s = child.wait() => s,
                _ = kill_rx.recv() => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            };
            let desc = match status {
                Ok(s) => match s.code() {
                    Some(c) => format!("exit code {c}"),
                    None => "killed by signal".to_string(),
                },
                Err(e) => format!("wait failed: {e}"),
            };
            on_exit(desc.clone());
            let _ = exit_tx.send(Some(desc));
        });

        Ok(Arc::new(Self {
            stdin: tokio::sync::Mutex::new(stdin),
            kill_tx,
            exited: exit_rx,
            ready: ready_rx,
            boot_lines,
            recent,
            started: Instant::now(),
        }))
    }

    pub async fn send_line(&self, line: &str) -> Result<()> {
        let mut g = self.stdin.lock().await;
        let stdin = g.as_mut().context("stdin closed")?;
        stdin.write_all(format!("{line}\n").as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub fn kill(&self) {
        let _ = self.kill_tx.try_send(());
    }

    pub async fn wait_ready(&self) {
        let mut rx = self.ready.clone();
        let _ = rx.wait_for(|v| *v).await;
    }

    pub async fn wait_exit(&self) -> String {
        let mut rx = self.exited.clone();
        match rx.wait_for(|v| v.is_some()).await {
            Ok(v) => v.clone().unwrap_or_default(),
            Err(_) => "exited".to_string(),
        }
    }

    pub fn has_exited(&self) -> bool {
        self.exited.borrow().is_some()
    }

    /// Graceful stop via console `stop`, falling back to kill after `grace`.
    /// Returns (forced, exit description).
    pub async fn stop(&self, grace: Duration) -> (bool, String) {
        if self.has_exited() {
            return (false, self.wait_exit().await);
        }
        let _ = self.send_line("stop").await;
        match tokio::time::timeout(grace, self.wait_exit()).await {
            Ok(d) => (false, d),
            Err(_) => {
                tracing::warn!("server did not stop within {grace:?}; killing");
                self.kill();
                (true, self.wait_exit().await)
            }
        }
    }

    /// Error entries (with short stack traces) and warning count from the boot output.
    pub fn boot_summary(&self, max_errors: usize) -> (Vec<String>, usize, usize) {
        let text = self.boot_lines.lock().unwrap().join("\n");
        let entries = logs::entries(&text);
        let errs: Vec<_> = entries.iter().filter(|e| e.level == Level::Error).collect();
        let warns = entries.iter().filter(|e| e.level == Level::Warn).count();
        let list = errs
            .iter()
            .take(max_errors)
            .map(|e| logs::shorten(e, 8))
            .collect();
        (list, errs.len(), warns)
    }

    pub fn boot_text(&self) -> String {
        self.boot_lines.lock().unwrap().join("\n")
    }

    pub fn tail(&self, n: usize) -> Vec<String> {
        let r = self.recent.lock().unwrap();
        r.iter().skip(r.len().saturating_sub(n)).cloned().collect()
    }
}
