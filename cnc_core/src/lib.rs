//! Defines the shared CNC API types, status models, discovery, and controller interface.
//!
//! The crate root is intentionally thin after the >500-line split:
//! - [`types`] - serializable domain types (axes, status, configuration, errors, events);
//! - [`api`]   - the [`CncApi`] trait every backend adapter implements plus the
//!   [`machine_state`] coarse-state classifier;
//! - [`discovery`], [`toolpath`], [`toolpath_parser`], [`adapters`] - unchanged modules.
//!
//! All items are re-exported at the crate root so existing paths
//! (`cnc_core::Axis`, `cnc_core::CncApi`, `cnc_core::machine_state`, ...) keep
//! working byte-for-byte for downstream crates.

pub mod adapters;
pub mod api;
pub mod discovery;
pub mod toolpath;
pub mod toolpath_parser;
mod types;

pub use api::{machine_state, CncApi};
pub use toolpath::{Toolpath, ToolpathProgress, ToolpathSegment};
pub use toolpath_parser::parse_toolpath;
pub use types::{
    Axis, AxisConfiguration, AxisUnits, CncEvent, CncResult, ControllerError, CoordinateSystem,
    CoordinateSystemOffset, InterpreterState, JogRequest, MachineAxis, MachineConfiguration,
    MachineState, MachineStatus, MotionKind, OverrideKind, Position, ProgramSource,
    SpindleCommand, SpindleConfiguration, SpindleDirection, SpindleStatus, TaskMode, TaskState,
};

#[cfg(test)]
mod tests;
