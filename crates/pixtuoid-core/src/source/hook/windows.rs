use std::ffi::c_void;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};
use tokio::sync::Semaphore;
use tracing::warn;
use windows_sys::Win32::Foundation::{LocalFree, ERROR_ACCESS_DENIED};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

use crate::source::TaggedSender;

use super::{handle_conn, CONN_TIMEOUT, MAX_CONCURRENT_CONNS};

/// Must cover the shim's whole stamped wire line — `STDIN_CAP` + the 256B
/// `STAMP_HEADROOM` in pixtuoid-hook are test-pinned to this 1MiB quota — so
/// the shim's sync write can't stall behind a busy daemon task.
const IN_BUFFER_SIZE: u32 = 1 << 20;

/// Owner-only security descriptor via SDDL `D:P(A;;GA;;;OW)` — the named-pipe
/// equivalent of the Unix socket's umask-0700, closing the default DACL's
/// Everyone-READ. Held alive for the daemon's lifetime so the raw-pointer
/// SECURITY_ATTRIBUTES stays valid at every create site.
struct OwnerOnlySd {
    psd: PSECURITY_DESCRIPTOR,
    attrs: SECURITY_ATTRIBUTES,
}

// SAFETY: the descriptor is immutable after creation (the Win32 calls only
// read through these pointers) and freed exactly once in Drop; none of the
// APIs involved carry thread affinity, so moving the owner across threads
// (tokio::spawn of the listener task) is sound.
unsafe impl Send for OwnerOnlySd {}

impl OwnerOnlySd {
    fn new() -> Result<Self> {
        let mut psd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: the SDDL literal is a valid NUL-terminated UTF-16 string,
        // psd is a live out-pointer, and the size out-param is documented
        // optional (null allowed).
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                windows_sys::w!("D:P(A;;GA;;;OW)"),
                SDDL_REVISION_1,
                &mut psd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(anyhow::Error::new(std::io::Error::last_os_error())
                .context("converting owner-only SDDL into a pipe security descriptor"));
        }
        Ok(Self {
            psd,
            attrs: SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: psd,
                bInheritHandle: 0,
            },
        })
    }

    /// Only ever read by CreateNamedPipeW, for the duration of that call.
    fn attributes_ptr(&self) -> *mut c_void {
        std::ptr::from_ref(&self.attrs).cast_mut().cast()
    }
}

impl Drop for OwnerOnlySd {
    fn drop(&mut self) {
        // SAFETY: psd was LocalAlloc'd by the SDDL conversion (documented
        // contract: caller frees with LocalFree) and is freed exactly once
        // here; no other reads can follow Drop.
        unsafe {
            LocalFree(self.psd);
        }
    }
}

pub(super) struct Listener {
    server: NamedPipeServer,
    name: String,
    sd: OwnerOnlySd,
}

/// `first` claims `first_pipe_instance`: ONLY the initial bind may, so a taken
/// name surfaces as the typed `SocketBusy`. The recreate + next-instance must
/// NOT claim it — the in-flight instance still holds it, and re-claiming fails
/// ACCESS_DENIED.
///
/// SAFETY: `attributes_ptr` must point at a well-formed `SECURITY_ATTRIBUTES`
/// whose `lpSecurityDescriptor` is valid for the duration of the call; the kernel
/// copies the descriptor during `CreateNamedPipeW`, so nothing borrows past it.
unsafe fn create_hook_pipe(
    name: &str,
    attributes_ptr: *mut c_void,
    first: bool,
) -> std::io::Result<NamedPipeServer> {
    let mut opts = ServerOptions::new();
    if first {
        opts.first_pipe_instance(true);
    }
    opts.reject_remote_clients(true)
        .pipe_mode(PipeMode::Byte)
        .in_buffer_size(IN_BUFFER_SIZE)
        .create_with_security_attributes_raw(name, attributes_ptr)
}

impl Listener {
    pub(super) async fn bind(path: &Path) -> Result<Self> {
        let name = path.to_string_lossy().into_owned();
        let sd = OwnerOnlySd::new()?;
        // The server stays DUPLEX (tokio default): the shim's client opens
        // read+write, so an inbound-only pipe would reject it with
        // ACCESS_DENIED — a silent event drop.
        //
        // SAFETY: sd outlives the call (it moves into Self below) and
        // attributes_ptr points at its well-formed SECURITY_ATTRIBUTES whose
        // lpSecurityDescriptor is the valid converted descriptor; the kernel
        // copies the descriptor during CreateNamedPipeW, so nothing borrows
        // past the call.
        let server = match unsafe { create_hook_pipe(&name, sd.attributes_ptr(), true) } {
            Ok(s) => s,
            // ERROR_ACCESS_DENIED is almost always another instance holding
            // first_pipe_instance on this name — the one recoverable bind
            // failure. A genuine ACL denial (restricted token / AppContainer)
            // is indistinguishable and also degrades to transcript-only;
            // accepted trade-off. Every other create error stays fatal.
            Err(e)
                if e.kind() == std::io::ErrorKind::PermissionDenied
                    || e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) =>
            {
                return Err(anyhow::Error::new(super::SocketBusy {
                    path: path.to_path_buf(),
                }));
            }
            Err(e) => {
                return Err(e).with_context(|| format!("creating hook pipe at {name}"));
            }
        };
        Ok(Self { server, name, sd })
    }

    pub(super) async fn run(
        mut self,
        tx: TaggedSender,
        pid_watch: Option<super::HookPidWatch>,
        presence_tx: Option<super::PresenceSender>,
    ) -> Result<()> {
        let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNS));
        loop {
            let permit = match Arc::clone(&sem).acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    anyhow::bail!("hook pipe semaphore closed unexpectedly");
                }
            };
            if let Err(e) = self.server.connect().await {
                // A failed instance isn't guaranteed reusable — recreate it.
                warn!(error = %e, "hook pipe connect error; recreating instance");
                // SAFETY: same contract as the bind site — `self.sd` outlives
                // the call and the kernel copies the descriptor during
                // CreateNamedPipeW, so nothing borrows past it.
                self.server =
                    unsafe { create_hook_pipe(&self.name, self.sd.attributes_ptr(), false) }
                        .with_context(|| {
                            format!("re-creating hook pipe after connect error at {}", self.name)
                        })?;
                continue;
            }
            // Create the NEXT instance BEFORE handing this one off: in the gap
            // between handoff and re-create, clients get ERROR_PIPE_BUSY or
            // NotFound depending on timing.
            //
            // SAFETY: same contract as the bind site — `self.sd` outlives the
            // call and the kernel copies the descriptor during
            // CreateNamedPipeW, so nothing borrows past it.
            let next = unsafe { create_hook_pipe(&self.name, self.sd.attributes_ptr(), false) }
                .with_context(|| format!("re-creating hook pipe at {}", self.name))?;
            let conn = std::mem::replace(&mut self.server, next);
            let tx = tx.clone();
            let pid_watch = pid_watch.clone();
            let presence_tx = presence_tx.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let _ = tokio::time::timeout(
                    CONN_TIMEOUT,
                    handle_conn(conn, tx, pid_watch, presence_tx),
                )
                .await;
            });
        }
    }
}
