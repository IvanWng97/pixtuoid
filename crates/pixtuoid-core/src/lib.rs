//! pixtuoid-core: headless logic for the pixtuoid TUI.

pub mod id;
pub mod layout;
pub mod physics;
mod platform;
pub mod pose;
pub mod render;
pub mod source;
pub mod sprite;
pub mod state;
pub mod walkable;

pub use id::AgentId;
pub use render::Renderer;
pub use source::{
    Activity, AgentEvent, Source, TaggedReceiver, TaggedSender, ToolDetail, Transport,
};
pub use sprite::{Frame, Palette, Pixel, Rgb, RgbBuffer, Sprite};
pub use state::reducer::Reducer;
pub use state::{ActivityState, AgentSlot, SceneState};
pub use walkable::{OccupancyOverlay, WalkableMask};
