/// Once-per-transition gate for the watcher's persistent-failure warnings
/// (the root going unreadable mid-run, a broken notify backend). Both
/// failures recur on every pass — the poll backstop re-scans forever — so an
/// ungated `warn!` would spam the warn-floor file log every interval, while
/// total silence leaves the watcher permanently blind with no breadcrumb
/// (the silently-empty-office class #157/#224 exist to surface; after a
/// successful bind there is no SourceDeath path left to report through).
/// `on_failure` is true exactly on the first failure after a success;
/// `on_success` is true exactly on recovery.
#[derive(Default)]
pub(super) struct FailureLatch {
    failing: bool,
}

impl FailureLatch {
    pub(super) fn on_failure(&mut self) -> bool {
        !std::mem::replace(&mut self.failing, true)
    }

    pub(super) fn on_success(&mut self) -> bool {
        std::mem::replace(&mut self.failing, false)
    }
}
