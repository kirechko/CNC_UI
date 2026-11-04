//! Construction and connection lifecycle facade for the FluidNC adapter.
//!
//! The cohesive implementation blocks live in child modules while this module
//! preserves the existing internal helper path used by adapter tests.

mod construction;
mod discovery;
mod lifecycle;
mod publication;

pub(super) use construction::cached_websocket_url;
