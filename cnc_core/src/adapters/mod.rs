pub mod debug;
pub mod emulator;
pub mod fluidnc_websocket;
#[path = "grbl_async.rs"]
pub mod grbl;

pub type FluidNcRs232Adapter = grbl::GrblAdapter;
