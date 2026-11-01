//! Rosetta-backed operations on `AppState`: macro runs with live logs,
//! capability validation and the transform preview/apply flow (T26 plan §3.4,
//! §3.5). All entry points are synchronous; the axum handlers call them on
//! the blocking path (they only touch mutexes and channels, no awaits).

use rosetta::ir::capability::Incompatibility;
use rosetta::ir::macro_ir::Value;
use rosetta::transform::model::{Artifact, ProgramRef, TransformPlan};
use serde::Serialize;

use super::super::models::LoadedProgram;
use super::AppState;
use crate::rosetta_host::backend::backend_from_device_id;
use crate::rosetta_host::registry::{build_registry, matrix_with_presets};
use crate::rosetta_host::run::{self, RunStatus};
use crate::rosetta_host::transform::TransformBridge;
use crate::web_server::state::position::temp_program_path;
use rosetta::macro_engine::registry::MacroRegistry;

/// One entry of the capability-validation response (wire contract).
#[derive(Serialize)]
pub(crate) struct ValidationFinding {
    /// 1-based source line of the offending code word.
    pub line: usize,
    /// Rendered code (`"G83"`), from `rosetta::ir::toolpath::Code`'s Display.
    pub code: String,
    /// `info` / `warning` / `blocker`.
    pub severity: &'static str,
    /// Whether a registered automatic resolution exists.
    pub resolvable: bool,
    /// Human-readable reason.
    pub detail: String,
}

/// Summary returned by `POST /api/program/validate`.
#[derive(Serialize)]
pub(crate) struct ValidationReport {
    /// Backend label the scan ran against.
    pub backend: String,
    /// True when every frame is executable as-is.
    pub clean: bool,
    pub findings: Vec<ValidationFinding>,
}

/// Summary returned by `POST /api/program/transform/preview`.
#[derive(Serialize)]
pub(crate) struct PreviewReport {
    /// Backend the plan targets.
    pub backend: String,
    /// Plan steps the auto-fixer proposed (serialized rosetta plan).
    pub plan: TransformPlan,
    /// Human prompts for findings that need operator parameters.
    pub unresolved: Vec<String>,
    /// The prospective artifact: original vs emitted text plus traces.
    pub artifact: Artifact,
}

/// Converts positional macro arguments from the JSON wire format to rosetta
/// `Value`s. Non-scalar JSON (objects/arrays) becomes its string form — macro
/// bodies consume numbers and strings only.
pub(crate) fn json_args_to_values(args: Vec<serde_json::Value>) -> Vec<Value> {
    args.into_iter()
        .map(|arg| match arg {
            serde_json::Value::Number(number) => Value::Num(number.as_f64().unwrap_or_default()),
            serde_json::Value::String(text) => Value::Str(text),
            serde_json::Value::Bool(flag) => Value::Bool(flag),
            serde_json::Value::Null => Value::Null,
            other => Value::Str(other.to_string()),
        })
        .collect()
}

impl AppState {
    /// Backend of the currently connected machine (Emulator default while
    /// nothing else is attached).
    pub(crate) fn rosetta_backend(&self) -> rosetta::ir::capability::BackendType {
        let device_id = {
            let controller = self.controller.lock();
            match controller {
                Ok(controller) => controller.connected_device_id(),
                Err(_) => None,
            }
        };
        backend_from_device_id(device_id.as_deref())
    }

    /// Builds the macro registry from the current `user_macros` rows.
    fn macro_registry(&self) -> Result<MacroRegistry, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "macro database lock poisoned".to_string())?;
        build_registry(&connection)
    }

    /// Transform bridge whose matrix carries the operator-editable
    /// `rosetta_settings` substitution presets on top of the built-ins, so
    /// validate/preview/apply all resolve cycles through the database.
    fn transform_bridge(&self) -> Result<TransformBridge, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "settings database lock poisoned".to_string())?;
        Ok(TransformBridge::with_matrix(matrix_with_presets(
            &connection,
        )))
    }

    /// Starts one macro run (dry or live) and returns its run id.
    pub(crate) fn run_macro(
        &self,
        macro_id: &str,
        dry: bool,
        args: Vec<Value>,
    ) -> Result<String, String> {
        let registry = self.macro_registry()?;
        let matrix = {
            let connection = self
                .log
                .lock()
                .map_err(|_| "settings database lock poisoned".to_string())?;
            matrix_with_presets(&connection)
        };
        let backend = self.rosetta_backend();
        // The controller may be swapped while a macro runs; the spawned thread
        // keeps using the handle of the worker pair that accepted the run.
        let handle = {
            let controller = self
                .controller
                .lock()
                .map_err(|_| "controller lock poisoned".to_string())?;
            controller.runtime_handle()
        };
        run::start_run(
            registry,
            &self.macro_runs,
            backend,
            handle,
            macro_id,
            dry,
            args,
            matrix,
        )
    }

    /// Tails one macro run (log lines from `from`, lifecycle, result).
    pub(crate) fn macro_run_status(&self, run_id: &str, from: usize) -> Option<RunStatus> {
        self.macro_runs.status(run_id, from)
    }

    /// Requests cancellation of a running macro.
    pub(crate) fn cancel_macro_run(&self, run_id: &str) -> bool {
        self.macro_runs.cancel(run_id)
    }

    /// Pure capability scan of a program text (§3.4).
    pub(crate) fn validate_program(&self, source: &str) -> Result<ValidationReport, String> {
        let backend = self.rosetta_backend();
        let bridge = self.transform_bridge()?;
        let findings = bridge
            .validate(source, backend)
            .map_err(|error| error.to_string())?;
        let mapped: Vec<ValidationFinding> = findings
            .iter()
            .map(|finding| ValidationFinding {
                line: finding.line,
                code: finding.code.to_string(),
                severity: severity_label(finding),
                resolvable: finding.resolution.is_some(),
                detail: finding.detail.clone(),
            })
            .collect();
        let clean = mapped.iter().all(|finding| finding.severity == "info");
        Ok(ValidationReport {
            backend: backend.label().to_owned(),
            clean,
            findings: mapped,
        })
    }

    /// Detect→Plan→local-Apply: the «was / became» diff for operator review
    /// (§3.3). The machine is not touched; nothing is persisted.
    pub(crate) fn preview_transform(&self, source: &str) -> Result<PreviewReport, String> {
        let backend = self.rosetta_backend();
        let bridge = self.transform_bridge()?;
        let registry = self.macro_registry()?;
        let expander = crate::rosetta_host::transform::RegistryExpander {
            registry: &registry,
            matrix: &bridge.matrix,
            backend,
        };
        let program_ref = ProgramRef {
            id: "buffer".into(),
            label: self.program_name(),
        };
        let (plan, unresolved, artifact) = bridge
            .preview(source, backend, &program_ref, Some(&expander))
            .map_err(|error| error.to_string())?;
        Ok(PreviewReport {
            backend: backend.label().to_owned(),
            plan,
            unresolved,
            artifact,
        })
    }

    /// The gated Apply of a previously previewed plan: runs the pipeline with
    /// the operator's approval, writes the emitted program over the temp
    /// file, reloads it into the controller and syncs the UI buffer.
    pub(crate) fn apply_transform(
        &self,
        plan: &TransformPlan,
        approved: bool,
    ) -> Result<Artifact, String> {
        let source = {
            let program = self
                .program
                .lock()
                .map_err(|_| "program lock poisoned".to_string())?;
            match &*program {
                Some(loaded) => loaded.content.clone(),
                None => return Err("no program is loaded to transform".into()),
            }
        };
        let bridge = self.transform_bridge()?;
        let registry = self.macro_registry()?;
        let backend = self.rosetta_backend();
        let expander = crate::rosetta_host::transform::RegistryExpander {
            registry: &registry,
            matrix: &bridge.matrix,
            backend,
        };
        let artifact = bridge
            .apply_plan(&source, plan, approved, Some(&expander))
            .map_err(|error| error.to_string())?;

        // Persist + load: the transformed text replaces the buffer program;
        // the temp file path keeps the controller-side load semantics.
        let path = temp_program_path();
        std::fs::write(&path, &artifact.emitted_text)
            .map_err(|error| format!("cannot write transformed program: {error}"))?;
        {
            let controller = self
                .controller
                .lock()
                .map_err(|_| "controller lock poisoned".to_string())?;
            controller
                .load_program(&path)
                .map_err(|error| format!("{}: {}", error.code, error.message))?;
        }
        let name = self.program_name().unwrap_or_else(|| "program".to_owned());
        *self
            .program
            .lock()
            .map_err(|_| "program lock poisoned".to_string())? = Some(LoadedProgram {
            name,
            content: artifact.emitted_text.clone(),
        });
        crate::message_log::record(
            "info",
            "rosetta transform",
            &format!(
                "applied plan \"{}\" ({} step(s))",
                plan.id,
                plan.steps.len()
            ),
        );
        Ok(artifact)
    }

    /// Text of the currently loaded program buffer, for validate/preview
    /// calls that omit an explicit source.
    pub(crate) fn loaded_program_text(&self) -> Option<String> {
        self.program
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|loaded| loaded.content.clone()))
    }

    /// File name of the loaded program buffer, if any.
    fn program_name(&self) -> Option<String> {
        self.program
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|loaded| loaded.name.clone()))
    }
}

/// UI severity token for one finding (mirrors the `rosetta` severity ladder).
fn severity_label(finding: &Incompatibility) -> &'static str {
    match finding.severity {
        rosetta::ir::capability::Severity::Info => "info",
        rosetta::ir::capability::Severity::Warning => "warning",
        rosetta::ir::capability::Severity::Blocker => "blocker",
    }
}
