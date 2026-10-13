//! The recorder's listener: the shim's endpoint under the daemon's own guards
//! (`source::hook`), each payload kept as received and teed to a headless
//! pixtuoid, on Unix and Windows alike.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::source::hook::{HookConn, HookSocketListener, MAX_CONN_BYTES, SocketBusy};

/// A hook can still be in flight when the CLI's own process exits, so the wait
/// is measured from the LAST PAYLOAD rather than from that exit.
const SETTLE: Duration = Duration::from_millis(500);
const QUIET_ROUNDS: u32 = 4;
const MAX_ROUNDS: u32 = 40;

/// How long a bound tap may take to start accepting.
const READY_BOUND: Duration = Duration::from_secs(5);
const READY_POLL: Duration = Duration::from_millis(10);

/// How long a send waits out a pipe whose every instance is mid-handshake.
#[cfg(windows)]
const PIPE_BUSY_BOUND: Duration = Duration::from_secs(2);
#[cfg(windows)]
const PIPE_BUSY_BACKOFF: Duration = Duration::from_millis(10);

/// Where a shim connects: a Unix socket, or a Windows named pipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint(PathBuf);

impl Endpoint {
    /// The endpoint `name` private to `dir`: a socket inside it, or on Windows a
    /// pipe named after it, since the pipe namespace is machine-wide.
    pub fn in_dir(dir: &Path, name: &str) -> Self {
        if cfg!(windows) {
            let tag = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Self(PathBuf::from(format!(r"\\.\pipe\pixtuoid-{tag}-{name}")))
        } else {
            Self(dir.join(format!("{name}.sock")))
        }
    }

    /// The endpoint a caller already named.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// What `PIXTUOID_SOCKET` and `pixtuoid run --socket` take.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Whether a server listens here, asked without connecting: a probe that
    /// connected would take a pipe's instance from the next client.
    pub fn accepting(&self) -> bool {
        #[cfg(windows)]
        {
            pipe_accepting(&self.0)
        }
        #[cfg(not(windows))]
        {
            self.0.exists()
        }
    }

    /// One connection carrying `bytes`, as the shim sends a payload.
    ///
    /// # Errors
    ///
    /// If nothing listens, or the write fails.
    pub(crate) async fn send(&self, bytes: &[u8]) -> std::io::Result<()> {
        #[cfg(windows)]
        let mut conn = {
            use tokio::net::windows::named_pipe::ClientOptions;
            let started = std::time::Instant::now();
            loop {
                match ClientOptions::new().open(&self.0) {
                    Ok(c) => break c,
                    Err(e)
                        if e.raw_os_error()
                            == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32)
                            && started.elapsed() < PIPE_BUSY_BOUND =>
                    {
                        tokio::time::sleep(PIPE_BUSY_BACKOFF).await;
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        #[cfg(not(windows))]
        let mut conn = tokio::net::UnixStream::connect(&self.0).await?;
        conn.write_all(bytes).await?;
        conn.shutdown().await
    }
}

/// `WaitNamedPipeW` "returns immediately" when no instance exists, and is
/// nonzero when "at least one instance of the pipe [is] available", connecting
/// none (learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-waitnamedpipew).
#[cfg(windows)]
fn pipe_accepting(name: &Path) -> bool {
    /// Long enough to find a free instance, short enough to poll with.
    const WAIT_MS: u32 = 1;
    let wide = super::host::nul_terminated(name.as_os_str());
    // SAFETY: `wide` is NUL-terminated and outlives the call, which only reads it.
    unsafe { windows_sys::Win32::System::Pipes::WaitNamedPipeW(wide.as_ptr(), WAIT_MS) != 0 }
}

/// Every payload a shim delivered to one endpoint, in arrival order.
#[derive(Debug)]
pub struct Tap {
    lines: Arc<Mutex<Vec<String>>>,
}

impl Tap {
    /// Listen at `at` on a runtime of its own, keeping each payload and teeing
    /// it to `tee`; returns once it accepts. A pipe accepts only once its server
    /// waits in `ConnectNamedPipe`, which fails with `ERROR_NO_DATA` for a client
    /// that already closed (learn.microsoft.com, ConnectNamedPipe): its payload
    /// is lost.
    ///
    /// # Errors
    ///
    /// `AddrInUse` if a live listener holds `at` (a squatted pipe name is one);
    /// any other bind failure as itself, or one that never starts accepting.
    pub fn bind(at: &Endpoint, tee: Option<Endpoint>) -> std::io::Result<Self> {
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let ready = at.clone();
        let at = at.path().to_path_buf();
        let (bound_tx, bound_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = bound_tx.send(Err(e));
                    return;
                }
            };
            rt.block_on(async move {
                let listener = match HookSocketListener::bind(&at).await {
                    Ok(l) => l,
                    Err(e) => {
                        let kind = if e.is::<SocketBusy>() {
                            std::io::ErrorKind::AddrInUse
                        } else {
                            std::io::ErrorKind::Other
                        };
                        let _ = bound_tx.send(Err(std::io::Error::new(kind, format!("{e:#}"))));
                        return;
                    }
                };
                let _ = bound_tx.send(Ok(()));
                let tee = Arc::new(tee);
                let _ = listener
                    .serve_raw(move |conn| keep(conn, Arc::clone(&sink), Arc::clone(&tee)))
                    .await;
            });
        });
        bound_rx.recv().map_err(std::io::Error::other)??;
        let started = std::time::Instant::now();
        while !ready.accepting() {
            if started.elapsed() > READY_BOUND {
                return Err(std::io::Error::other(format!(
                    "{} never started accepting",
                    ready.path().display()
                )));
            }
            std::thread::sleep(READY_POLL);
        }
        Ok(Self { lines })
    }

    fn len(&self) -> usize {
        self.lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Wait until no payload has arrived for [`QUIET_ROUNDS`] × [`SETTLE`].
    pub fn settle(&self) {
        let (mut prev, mut quiet, mut round) = (usize::MAX, 0, 0);
        while quiet < QUIET_ROUNDS && round < MAX_ROUNDS {
            let n = self.len();
            quiet = if n == prev { quiet + 1 } else { 0 };
            prev = n;
            round += 1;
            std::thread::sleep(SETTLE);
        }
    }

    /// Every payload so far.
    pub fn take(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Keep one connection's payload. The tee runs detached, outside the
/// listener's per-connection budget, so a slow headless pixtuoid never costs
/// the recording a payload; a tee that cannot connect is its own failure, left
/// in its scene log.
async fn keep(mut conn: HookConn, sink: Arc<Mutex<Vec<String>>>, tee: Arc<Option<Endpoint>>) {
    let mut bytes = Vec::new();
    match (&mut conn)
        .take(MAX_CONN_BYTES)
        .read_to_end(&mut bytes)
        .await
    {
        // A pipe whose writer closed reads as `ERROR_BROKEN_PIPE`, not as an end
        // (learn.microsoft.com, ReadFile, "Pipes"); what came before it stands.
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => return,
        _ => {}
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if text.trim().is_empty() {
        return;
    }
    sink.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(text);
    if let Some(to) = tee.as_ref().clone() {
        tokio::spawn(async move {
            let _ = to.send(&bytes).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "{\"hook_event_name\":\"SessionStart\"}\n";

    fn private_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[tokio::test]
    async fn a_payload_is_kept_and_teed() {
        let d = private_dir();
        let tee_at = Endpoint::in_dir(d.path(), "tee");
        let tee = Tap::bind(&tee_at, None).expect("tee");
        let at = Endpoint::in_dir(d.path(), "tap");
        let tap = Tap::bind(&at, Some(tee_at)).expect("tap");
        at.send(LINE.as_bytes()).await.expect("send");
        tap.settle();
        tee.settle();
        assert_eq!(tap.take(), [LINE]);
        assert_eq!(tee.take(), [LINE]);
    }

    #[tokio::test]
    async fn accepting_turns_true_on_bind_and_takes_no_instance() {
        let d = private_dir();
        let at = Endpoint::in_dir(d.path(), "tap");
        assert!(!at.accepting(), "nothing listens yet");
        let tap = Tap::bind(&at, None).expect("tap");
        assert!(at.accepting());
        assert!(
            at.accepting(),
            "a probe leaves the instance for the next one"
        );
        at.send(LINE.as_bytes()).await.expect("send");
        tap.settle();
        assert_eq!(tap.take(), [LINE]);
    }

    #[test]
    fn a_second_tap_on_one_endpoint_is_refused_as_in_use() {
        let d = private_dir();
        let at = Endpoint::in_dir(d.path(), "tap");
        let _first = Tap::bind(&at, None).expect("first");
        let second = Tap::bind(&at, None).expect_err("a live listener holds it");
        assert_eq!(second.kind(), std::io::ErrorKind::AddrInUse, "{second}");
    }

    #[tokio::test]
    async fn a_tee_with_no_listener_costs_the_recording_nothing() {
        let d = private_dir();
        let at = Endpoint::in_dir(d.path(), "tap");
        let tap = Tap::bind(&at, Some(Endpoint::in_dir(d.path(), "absent"))).expect("tap");
        at.send(LINE.as_bytes()).await.expect("send");
        tap.settle();
        assert_eq!(tap.take(), [LINE]);
    }

    /// The hook server always pre-creates its next instance, so only a server
    /// of exactly one shows the probe reading "every instance is taken".
    #[cfg(windows)]
    #[tokio::test]
    async fn accepting_reads_a_single_instance_server() {
        use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
        let d = private_dir();
        let at = Endpoint::in_dir(d.path(), "single");
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .max_instances(1)
            .create(at.path())
            .expect("server");
        assert!(at.accepting(), "its one instance is free");
        let _client = ClientOptions::new().open(at.path()).expect("client");
        server.connect().await.expect("connected");
        assert!(!at.accepting(), "its one instance is taken");
    }
}
