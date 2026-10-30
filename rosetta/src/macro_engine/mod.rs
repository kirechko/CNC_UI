//! Rosetta Macro Engine (ТЗ №2) — the execution side of the macro system.
//!
//! Submodules:
//!
//! * [`runtime`] — the [`runtime::HostRuntime`] seam through which the engine
//!   touches the machine (commands, status, sleeps), plus cancellation,
//!   logging events and the dry-run/offline runtimes (ТЗ №2 §8–9).
//! * [`interpreter`] — the synchronous host interpreter (`MAC` engine core:
//!   state machine `Idle → Running → Done/Aborted`, step budget, variable
//!   scopes, G-code template interpolation).
//! * [`generative`] — offline expansion of generative macros into flat
//!   G-code lines (consumed by `io::FlatEmitter`, contract `CORE-M-04`).
//! * [`registry`] — the [`registry::MacroRegistry`] the core requires from
//!   the engine (`CORE-M-06` / `MAC-C-06`) plus link validation (`CORE-M-05`).
//! * [`triggers`] — launch trigger lookup helpers (ТЗ №2 §4).
//! * [`safety`] — pre-start checks and the capability gate guaranteeing
//!   `MAC-C-07` (never run what failed `CORE-M-03`).
//!
//! # Synchronous design
//!
//! The ТЗ sketch uses `async fn invoke(...)`. The engine is deliberately
//! synchronous with injected `FnMut` hooks so the crate carries no async
//! runtime; an application layer wraps these calls in `spawn_blocking` or an
//! async shim without touching any type in this module (see crate docs).

pub mod generative;
pub mod interpreter;
pub mod registry;
pub mod runtime;
pub mod safety;
pub mod triggers;

use crate::error::RosettaResult;
use crate::ir::macro_ir::{MacroId, Value};
use crate::macro_engine::runtime::{CancellationToken, HostRuntime, MacroLogLine, RunOutcome};

pub use registry::{MacroRef, MacroRegistry};
pub use runtime::{DryRunRuntime, EmitRuntime, MachineState, HostRuntime as HostRuntimeTrait};
pub use safety::{SafetyReport, prestart as prestart_check};
pub use triggers::{TriggerEvent, find_trigger};

/// Cross-subsystem entry point the core and the transform engine call to run
/// a registered macro by id (`MAC-T-02` / `TRF-M-02`).
///
/// Implementations MUST enforce the safety pipeline: pre-start machine check,
/// capability validation (`CORE-M-03`/`MAC-C-03`), then interpretation.
pub trait MacroInvoker {
    /// Invoke macro `id` with positional `args`, driving the machine through
    /// `rt`, honoring `cancel`, and streaming log events through `log`.
    /// Returns the engine-level outcome; transport/controller failures are
    /// `Err`, semantic aborts (`Stmt::Abort`) are `Ok(RunOutcome::Aborted)`.
    fn invoke(
        &mut self,
        id: &MacroId,
        args: Vec<Value>,
        rt: &mut dyn HostRuntime,
        cancel: &CancellationToken,
        log: &mut dyn FnMut(MacroLogLine),
    ) -> RosettaResult<RunOutcome>;
}

/// The default [`MacroInvoker`]: binds a [`MacroRegistry`], the capability
/// matrix and the target backend, with an optional hook for
/// `Stmt::InvokeTransform` (`MAC-T-03`/`MAC-T-05`: synchronous, returns
/// status, respects the same cancellation as the macro — `MAC-T-06`).
pub struct Invoker<'a> {
    /// Source of registered macro programs (`CORE-M-06`).
    pub registry: &'a MacroRegistry,
    /// Matrix consulted before execution (`CORE-M-03`).
    pub matrix: &'a crate::ir::capability::CapabilityMatrix,
    /// Backend the macro runs against.
    pub backend: crate::ir::capability::BackendType,
    /// Host-provided transform bridge: `(transform id, args) -> result`.
    /// `None` disables `Stmt::InvokeTransform` for this invoker.
    pub transform:
        Option<&'a mut dyn FnMut(&str, Vec<Value>) -> RosettaResult<Option<Value>>>,
}

impl<'a> Invoker<'a> {
    /// Construct an invoker over `registry`, validating against `matrix` for
    /// `backend`; `transform` wires the ТЗ №3 bridge (may be `None`).
    pub fn new(
        registry: &'a MacroRegistry,
        matrix: &'a crate::ir::capability::CapabilityMatrix,
        backend: crate::ir::capability::BackendType,
        transform: Option<&'a mut dyn FnMut(&str, Vec<Value>) -> RosettaResult<Option<Value>>>,
    ) -> Self {
        Invoker { registry, matrix, backend, transform }
    }
}

impl MacroInvoker for Invoker<'_> {
    fn invoke(
        &mut self,
        id: &MacroId,
        args: Vec<Value>,
        rt: &mut dyn HostRuntime,
        cancel: &CancellationToken,
        log: &mut dyn FnMut(MacroLogLine),
    ) -> RosettaResult<RunOutcome> {
        // Copy the shared registry handle out of `&mut self` so the program
        // reference keeps the invoker's own lifetime, independent of `rt`.
        let registry: &MacroRegistry = self.registry;
        let program = registry
            .get(id)
            .ok_or_else(|| crate::error::RosettaError::NotFound {
                kind: "macro".into(),
                id: id.clone(),
            })?;
        safety::execute(
            program,
            args,
            rt,
            cancel,
            log,
            &mut self.transform,
            self.matrix,
            self.backend,
        )
    }
}
