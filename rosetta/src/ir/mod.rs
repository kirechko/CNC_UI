//! Rosetta IR — the core intermediate representations (ТЗ №1, `CORE-M-01`
//! and `CORE-T-01`).
//!
//! The core owns exactly two IRs plus the cross-cutting registries:
//!
//! * [`macro_ir`] — Macro IR (`MacroProgram` / `Stmt` / `Expr`), the single
//!   source of truth for authoring procedures consumed by the Macro Engine
//!   (`MAC-C-01`).
//! * [`toolpath`] — Toolpath IR (`Toolpath` / `Segment` / `CodeFrame`), the
//!   canonical program representation consumed by Rosetta Transform
//!   (`TRF-C-01`).
//! * [`capability`] — the Capability Matrix *and* the single Resolution
//!   Registry (`CORE-M-03`, `CORE-T-03/04`, `CORE-V-02`). Neither the macro
//!   engine nor the transform engine keeps its own duplicate table.
//!
//! Every type derives `Serialize`/`Deserialize` because, per ТЗ №1 principle
//! 1, file formats (YAML, JSON) are only serializations of the IR — the IR
//! itself is always the truth.

pub mod capability;
pub mod macro_ir;
pub mod toolpath;

pub use capability::{
    BackendType, CapabilityMatrix, CapabilityRegistry, ConfirmPolicy, Feature, Incompatibility,
    Resolution, ResolutionRule, Severity, Support,
};
pub use macro_ir::{
    BinOp, Expr, MacroClass, MacroId, MacroMeta, MacroProgram, ParamDecl, Stmt, Subroutine,
    TransformId, Trigger, UnOp, Value, ValueKind, VarDecl,
};
pub use toolpath::{Code, CodeFrame, Point2, Point3, Segment, SegmentId, SegmentKind, Toolpath, Units};
