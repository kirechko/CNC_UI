# Rosetta

The Rosetta system defined by the three specifications in [`plans/Rosetta.md`](../plans/Rosetta.md):

| Subsystem | Module | Spec |
|---|---|---|
| **Core — Rosetta IR** (Macro IR, Toolpath IR, capability matrix + resolution registry, frontends/emitters) | [`src/ir/`](src/ir), [`src/io/`](src/io) | ТЗ №1 |
| **Rosetta Macro Engine** (generative/reactive macros, triggers, host interpreter, safety gates) | [`src/macro_engine/`](src/macro_engine) | ТЗ №2 |
| **Rosetta Transform** (affine/geometric/structural program rewrites, pipeline, artifacts) | [`src/transform/`](src/transform) | ТЗ №3 |

Every public type documents the contract IDs it implements (`CORE-M-01`,
`MAC-C-07`, `TRF-M-06`, …), so the traceability matrix can be verified
against the code. `CONTRACT_VERSION` (see [`src/lib.rs`](src/lib.rs)) tracks
breaking changes to the IR and trait surfaces.

## Design notes

* **Synchronous core.** The ТЗ sketches `async` traits; this crate keeps no
  runtime dependency — machine I/O is injected through
  `macro_engine::runtime::HostRuntime` and cross-subsystem calls use plain
  hooks (`TransformHook`, `transform::model::MacroExpander`). An application
  layer wraps these in `spawn_blocking`/async without touching IR types.
* **Dependencies: `serde` + `serde_yaml_ng` only** — the crate is portable
  to any host (embedded tooling, desktop app, web backend).
* **Serde shapes.** `Stmt`/`Expr` are externally tagged: JSON uses the map
  form `{"gcode": {"template": …}}`, `serde_yaml_ng` uses the YAML tag form
  `!gcode {template: …}` (both are the same IR; see the note in
  [`src/ir/macro_ir.rs`](src/ir/macro_ir.rs)). Triggers use the spec's
  `{type: mcode, code: 6}` internal-tag form.
* **Deferred roadmap items fail loudly** with `RosettaError::Unsupported`
  naming the phase (O-words frontend = "2-3", Klipper/Jinja2 emitters = "2",
  CutterCompBake/LeadInOut/CornerRound = "D", `BetweenMarks` regions = "2").

## Building and testing

```sh
cargo test -p rosetta          # unit + integration (tests/e2e.rs)
```

## Detaching into a standalone project

The directory is self-contained on purpose:

1. Copy `rosetta/` (Cargo.toml + src + tests) out of the workspace.
2. Remove the `"rosetta"` entry from the root `Cargo.toml` `[workspace] members`.
3. `cd rosetta && cargo build` — there are no path dependencies; the crate
   depends only on `serde` and `serde_yaml_ng`.

Storage locations assumed by the specs (wired by the host application, not
by this crate): macro sources in `config/macros/*.yaml`, artifacts emitted
next to the original program.
