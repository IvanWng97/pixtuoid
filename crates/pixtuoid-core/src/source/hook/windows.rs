use std::ffi::c_void;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};
use tokio::sync::Semaphore;
use tracing::warn;
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

use crate::source::TaggedSender;

use super::{CONN_TIMEOUT, MAX_CONCURRENT_CONNS, handle_conn};

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

impl std::fmt::Debug for OwnerOnlySd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerOnlySd")
            .field("psd", &self.psd)
            .finish_non_exhaustive()
    }
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

    /// A hook pipe instance under this descriptor. `first` claims
    /// `first_pipe_instance`: ONLY the initial bind may, so a taken name surfaces
    /// as the typed `SocketBusy`. The recreate + next-instance must NOT claim it
    /// — the in-flight instance still holds it, and re-claiming fails
    /// ACCESS_DENIED.
    fn create_pipe(&self, name: &str, first: bool) -> std::io::Result<NamedPipeServer> {
        let attrs: *mut c_void = std::ptr::from_ref(&self.attrs).cast_mut().cast();
        // SAFETY: `attrs` is the well-formed SECURITY_ATTRIBUTES whose descriptor
        // `self` owns, and the borrow of `self` outlives the call; the kernel
        // copies the descriptor during CreateNamedPipeW, so nothing borrows past it.
        unsafe { hook_pipe_options(first).create_with_security_attributes_raw(name, attrs) }
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

#[derive(Debug)]
pub(super) struct Listener {
    server: NamedPipeServer,
    name: String,
    sd: OwnerOnlySd,
}

fn hook_pipe_options(first: bool) -> ServerOptions {
    let mut opts = ServerOptions::new();
    opts.first_pipe_instance(first)
        .reject_remote_clients(true)
        .pipe_mode(PipeMode::Byte)
        .in_buffer_size(IN_BUFFER_SIZE);
    opts
}

impl Listener {
    pub(super) async fn bind(path: &Path) -> Result<Self> {
        let name = path.to_string_lossy().into_owned();
        let sd = OwnerOnlySd::new()?;
        // The server stays DUPLEX (tokio default): the shim's client opens
        // read+write, so an inbound-only pipe would reject it with
        // ACCESS_DENIED — a silent event drop.
        let server = match sd.create_pipe(&name, true) {
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
                self.server = self.sd.create_pipe(&self.name, false).with_context(|| {
                    format!("re-creating hook pipe after connect error at {}", self.name)
                })?;
                continue;
            }
            // Create the NEXT instance BEFORE handing this one off: in the gap
            // between handoff and re-create, clients get ERROR_PIPE_BUSY or
            // NotFound depending on timing.
            let next = self
                .sd
                .create_pipe(&self.name, false)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

    /// No API reads `PIPE_REJECT_REMOTE_CLIENTS` back off a pipe — neither
    /// [`GetNamedPipeInfo`](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-getnamedpipeinfo)
    /// nor [`GetNamedPipeHandleState`](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-getnamedpipehandlestatew)
    /// reports it — so the options are read where they are built.
    #[test]
    fn the_hook_pipe_rejects_remote_clients() {
        for first in [true, false] {
            let opts = format!("{:?}", hook_pipe_options(first));
            assert!(opts.contains("reject_remote_clients: true"), "{opts}");
            assert!(
                opts.contains(&format!("first_pipe_instance: {first}")),
                "{opts}"
            );
        }
    }

    /// The DACL the kernel holds for the created pipe, as SDDL.
    fn dacl_sddl(server: &NamedPipeServer) -> String {
        let mut psd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: a live pipe handle; every out-pointer this call doesn't fill
        // is null, as documented optional, and `psd` is a live out-pointer.
        let rc = unsafe {
            GetSecurityInfo(
                server.as_raw_handle(),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut psd,
            )
        };
        assert_eq!(rc, 0, "GetSecurityInfo");
        let mut text: windows_sys::core::PWSTR = std::ptr::null_mut();
        // SAFETY: `psd` is the descriptor read above; both out-pointers are live.
        let ok = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                psd,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(
            ok, 0,
            "ConvertSecurityDescriptorToStringSecurityDescriptorW"
        );
        // SAFETY: `text` is the NUL-terminated string the conversion allocated.
        let sddl = String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(text, (0..).take_while(|&i| *text.add(i) != 0).count())
        });
        // SAFETY: both were LocalAlloc'd by the calls above, freed once here.
        unsafe {
            LocalFree(text.cast());
            LocalFree(psd);
        }
        sddl
    }

    /// One protected ACE, for the owner alone, read back off the pipe `bind`
    /// creates: a bind that drops the descriptor reads the default DACL here.
    #[tokio::test]
    async fn the_hook_pipe_is_owner_only() {
        let name = format!(r"\\.\pipe\pixtuoid-sd-{}", std::process::id());
        let listener = Listener::bind(Path::new(&name)).await.expect("bind");
        let sddl = dacl_sddl(&listener.server);
        let (flags, aces) = sddl.split_once('(').expect("an ACE");
        assert!(flags.starts_with("D:") && flags.contains('P'), "{sddl}");
        assert_eq!(aces.matches('(').count(), 0, "one ACE: {sddl}");
        assert!(
            aces.starts_with("A;;") && aces.ends_with(";;;OW)"),
            "{sddl}"
        );
    }
}
