use std::ffi::c_void;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};
use tokio::sync::Semaphore;
use tracing::warn;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

use crate::source::TaggedSender;

use super::{handle_conn, CONN_TIMEOUT, MAX_CONCURRENT_CONNS};

/// In-buffer must cover the shim's 1MiB stdin cap (pixtuoid-hook main.rs
/// `take(1 << 20)`) so one payload always fits the pipe quota and the shim's
/// sync write can't stall behind a momentarily busy daemon task.
const IN_BUFFER_SIZE: u32 = 1 << 20;

/// Owner-only security descriptor via SDDL `D:P(A;;GA;;;OW)` — protected
/// DACL, single ACE granting GENERIC_ALL to OWNER_RIGHTS (the creating
/// user). The named-pipe equivalent of the Unix socket's umask-0700: closes
/// the default DACL's Everyone-READ while keeping the owner fully able to
/// connect. Held alive for the daemon's lifetime; the kernel copies the
/// descriptor at each CreateNamedPipe, but keeping the allocation around
/// makes the raw-pointer SECURITY_ATTRIBUTES trivially valid at every
/// create site.
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

    /// Pointer for tokio's `create_with_security_attributes_raw`. Only ever
    /// read by CreateNamedPipeW for the duration of that call.
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

impl Listener {
    pub(super) async fn bind(path: &Path) -> Result<Self> {
        let name = path.to_string_lossy().into_owned();
        let sd = OwnerOnlySd::new()?;
        // first_pipe_instance: if another process already owns this name
        // (squatting), creation fails ACCESS_DENIED — bail loudly rather
        // than silently queueing behind an impostor. reject_remote_clients
        // is the tokio default; pinned here explicitly. The server stays
        // DUPLEX (tokio default) — the shim's client opens read+write, so an
        // inbound-only pipe would reject it with ACCESS_DENIED (silent event
        // drop).
        //
        // SAFETY: sd outlives the call (it moves into Self below) and
        // attributes_ptr points at its well-formed SECURITY_ATTRIBUTES whose
        // lpSecurityDescriptor is the valid converted descriptor; the kernel
        // copies the descriptor during CreateNamedPipeW, so nothing borrows
        // past the call.
        let server = unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .pipe_mode(PipeMode::Byte)
                .in_buffer_size(IN_BUFFER_SIZE)
                .create_with_security_attributes_raw(&name, sd.attributes_ptr())
        }
        .with_context(|| format!("creating hook pipe at {name}"))?;
        Ok(Self { server, name, sd })
    }

    pub(super) async fn run(mut self, tx: TaggedSender) -> Result<()> {
        let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNS));
        loop {
            let permit = match Arc::clone(&sem).acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    anyhow::bail!("hook pipe semaphore closed unexpectedly");
                }
            };
            if let Err(e) = self.server.connect().await {
                // A failed instance isn't guaranteed reusable (tokio's own
                // accept-loop pattern propagates connect errors for this
                // reason) — recreate it; if THAT fails the error converges
                // with the recreate-bail below. Unix accept errors leave the
                // listener fd valid, hence its plain warn+continue.
                warn!("hook pipe connect error: {e}; recreating instance");
                // SAFETY: self.sd lives for the whole loop; same descriptor
                // validity argument as bind().
                self.server = unsafe {
                    ServerOptions::new()
                        .reject_remote_clients(true)
                        .pipe_mode(PipeMode::Byte)
                        .in_buffer_size(IN_BUFFER_SIZE)
                        .create_with_security_attributes_raw(&self.name, self.sd.attributes_ptr())
                }
                .with_context(|| {
                    format!("re-creating hook pipe after connect error at {}", self.name)
                })?;
                continue;
            }
            // Create the NEXT instance BEFORE handing this one off —
            // tokio's documented pattern; in the gap between handoff and
            // re-create, clients would get ERROR_PIPE_BUSY or NotFound
            // depending on timing.
            //
            // SAFETY: self.sd lives for the whole loop; same descriptor
            // validity argument as bind().
            let next = unsafe {
                ServerOptions::new()
                    .reject_remote_clients(true)
                    .pipe_mode(PipeMode::Byte)
                    .in_buffer_size(IN_BUFFER_SIZE)
                    .create_with_security_attributes_raw(&self.name, self.sd.attributes_ptr())
            }
            .with_context(|| format!("re-creating hook pipe at {}", self.name))?;
            let conn = std::mem::replace(&mut self.server, next);
            let tx = tx.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let _ = tokio::time::timeout(CONN_TIMEOUT, handle_conn(conn, tx)).await;
            });
        }
    }
}
