//! Machine-discovery endpoints: snapshots, live rescans, and runtime selection.
//! Scan coordination remains centralized in the scan module so all routes
//! share the same single-flight guard and registry updates.

mod routes;
mod scan;
mod selection;

pub(crate) use routes::{get_discovered_machines, rediscover_machines};
pub(crate) use scan::discover_and_connect_first;
pub(crate) use selection::select_machine;
