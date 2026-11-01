//! HTTP endpoints of the Rosetta integration (T26 plan §3.5).
//!
//! * `POST /api/macros/run` — start one macro run (`dry=1` selects the
//!   mandatory dry-run of ТЗ №2 §9.4); returns the run id immediately, the
//!   interpreter keeps running on its own thread.
//! * `GET  /api/macros/run/{run_id}` — run snapshot + log tail from `?from=`.
//! * `POST /api/macros/run/{run_id}/cancel` — cooperative cancellation.
//! * `POST /api/program/validate` — capability scan of a program text.
//! * `POST /api/program/transform/preview` — Detect→Plan plus the local
//!   was/became artifact for the confirmation dialog.
//! * `POST /api/program/transform/apply` — the gated run + program reload.
//!
//! Handlers stay thin: parse, delegate to `AppState` rosetta operations,
//! map `String` errors to 400/500 like the rest of this backend.

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use rosetta::ir::macro_ir::Value;
use rosetta::transform::model::{Artifact, TransformPlan};
use serde::{Deserialize, Serialize};

use crate::web_server::state::rosetta_ops::{PreviewReport, ValidationReport};
use crate::web_server::SharedController;

/// Body of `POST /api/macros/run`.
#[derive(Deserialize)]
pub(crate) struct MacroRunRequest {
    /// Registered macro id (the panel's primary key).
    pub(crate) id: String,
    /// Dry run: logic executes, motion commands are recorded only.
    #[serde(default)]
    pub(crate) dry: bool,
    /// Positional parameters passed to the macro IR.
    #[serde(default)]
    pub(crate) args: Vec<serde_json::Value>,
}

/// Response of `POST /api/macros/run`.
#[derive(Serialize)]
pub(crate) struct MacroRunResponse {
    pub(crate) run_id: String,
}

/// `?from=` offset tailing parameter of the run-status endpoint.
#[derive(Deserialize, Default)]
pub(crate) struct RunTailQuery {
    pub(crate) from: Option<usize>,
}

/// Body of `POST /api/program/validate` and `…/preview`: the program text;
/// when omitted, the currently loaded buffer program is used.
#[derive(Deserialize, Default)]
pub(crate) struct ProgramSourceRequest {
    #[serde(default)]
    pub(crate) content: Option<String>,
}

/// Body of `POST /api/program/transform/apply`.
#[derive(Deserialize)]
pub(crate) struct TransformApplyRequest {
    /// The plan exactly as returned by preview (round-trips through JSON).
    pub(crate) plan: TransformPlan,
    /// The operator's answer to the confirmation gate.
    pub(crate) approved: bool,
}

/// Maps a backend `String` error to the HTTP status pair used across this
/// API: everything these endpoints report is a client/state problem (400);
/// lock-poisoning wording never reaches users as a secret, but 500 is safer
/// for internal failures, so a prefix convention distinguishes them.
fn api_error(error: String) -> (StatusCode, String) {
    let status = if error.contains("lock poisoned") || error.starts_with("cannot ") {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, error)
}

/// `POST /api/macros/run` — see the module docs.
pub(crate) async fn run_macro(
    State(state): State<SharedController>,
    Json(request): Json<MacroRunRequest>,
) -> Result<Json<MacroRunResponse>, (StatusCode, String)> {
    // JSON values become rosetta Values through the shared converter, so the
    // Tauri command path and this HTTP path interpret arguments identically.
    let args: Vec<Value> = crate::web_server::json_args_to_values(request.args);
    let run_id = state
        .run_macro(&request.id, request.dry, args)
        .map_err(api_error)?;
    Ok(Json(MacroRunResponse { run_id }))
}

/// `GET /api/macros/run/{run_id}` — status + log tail.
pub(crate) async fn macro_run_status(
    State(state): State<SharedController>,
    UrlPath(run_id): UrlPath<String>,
    Query(query): Query<RunTailQuery>,
) -> Result<Json<crate::rosetta_host::run::RunStatus>, (StatusCode, String)> {
    state
        .macro_run_status(&run_id, query.from.unwrap_or(0))
        .map(Json)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("unknown macro run \"{run_id}\""),
            )
        })
}

/// `POST /api/macros/run/{run_id}/cancel` — cooperative cancellation.
pub(crate) async fn cancel_macro_run(
    State(state): State<SharedController>,
    UrlPath(run_id): UrlPath<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    if state.cancel_macro_run(&run_id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            format!("unknown macro run \"{run_id}\""),
        ))
    }
}

/// `POST /api/program/validate` — capability report for the connected backend.
pub(crate) async fn validate_program(
    State(state): State<SharedController>,
    Json(request): Json<ProgramSourceRequest>,
) -> Result<Json<ValidationReport>, (StatusCode, String)> {
    let source = resolve_source(&state, request.content)?;
    state.validate_program(&source).map(Json).map_err(api_error)
}

/// `POST /api/program/transform/preview` — the «was / became» diff.
pub(crate) async fn preview_transform(
    State(state): State<SharedController>,
    Json(request): Json<ProgramSourceRequest>,
) -> Result<Json<PreviewReport>, (StatusCode, String)> {
    let source = resolve_source(&state, request.content)?;
    state
        .preview_transform(&source)
        .map(Json)
        .map_err(api_error)
}

/// `POST /api/program/transform/apply` — gated pipeline run + reload.
pub(crate) async fn apply_transform(
    State(state): State<SharedController>,
    Json(request): Json<TransformApplyRequest>,
) -> Result<Json<Artifact>, (StatusCode, String)> {
    state
        .apply_transform(&request.plan, request.approved)
        .map(Json)
        .map_err(api_error)
}

/// Uses the request text when present, otherwise the loaded buffer program;
/// an empty buffer is a client error (nothing to validate/transform).
fn resolve_source(
    state: &SharedController,
    content: Option<String>,
) -> Result<String, (StatusCode, String)> {
    if let Some(text) = content {
        return Ok(text);
    }
    state
        .loaded_program_text()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "no program is loaded".to_string()))
}
