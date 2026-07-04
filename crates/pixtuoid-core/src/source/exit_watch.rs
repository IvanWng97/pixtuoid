//! Instant process-exit detection (#223 rung 2): the kernel tells us the
//! moment a probed agent process dies, instead of the negative vouch's
//! ~60–120s or the reducer's TTL/stale sweeps.
//!
//! One `ExitWatch` = one kernel wait primitive + ONE detached `std::thread`
//! blocked on it (macOS: a `kqueue` with `EVFILT_PROC NOTE_EXIT` knotes;
//! Linux: `poll` over `pidfd_open` descriptors plus a self-pipe). NOT
//! `tokio::spawn_blocking` — a forever-loop would pin a blocking-pool slot
//! for the process lifetime.
//!
//! Lifecycle: `watch(pid)` pushes the pid onto a shared pending queue and
//! WAKES the blocked wait (macOS: an `EVFILT_USER` ident-0 trigger; Linux: a
//! self-pipe byte); the thread drains the queue and registers each pid with
//! the kernel. Exits flow back as the raw pid on a
//! `tokio::sync::mpsc::UnboundedSender` (send is sync-callable from a plain
//! thread). The thread exits when (a) `ExitWatch::Drop` sets `closed` and
//! wakes it, (b) an exit send fails (receiver gone — the watcher task
//! ended), or (c) the backend dies (kevent/poll error, pidfd ENOSYS); every
//! exit path marks `closed` on the way out so `watch()` rejects instead of
//! queueing onto a queue nothing will ever drain. The wait primitive's fds
//! live in the shared `Arc` state, so the waker side can never touch a
//! recycled fd after the thread closes up.
//!
//! Contract (workspace invariant: log + continue, never panic): every
//! failure path — backend init, registration, thread death — degrades to
//! the slower exit rungs (negative vouch ~60–120s, ProofOfLife TTL lapse,
//! stale sweeps). A missing instant exit costs latency, never correctness.
//! Registration races are resolved at the primitive: a pid that died BEFORE
//! registration is detected right there (ESRCH) and its exit synthesized
//! immediately; EPERM is dropped silently. On delivery the macOS knote
//! self-removes (`EV_ONESHOT`) and the Linux pidfd is closed, and the pid
//! leaves the watched set — so a recycled pid can be re-watched for a new
//! session later.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::mpsc::UnboundedSender;

pub(crate) struct ExitWatch {
    shared: Arc<Shared>,
}

/// State shared between the registering side (`watch`/`Drop`) and the
/// watcher thread. The platform `Backend` (the wait/wake fds) lives here so
/// it closes only when BOTH sides have dropped their `Arc` — never while the
/// other might still poke it.
struct Shared {
    /// Pids queued by `watch()`, drained by the thread on each wake.
    pending: Mutex<Vec<i32>>,
    /// Set by `Drop`; the thread exits on its next wake.
    closed: AtomicBool,
    backend: imp::Backend,
}

/// A poisoned lock only means another thread panicked mid-push of an `i32` —
/// the Vec is still structurally sound, so take the guard anyway (the shim
/// must never panic; see the module contract).
fn lock_pending(shared: &Shared) -> std::sync::MutexGuard<'_, Vec<i32>> {
    shared
        .pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Take everything `watch()` queued since the last wake, DEDUPED. Shared by
/// the platform run loops. The dedup is load-bearing for the AlreadyDead
/// path: a duplicate `watch()` of a pid that died before the drain would
/// otherwise register → ESRCH → send, then re-register the batch's second
/// copy (the first ESRCH already removed it from `watched`) and synthesize
/// a SECOND exit (caught by `watch_is_idempotent`).
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn drain_pending(shared: &Shared) -> Vec<i32> {
    let mut batch = std::mem::take(&mut *lock_pending(shared));
    batch.sort_unstable();
    batch.dedup();
    batch
}

impl ExitWatch {
    /// Spawns the watcher thread. `exit_tx` receives the pid of every watched
    /// process that exits (or was already dead at registration). Returns
    /// `None` when the platform backend failed to init (kqueue()/pipe2
    /// failed) or the platform has no validated primitive — callers treat
    /// that as "instant exit off", with the slower rungs covering.
    pub(crate) fn spawn(exit_tx: UnboundedSender<i32>) -> Option<Self> {
        let backend = imp::Backend::init()?;
        let shared = Arc::new(Shared {
            pending: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
            backend,
        });
        let thread_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("pixtuoid-exit-watch".into())
            .spawn(move || {
                imp::run(&thread_shared, &exit_tx);
                // EVERY run() exit path (kevent/poll error, pidfd ENOSYS,
                // receiver gone, Drop) lands here: mark the handle dead so
                // `watch()` rejects instead of queueing — long-lived
                // producers (HookPidWatch::note, PresenceExitWatch::watch)
                // push per hook event for the process lifetime, and with no
                // drainer `pending` would grow unboundedly. Clear what the
                // final drain left behind; a push racing the store can leave
                // at most one stray batch (bounded, not growth).
                thread_shared.closed.store(true, Ordering::SeqCst);
                lock_pending(&thread_shared).clear();
            });
        if let Err(e) = spawned {
            tracing::debug!(
                "exit-watch thread spawn failed: {e}; instant exit off (backstops cover)"
            );
            return None;
        }
        Some(Self { shared })
    }

    /// Ask the thread to watch `pid`. Idempotent: watching an already-watched
    /// live pid is a no-op (the thread dedups against its watched set).
    /// Never blocks beyond the queue lock; once the thread has died (`closed`
    /// set by its exit hook) the pid is rejected outright — nothing would
    /// ever drain it, so pushing would only grow `pending` (backstops cover).
    pub(crate) fn watch(&self, pid: i32) {
        if self.shared.closed.load(Ordering::SeqCst) {
            return;
        }
        lock_pending(&self.shared).push(pid);
        self.shared.backend.wake();
    }
}

impl Drop for ExitWatch {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        self.shared.backend.wake();
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::HashSet;
    use std::mem::MaybeUninit;
    use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
    use std::ptr;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use rustix::event::kqueue::{
        kevent, kqueue, Event, EventFilter, EventFlags, ProcessEvents, UserDefinedFlags, UserFlags,
    };
    use rustix::io::Errno;
    use rustix::process::Pid;
    use tokio::sync::mpsc::UnboundedSender;

    use super::{drain_pending, Shared};

    /// Per-wait event budget. More than 16 simultaneous deliveries just take
    /// another loop turn — kqueue events are queued state, never lost.
    const EVENT_BUF: usize = 16;

    /// The `EVFILT_USER` wake slot ident. A pure user-triggered wake — 0
    /// carries no fd/pid, it's just the key we register once and trigger.
    const WAKE_IDENT: isize = 0;

    /// The `EVFILT_USER` ident-0 wake filter. Registration and fire differ only
    /// in the fflags: registration carries no `NOTE_TRIGGER` (paired with
    /// `EV_ADD | EV_CLEAR` at the call site), the fire carries `NOTE_TRIGGER`.
    fn wake_filter(trigger: bool) -> EventFilter {
        EventFilter::User {
            ident: WAKE_IDENT,
            flags: if trigger {
                UserFlags::TRIGGER
            } else {
                UserFlags::empty()
            },
            user_flags: UserDefinedFlags::new(0),
        }
    }

    pub(super) struct Backend {
        /// One kqueue for the whole watch: the `EVFILT_PROC NOTE_EXIT` knotes
        /// plus the `EVFILT_USER` ident-0 wake slot. Owned HERE (reached by
        /// both sides through `Arc<Shared>`) so the fd closes only when both
        /// the waker and the thread are gone — `wake()` can never hit a
        /// recycled fd (tokio's own selector is a kqueue; triggering a user
        /// event on a recycled fd would corrupt a foreign event loop).
        kq: OwnedFd,
    }

    impl Backend {
        pub(super) fn init() -> Option<Self> {
            let kq = match kqueue() {
                Ok(kq) => kq,
                Err(e) => {
                    tracing::debug!("kqueue() failed: {e}; instant exit off (backstops cover)");
                    return None;
                }
            };
            // The wake slot: EVFILT_USER ident 0. EV_CLEAR so each delivered
            // NOTE_TRIGGER auto-resets the event for the next trigger.
            let wake_slot = Event::new(
                wake_filter(false),
                EventFlags::ADD | EventFlags::CLEAR,
                ptr::null_mut(),
            );
            // An empty eventlist ⇒ nevents 0: the kernel processes the change
            // and writes nothing back.
            let eventlist: &mut [Event] = &mut [];
            // SAFETY: the EVFILT_USER change references no fd (ident 0 is a pure
            // user-event key), so the kqueue-fd-validity contract is trivially
            // met; the zero timeout means this can't block.
            let registered = unsafe { kevent(&kq, &[wake_slot], eventlist, Some(Duration::ZERO)) };
            if let Err(e) = registered {
                tracing::debug!("EVFILT_USER wake-slot registration failed: {e}; instant exit off");
                return None;
            }
            Some(Self { kq })
        }

        /// Trigger the EVFILT_USER ident-0 slot, waking a blocked `kevent`
        /// wait. kevent(2) is documented thread-safe on one kq, so the
        /// registering side may call this while the thread is blocked.
        pub(super) fn wake(&self) {
            let trigger = Event::new(wake_filter(true), EventFlags::empty(), ptr::null_mut());
            let eventlist: &mut [Event] = &mut [];
            // SAFETY: the trigger references no fd; the empty eventlist + zero
            // timeout write nothing and never block. Cannot ENOENT — the slot
            // is registered at init and never deleted while the kq lives.
            if let Err(e) = unsafe { kevent(&self.kq, &[trigger], eventlist, Some(Duration::ZERO)) }
            {
                tracing::debug!("exit-watch wake trigger failed: {e}");
            }
        }
    }

    enum Registered {
        Ok,
        /// The pid died before registration (ESRCH receipt) — the caller
        /// synthesizes the exit immediately.
        AlreadyDead,
        /// EPERM / anything else: dropped (logged); backstops cover.
        Failed,
    }

    /// Register a NOTE_EXIT knote for `pid`, resolving the registration race
    /// via EV_RECEIPT: the receipt entry comes back in the SAME call (flags
    /// carry EV_ERROR; data is 0 on success, the errno otherwise), so a pid
    /// that died before this call is detected HERE rather than silently
    /// never firing (prior art: Irrlicht; xnu-verified). NOTE_EXITSTATUS is
    /// deliberately NOT requested — xnu's filt_procattach has no credential
    /// check for plain NOTE_EXIT, so same-user non-children (every CC/codex
    /// session) are watchable unprivileged; the exit STATUS is child-only
    /// and unneeded.
    fn register(kq: BorrowedFd, pid: i32) -> Registered {
        let Some(rpid) = Pid::from_raw(pid) else {
            return Registered::Failed; // pid <= 0 is never a real process
        };
        let change = Event::new(
            EventFilter::Proc {
                pid: rpid,
                flags: ProcessEvents::EXIT,
            },
            // EV_ONESHOT: NOTE_EXIT can only fire once per process; the knote
            // self-removes on delivery, so no post-fire cleanup exists.
            EventFlags::ADD | EventFlags::ONESHOT | EventFlags::RECEIPT,
            ptr::null_mut(),
        );
        // One receipt slot — EV_RECEIPT guarantees the receipt fills it ahead
        // of any pending event.
        let mut slot = [const { MaybeUninit::<Event>::uninit() }; 1];
        // SAFETY: the EVFILT_PROC change references a pid, not an fd, so the
        // kqueue-fd-validity contract is trivially met; the zero timeout can
        // never block the loop.
        let receipt = match unsafe { kevent(kq, &[change], &mut slot, Some(Duration::ZERO)) } {
            Ok((got, _rest)) => got,
            Err(e) => {
                tracing::debug!(
                    "EVFILT_PROC registration for pid {pid} failed: {e}; dropped (backstops cover)"
                );
                return Registered::Failed;
            }
        };
        // No receipt, or one without EV_ERROR / with data 0 = success.
        let Some(ev) = receipt.first() else {
            return Registered::Ok;
        };
        let data = ev.data();
        if !ev.flags().contains(EventFlags::ERROR) || data == 0 {
            return Registered::Ok;
        }
        let errno = Errno::from_raw_os_error(data as i32);
        if errno == Errno::SRCH {
            Registered::AlreadyDead
        } else if errno == Errno::PERM {
            tracing::debug!("EVFILT_PROC EPERM for pid {pid}; dropped (backstops cover)");
            Registered::Failed
        } else {
            tracing::debug!("EVFILT_PROC registration for pid {pid} returned {errno:?}; dropped");
            Registered::Failed
        }
    }

    pub(super) fn run(shared: &Shared, exit_tx: &UnboundedSender<i32>) {
        let kq = shared.backend.kq.as_fd();
        let mut watched: HashSet<i32> = HashSet::new();
        loop {
            let mut buf = [const { MaybeUninit::<Event>::uninit() }; EVENT_BUF];
            // SAFETY: the empty changelist references no fds; the None timeout
            // blocks until a NOTE_EXIT delivery or a NOTE_TRIGGER wake (both are
            // queued kqueue state, so a wake sent before this call is still
            // picked up — no lost wakeups).
            let events = match unsafe { kevent(kq, &[], &mut buf, None) } {
                Ok((events, _rest)) => events,
                Err(e) if e == Errno::INTR => continue,
                Err(e) => {
                    tracing::debug!(
                        "exit-watch kevent wait failed: {e}; thread exiting (backstops cover)"
                    );
                    return;
                }
            };
            if shared.closed.load(Ordering::SeqCst) {
                // ExitWatch dropped. The kq closes when the last Arc<Shared>
                // (ours, on return) drops.
                return;
            }
            // Drain pending BEFORE processing exit deliveries: a duplicate
            // watch() whose NOTE_TRIGGER is batched alongside (or queued
            // behind) the pid's NOTE_EXIT must dedup against the still-
            // watched entry — events-first would remove the entry, then
            // re-register the drained duplicate against a dead pid and
            // synthesize a SECOND exit (caught by watch_is_idempotent).
            for pid in drain_pending(shared) {
                if !watched.insert(pid) {
                    continue; // idempotent: already watched
                }
                match register(kq, pid) {
                    Registered::Ok => {}
                    Registered::AlreadyDead => {
                        watched.remove(&pid);
                        if exit_tx.send(pid).is_err() {
                            return; // receiver gone — the watcher task ended
                        }
                    }
                    Registered::Failed => {
                        watched.remove(&pid);
                    }
                }
            }
            for ev in events.iter() {
                if let EventFilter::Proc { pid, flags } = ev.filter() {
                    if flags.contains(ProcessEvents::EXIT) {
                        let pid = pid.as_raw_pid();
                        // The knote already self-removed (EV_ONESHOT); drop our
                        // bookkeeping so a recycled pid can be re-watched.
                        watched.remove(&pid);
                        if exit_tx.send(pid).is_err() {
                            return;
                        }
                    }
                }
                // EVFILT_USER ident 0 is the wake: no payload, the pending
                // drain above is its handler.
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
    use std::sync::atomic::Ordering;

    use rustix::event::{poll, PollFd, PollFlags};
    use rustix::io::Errno;
    use rustix::pipe::{pipe_with, PipeFlags};
    use rustix::process::{pidfd_open, Pid, PidfdFlags};
    use tokio::sync::mpsc::UnboundedSender;

    use super::{drain_pending, Shared};

    pub(super) struct Backend {
        /// Self-pipe: `wake()` writes a byte; the thread keeps the read end
        /// in its poll set and drains on wake. Both ends are O_NONBLOCK — a
        /// full pipe means a wake is already pending (EAGAIN is success),
        /// and the drain reads until EAGAIN. Owned HERE (reached through
        /// `Arc<Shared>`) so neither side can ever touch a recycled fd.
        pipe_rd: OwnedFd,
        pipe_wr: OwnedFd,
    }

    impl Backend {
        pub(super) fn init() -> Option<Self> {
            let (pipe_rd, pipe_wr) = match pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK) {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::debug!("pipe2 failed: {e}; instant exit off (backstops cover)");
                    return None;
                }
            };
            Some(Self { pipe_rd, pipe_wr })
        }

        pub(super) fn wake(&self) {
            // O_NONBLOCK, so a full pipe returns EAGAIN instead of blocking —
            // which is success for our purposes (a wake is already pending).
            let _ = rustix::io::write(&self.pipe_wr, &[1]);
        }
    }

    enum PidfdOpen {
        Opened(OwnedFd),
        /// ESRCH — the pid died (and was reaped) before registration; the
        /// caller synthesizes the exit immediately.
        AlreadyDead,
        /// ENOSYS — pre-5.3 kernel; the whole backend is unavailable.
        Unsupported,
        /// Anything else: dropped (logged); backstops cover.
        Failed,
    }

    /// `pidfd_open(2)` via `rustix::process::pidfd_open` — a safe wrapper over
    /// the syscall (libc 0.2 ships none), returning an `OwnedFd` directly.
    fn pidfd_open_for(pid: i32) -> PidfdOpen {
        let Some(rpid) = Pid::from_raw(pid) else {
            return PidfdOpen::Failed; // pid <= 0 is never a real process
        };
        match pidfd_open(rpid, PidfdFlags::empty()) {
            Ok(fd) => PidfdOpen::Opened(fd),
            Err(e) if e == Errno::SRCH => PidfdOpen::AlreadyDead,
            Err(e) if e == Errno::NOSYS => PidfdOpen::Unsupported,
            Err(e) => {
                tracing::debug!("pidfd_open({pid}) failed: {e}; dropped (backstops cover)");
                PidfdOpen::Failed
            }
        }
    }

    /// Empty the self-pipe so the next wake byte makes it readable again.
    fn drain_pipe(pipe_rd: BorrowedFd) {
        let mut buf = [0u8; 64];
        loop {
            // O_NONBLOCK so this never blocks (EAGAIN ends the drain); a short
            // read means the pipe is now empty.
            match rustix::io::read(pipe_rd, &mut buf) {
                Ok(n) if n == buf.len() => continue,
                _ => return,
            }
        }
    }

    pub(super) fn run(shared: &Shared, exit_tx: &UnboundedSender<i32>) {
        let pipe_rd = shared.backend.pipe_rd.as_fd();
        // pid → its pidfd, in poll-set order: slot 0 of `fds` is always the
        // pipe, slot i+1 is watched[i] (pushes only append, so the zip below
        // stays aligned with the fds snapshot taken before any mutation).
        let mut watched: Vec<(i32, OwnedFd)> = Vec::new();
        loop {
            // Re-check `closed` BEFORE blocking in poll(). `Drop` sets `closed`
            // then writes a wake byte, but `drain_pipe` below drains the pipe
            // unconditionally — so a Drop landing after the post-poll `closed`
            // check (further down) but before that drain has its wake byte
            // EATEN, leaving `closed = true` with an empty pipe. Without this
            // top check the thread would re-enter poll() and block on a still-
            // live watched pid until it exits (or forever) — the self-pipe
            // lost-wakeup that flaked `drop_stops_cleanly` on the Linux CI.
            // SeqCst pairs with Drop's store-before-wake: if the drain consumed
            // the byte, this load is guaranteed to observe `true`. (macOS is
            // immune — its EVFILT_USER trigger is consumed by kevent() itself,
            // no separate drain to eat it.)
            if shared.closed.load(Ordering::SeqCst) {
                return;
            }
            let mut fds: Vec<PollFd> = Vec::with_capacity(watched.len() + 1);
            fds.push(PollFd::from_borrowed_fd(pipe_rd, PollFlags::IN));
            for (_, pidfd) in &watched {
                fds.push(PollFd::from_borrowed_fd(pidfd.as_fd(), PollFlags::IN));
            }
            // -1 (None timeout) blocks until a process exit or a wake byte.
            match poll(&mut fds, None) {
                Ok(_) => {}
                Err(e) if e == Errno::INTR => continue,
                Err(e) => {
                    tracing::debug!(
                        "exit-watch poll failed: {e}; thread exiting (backstops cover)"
                    );
                    return;
                }
            }
            if shared.closed.load(Ordering::SeqCst) {
                // ExitWatch dropped. Pidfds + pipe close with their owners.
                return;
            }
            // Collect exits from the PRE-mutation alignment (fds[i+1] ↔
            // watched[i]); pushes below only APPEND, and the zip truncates at
            // the fds snapshot's length, so the pairing stays aligned. A
            // pidfd polls POLLIN when the process exits (zombie) and POLLHUP
            // once reaped; ERR/NVAL are defensive — all four mean "this
            // watch is over" (prior art: Irrlicht).
            let pipe_woke = !fds[0].revents().is_empty();
            let exited: Vec<i32> = fds[1..]
                .iter()
                .zip(watched.iter())
                .filter(|(slot, _)| {
                    slot.revents().intersects(
                        PollFlags::IN | PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL,
                    )
                })
                .map(|(_, (pid, _))| *pid)
                .collect();
            // `fds` borrows `watched` (and `pipe_rd`); drop it before mutating
            // the set below.
            drop(fds);
            // Drain pending BEFORE removing/sending the exits: a duplicate
            // watch() arriving in the same wake as its pid's exit must dedup
            // against the still-watched entry — exits-first would remove it,
            // then pidfd_open the drained duplicate on a dead pid and
            // synthesize a SECOND exit (caught by watch_is_idempotent).
            if pipe_woke {
                drain_pipe(pipe_rd);
                for pid in drain_pending(shared) {
                    if watched.iter().any(|(p, _)| *p == pid) {
                        continue; // idempotent: already watched
                    }
                    match pidfd_open_for(pid) {
                        PidfdOpen::Opened(pidfd) => watched.push((pid, pidfd)),
                        PidfdOpen::AlreadyDead => {
                            if exit_tx.send(pid).is_err() {
                                return; // receiver gone — the watcher task ended
                            }
                        }
                        PidfdOpen::Unsupported => {
                            tracing::debug!(
                                "pidfd_open ENOSYS (pre-5.3 kernel); instant exit off (backstops cover)"
                            );
                            return;
                        }
                        PidfdOpen::Failed => {}
                    }
                }
            }
            for pid in &exited {
                // Dropping the OwnedFd closes the pidfd; the pid leaves the
                // set so it can be re-watched if recycled for a new session.
                watched.retain(|(p, _)| p != pid);
                if exit_tx.send(*pid).is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    use tokio::sync::mpsc::UnboundedSender;

    use super::Shared;

    /// No validated exit-watch primitive on this platform (Windows would
    /// need a RegisterWaitForSingleObject handle design we haven't vetted).
    /// `init` returning `None` makes `ExitWatch::spawn` return `None`, so
    /// the watcher keeps the negative-vouch + TTL backstops only — instant
    /// exit is an additive fast path, never a dependency.
    pub(super) struct Backend;

    impl Backend {
        pub(super) fn init() -> Option<Self> {
            None
        }

        pub(super) fn wake(&self) {}
    }

    pub(super) fn run(_shared: &Shared, _exit_tx: &UnboundedSender<i32>) {}
}

#[cfg(test)]
mod tests {
    use super::ExitWatch;

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn spawn_is_none_on_unsupported_platforms() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        assert!(ExitWatch::spawn(tx).is_none());
    }

    /// Child-process tests, mirroring `fd_probe`'s twin style: the Linux arm
    /// pins the shared logic in CI (ubuntu), the macOS arm locally.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    mod live {
        use std::process::{Child, Command};
        use std::time::Duration;

        use super::ExitWatch;

        /// Liveness ceiling for the real-process `live::` tests. Generous
        /// enough to absorb `cargo llvm-cov` instrumentation + full-parallel
        /// scheduling jitter on CI — the coverage job intermittently flaked
        /// `drop_stops_cleanly` at 3s, where the watcher thread's post-`Drop`
        /// kqueue/pidfd wake just hadn't been scheduled yet. It is a liveness
        /// bound, NOT a latency assertion: a true hang is still failed by
        /// nextest's 180s slow-timeout, so a wider deadline doesn't weaken it.
        const LIVE_PID_DEADLINE: Duration = Duration::from_secs(10);

        fn sleeper() -> Child {
            Command::new("sleep").arg("30").spawn().unwrap()
        }

        fn kill_and_reap(child: &mut Child) {
            let _ = child.kill();
            let _ = child.wait();
        }

        /// Drain `rx` until `pid` arrives (true) or the deadline passes
        /// (false). Unrelated pids (a bogus-pid ESRCH synthesis) are skipped.
        async fn wait_for_pid(
            rx: &mut tokio::sync::mpsc::UnboundedReceiver<i32>,
            pid: i32,
            within: Duration,
        ) -> bool {
            let deadline = tokio::time::Instant::now() + within;
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                    Ok(Some(p)) if p == pid => return true,
                    Ok(Some(_)) => {}
                    Ok(None) => return false,
                    Err(_) => {}
                }
            }
            false
        }

        /// Panic if another exit for `pid` arrives within `window`.
        async fn assert_no_pid_within(
            rx: &mut tokio::sync::mpsc::UnboundedReceiver<i32>,
            pid: i32,
            window: Duration,
            why: &str,
        ) {
            let deadline = tokio::time::Instant::now() + window;
            while tokio::time::Instant::now() < deadline {
                if let Ok(Some(p)) =
                    tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
                {
                    assert_ne!(p, pid, "{why}");
                }
            }
        }

        #[tokio::test]
        async fn exit_watch_reports_a_killed_child() {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let watch = ExitWatch::spawn(tx).expect("backend must init on macOS/Linux");
            let mut child = sleeper();
            let pid = child.id() as i32;
            watch.watch(pid);
            kill_and_reap(&mut child);
            assert!(
                wait_for_pid(&mut rx, pid, LIVE_PID_DEADLINE).await,
                "a watched process dying must surface its pid on the exit channel"
            );
        }

        /// The registration race (a pid dead BEFORE watch() is processed)
        /// must synthesize the exit immediately — macOS via the EV_RECEIPT
        /// receipt's ESRCH, Linux via pidfd_open's ESRCH. (A pid recycle
        /// between reap and watch would make this flake; the window is
        /// microseconds and both kernels allocate pids sequentially.)
        #[tokio::test]
        async fn exit_watch_already_dead_pid_fires_immediately() {
            let mut child = sleeper();
            let pid = child.id() as i32;
            kill_and_reap(&mut child);

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let watch = ExitWatch::spawn(tx).expect("backend must init on macOS/Linux");
            watch.watch(pid);
            assert!(
                wait_for_pid(&mut rx, pid, LIVE_PID_DEADLINE).await,
                "watching an already-dead pid must synthesize its exit (ESRCH path)"
            );
        }

        #[tokio::test]
        async fn watch_is_idempotent() {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let watch = ExitWatch::spawn(tx).expect("backend must init on macOS/Linux");
            let mut child = sleeper();
            let pid = child.id() as i32;
            watch.watch(pid);
            watch.watch(pid);
            kill_and_reap(&mut child);
            assert!(
                wait_for_pid(&mut rx, pid, LIVE_PID_DEADLINE).await,
                "the first exit must arrive"
            );
            assert_no_pid_within(
                &mut rx,
                pid,
                Duration::from_millis(500),
                "double-watching a live pid must yield exactly ONE exit",
            )
            .await;
        }

        /// A garbage pid must not panic or kill the thread — and may either
        /// synthesize an exit (ESRCH) or stay silent; the load-bearing
        /// assertion is ISOLATION: a real watch registered after it still
        /// fires.
        #[tokio::test]
        async fn watch_bogus_pid_does_not_panic() {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let watch = ExitWatch::spawn(tx).expect("backend must init on macOS/Linux");
            watch.watch(999_999_999);
            let mut child = sleeper();
            let pid = child.id() as i32;
            watch.watch(pid);
            kill_and_reap(&mut child);
            assert!(
                wait_for_pid(&mut rx, pid, LIVE_PID_DEADLINE).await,
                "a bogus registration must not break later real watches"
            );
        }

        /// A DEAD watcher thread (backend error, pidfd ENOSYS, receiver gone)
        /// must mark the handle closed so `watch()` rejects instead of
        /// queueing onto `pending` forever — long-lived producers
        /// (`HookPidWatch::note`, `PresenceExitWatch::watch`) push per hook
        /// event for the process lifetime, and with no drainer the Vec grows
        /// unboundedly. Thread death is forced here via the receiver-gone
        /// exit path (the only one triggerable portably).
        #[tokio::test]
        async fn watch_after_thread_death_stops_queueing() {
            use std::sync::atomic::Ordering;
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let watch = ExitWatch::spawn(tx).expect("backend must init on macOS/Linux");
            drop(rx);
            // An already-dead pid synthesizes an exit whose send fails
            // (receiver dropped) → run() returns → the death hook must fire.
            let mut child = sleeper();
            let pid = child.id() as i32;
            kill_and_reap(&mut child);
            watch.watch(pid);
            let deadline = tokio::time::Instant::now() + LIVE_PID_DEADLINE;
            while !watch.shared.closed.load(Ordering::SeqCst)
                && tokio::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(
                watch.shared.closed.load(Ordering::SeqCst),
                "a dead watcher thread must mark the handle closed"
            );
            watch.watch(4242);
            watch.watch(4243);
            assert!(
                super::super::lock_pending(&watch.shared).is_empty(),
                "watch() after thread death must not grow `pending`"
            );
        }

        /// Drop sets `closed` + wakes: the thread must exit, observable as
        /// the channel closing (the thread owns the only sender). The kill
        /// also exercises the post-Drop machinery for no-panic.
        ///
        /// Looped to STRESS the `watch()`-then-`drop()` wake race: on Linux a
        /// Drop landing between the watcher thread's post-poll `closed` check
        /// and its unconditional `drain_pipe` could have its wake byte EATEN by
        /// that drain, stranding the thread in `poll()` on the still-live child
        /// until LIVE_PID_DEADLINE (the self-pipe lost-wakeup the top-of-loop
        /// `closed` re-check fixes). One unfixed iteration hangs the whole test;
        /// fixed, every iteration closes in ms.
        #[tokio::test]
        async fn drop_stops_cleanly() {
            for _ in 0..50 {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let watch = ExitWatch::spawn(tx).expect("backend must init on macOS/Linux");
                let mut child = sleeper();
                watch.watch(child.id() as i32);
                drop(watch);
                let closed = tokio::time::timeout(LIVE_PID_DEADLINE, async {
                    while rx.recv().await.is_some() {}
                })
                .await;
                // Reap BEFORE asserting: were the drop slow enough for the assert
                // to fire, a trailing reap would be skipped and the sleeper would
                // leak (the nextest LEAK that paired with this test's flake).
                kill_and_reap(&mut child);
                assert!(
                    closed.is_ok(),
                    "dropping the ExitWatch must stop the thread (its sender must drop)"
                );
            }
        }
    }
}
