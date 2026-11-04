//! Realtime control endpoints, grouped by spindle, jog, and position behavior.
//!
//! This facade keeps the handler names available to the router and contract
//! tests while each cohesive domain owns its request types and implementations.

mod jog;
mod position;
mod spindle;

pub(crate) use jog::*;
pub(crate) use position::*;
pub(crate) use spindle::*;
