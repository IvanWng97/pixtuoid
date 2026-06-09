//! The agent dashboard: a modal popup overviewing every agent across every
//! floor as a foldable parent→subagent tree. This module is the PURE model —
//! no ratatui. It turns a `SceneState` into a flat, navigable row list and
//! owns the fold + selection logic. The ratatui painter lives in
//! `tui::widgets::dashboard`; the event-loop wiring lives in `tui::mod`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pixtuoid_core::state::{ActivityState, AgentSlot, SceneState};
use pixtuoid_core::AgentId;

/// Roots with more than this many direct subagents render collapsed by
/// default, so a large CC workflow (≈20 subagents) doesn't flood the board.
pub const AUTO_COLLAPSE_THRESHOLD: usize = 5;

/// Inner visible-row count of the dashboard popup. Shared by `clamp_scroll`
/// (event loop) and the painter so the scroll math and the painted window
/// can't disagree.
pub const DASHBOARD_VIEWPORT_ROWS: usize = 16;

/// The activity shown on a row, distilled from `ActivityState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    /// Active, carrying the live tool detail when the slot had one.
    Active(Option<Arc<str>>),
    /// Waiting on a permission/decision, carrying the reason.
    Waiting(Arc<str>),
    Idle,
}

/// One visible row in the dashboard list.
#[derive(Debug, Clone)]
pub struct DashboardRow {
    pub agent_id: AgentId,
    /// `None` for a root (main) agent; the parent's id for a subagent. Carried
    /// so the event loop can collapse a child's parent without re-querying.
    pub parent_id: Option<AgentId>,
    /// 0 = root, 1 = subagent (the tree is two levels in practice).
    pub depth: u8,
    pub label: Arc<str>,
    pub source: Arc<str>,
    pub floor_idx: usize,
    pub state: RowState,
    /// Direct-subagent count (0 for non-roots). Drives the `(N)` fold badge.
    pub child_count: usize,
    /// True when this is a collapsed root (its subtree is hidden below it).
    pub collapsed: bool,
}

/// Per-session fold state for root agents. Persists across open/close while
/// the app runs. Grown in later steps with the auto-collapse + toggle logic.
#[derive(Debug, Default)]
pub struct DashboardFolds {
    collapsed: HashSet<AgentId>,
    user_toggled: HashSet<AgentId>,
}

impl DashboardFolds {
    /// Whether root `root_id` (with `child_count` direct subagents) renders
    /// collapsed. A root the user has explicitly toggled honors that choice;
    /// otherwise it auto-collapses once it exceeds `AUTO_COLLAPSE_THRESHOLD`,
    /// so a workflow that balloons past the threshold mid-session folds itself.
    fn is_collapsed(&self, root_id: AgentId, child_count: usize) -> bool {
        if self.user_toggled.contains(&root_id) {
            self.collapsed.contains(&root_id)
        } else {
            child_count > AUTO_COLLAPSE_THRESHOLD
        }
    }

    /// Collapse every given root and pin it (a deliberate bulk op; roots that
    /// appear later still auto-evaluate, since they aren't pinned).
    pub fn fold_all(&mut self, roots: impl IntoIterator<Item = AgentId>) {
        for root in roots {
            self.user_toggled.insert(root);
            self.collapsed.insert(root);
        }
    }

    /// Expand every given root and pin it.
    pub fn unfold_all(&mut self, roots: impl IntoIterator<Item = AgentId>) {
        for root in roots {
            self.user_toggled.insert(root);
            self.collapsed.remove(&root);
        }
    }
}

/// Flatten the scene into a tree-ordered row list: roots sorted by
/// `desk_index`, each immediately followed by its visible subagents. A root
/// with no parent — OR whose `parent_id` points at an agent absent from the
/// scene (an orphan) — anchors a subtree; collapsing a root hides its whole
/// subtree. Deeper nesting than two levels is walked too, so nothing is ever
/// silently dropped.
pub fn build_dashboard_rows(scene: &SceneState, folds: &DashboardFolds) -> Vec<DashboardRow> {
    // parent_id -> direct children, only for parents present in the scene.
    let mut children: HashMap<AgentId, Vec<AgentId>> = HashMap::new();
    for (id, slot) in &scene.agents {
        if let Some(parent) = slot.parent_id {
            if scene.agents.contains_key(&parent) {
                children.entry(parent).or_default().push(*id);
            }
        }
    }

    // Roots: no parent, or a parent that isn't in the scene (orphan-as-root).
    let mut roots: Vec<AgentId> = scene
        .agents
        .iter()
        .filter(|(_, s)| s.parent_id.map_or(true, |p| !scene.agents.contains_key(&p)))
        .map(|(id, _)| *id)
        .collect();
    roots.sort_by_key(|id| scene.agents[id].desk_index);

    let mut rows = Vec::new();
    for root in roots {
        push_subtree(scene, &children, folds, root, 0, None, &mut rows);
    }
    rows
}

/// Depth-first emit `node` then (unless collapsed) its children, in
/// `desk_index` order. Only roots (`depth == 0`) are collapsible in v1.
fn push_subtree(
    scene: &SceneState,
    children: &HashMap<AgentId, Vec<AgentId>>,
    folds: &DashboardFolds,
    node: AgentId,
    depth: u8,
    parent_id: Option<AgentId>,
    rows: &mut Vec<DashboardRow>,
) {
    let empty: Vec<AgentId> = Vec::new();
    let kids = children.get(&node).unwrap_or(&empty);
    let child_count = kids.len();
    let collapsed = depth == 0 && folds.is_collapsed(node, child_count);

    rows.push(row_for(
        &scene.agents[&node],
        parent_id,
        depth,
        child_count,
        collapsed,
    ));
    if collapsed {
        return;
    }

    let mut kids = kids.clone();
    kids.sort_by_key(|id| scene.agents[id].desk_index);
    for kid in kids {
        push_subtree(scene, children, folds, kid, depth + 1, Some(node), rows);
    }
}

fn row_for(
    slot: &AgentSlot,
    parent_id: Option<AgentId>,
    depth: u8,
    child_count: usize,
    collapsed: bool,
) -> DashboardRow {
    DashboardRow {
        agent_id: slot.agent_id,
        parent_id,
        depth,
        label: slot.label.clone(),
        source: slot.source.clone(),
        floor_idx: slot.floor_idx,
        state: row_state(&slot.state),
        child_count,
        collapsed,
    }
}

fn row_state(state: &ActivityState) -> RowState {
    match state {
        ActivityState::Active { detail, .. } => RowState::Active(detail.clone()),
        ActivityState::Waiting { reason } => RowState::Waiting(reason.clone()),
        ActivityState::Idle => RowState::Idle,
    }
}

/// Move the selection one visible row up (`dir = -1`) or down (`dir = +1`),
/// clamped at the ends. With nothing selected — or a selection that has since
/// vanished/hidden — it re-anchors to the first row. `None` only when empty.
pub fn move_selection(
    rows: &[DashboardRow],
    current: Option<AgentId>,
    dir: i32,
) -> Option<AgentId> {
    if rows.is_empty() {
        return None;
    }
    let new_idx = match current.and_then(|c| rows.iter().position(|r| r.agent_id == c)) {
        Some(i) => (i as i32 + dir).clamp(0, rows.len() as i32 - 1) as usize,
        None => 0, // nothing selected, or it vanished — re-anchor to the first row
    };
    Some(rows[new_idx].agent_id)
}

/// Each-frame re-anchor: keep `current` if it's still a visible row, else fall
/// back to the first row (the selected agent exited or was hidden by a fold).
pub fn reanchor_selection(rows: &[DashboardRow], current: Option<AgentId>) -> Option<AgentId> {
    match current {
        Some(c) if rows.iter().any(|r| r.agent_id == c) => Some(c),
        _ => rows.first().map(|r| r.agent_id),
    }
}

/// The floor the selected agent sits on, for the jump. `None` if not present.
pub fn resolve_floor(rows: &[DashboardRow], selected: AgentId) -> Option<usize> {
    rows.iter()
        .find(|r| r.agent_id == selected)
        .map(|r| r.floor_idx)
}

/// Adjust `scroll` so the selected row stays within a `visible_height`-row
/// viewport: scroll up if it sits above the window, down if below, else leave it.
pub fn clamp_scroll(
    rows: &[DashboardRow],
    selected: Option<AgentId>,
    scroll: usize,
    visible_height: usize,
) -> usize {
    let Some(sel) = selected else {
        return 0;
    };
    let Some(idx) = rows.iter().position(|r| r.agent_id == sel) else {
        return scroll;
    };
    if idx < scroll {
        idx
    } else if visible_height > 0 && idx >= scroll + visible_height {
        idx + 1 - visible_height
    } else {
        scroll
    }
}

/// Session-persistent dashboard UI state, owned by the event loop. Only `open`
/// flips on close, so folds + selection survive close/reopen for the session.
#[derive(Debug, Default)]
pub struct DashboardUi {
    pub open: bool,
    pub selected: Option<AgentId>,
    pub scroll: usize,
    pub folds: DashboardFolds,
}

#[cfg(test)]
mod tests;
