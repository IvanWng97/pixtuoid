//! What a painted frame answers a pointer with: one [`Hover`] per hit box.

use pixtuoid_core::id::AgentId;
use pixtuoid_core::source::daemon::DaemonInstanceKey;

use crate::layout::Bounds;

/// A hit box in the frame and who answers for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hover {
    /// In logical pixels, whatever the render scale.
    pub(crate) at: Bounds,
    pub(crate) target: HoverTarget,
}

/// Who a [`Hover`] names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HoverTarget {
    Agent(AgentId),
    Pet,
    Mascot(DaemonInstanceKey),
}
