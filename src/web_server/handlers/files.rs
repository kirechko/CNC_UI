//! File-manager endpoints: source list, directory listing, text content,
//! binary download and the mutating operations (mkdir/delete/rename/move/
//! copy/upload) over the local jail and the two FluidNC volumes.
//!
//! Handlers stay thin: parse → delegate to `web_server::files` dispatch →
//! translate any [`FsError`] into `(400 Bad Request, localized message)`.
//! Uploads are raw request bodies (`application/octet-stream`), not JSON, so
//! a 50 MiB program can stream without doubling memory in a JSON string.

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;

use crate::i18n;
use crate::web_server::files::{
    self, base_name, FsError, FsSource, MAX_TEXT_BYTES, MAX_UPLOAD_BYTES,
};
use crate::web_server::SharedController;

/// Query of every `GET /api/files/*` endpoint.
#[derive(Deserialize, Default)]
pub(crate) struct FilesGetQuery {
    /// `"local" | "controller" | "sd"`; absent for `/sources`.
    source: Option<String>,
    /// Directory (or file for content/download) path inside the source.
    path: Option<String>,
    /// UI language override; falls back to `Accept-Language`.
    lang: Option<String>,
}

/// Shared body of `POST /api/files/mkdir|delete|rename|move|copy`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileMutationBody {
    source: String,
    /// Target directory for mkdir/move/copy, existing path for the rest.
    path: String,
    /// New folder / new file name (mkdir, rename).
    #[serde(default)]
    name: Option<String>,
    /// Destination directory for move/copy.
    #[serde(default)]
    to_dir: Option<String>,
    /// Destination source for move/copy; defaults to `source`.
    #[serde(default)]
    to_source: Option<String>,
    /// UI language override; falls back to `Accept-Language`.
    #[serde(default)]
    lang: Option<String>,
}

/// Query of `POST /api/files/upload` (file bytes ride in the body).
#[derive(Deserialize, Default)]
pub(crate) struct FilesUploadQuery {
    source: Option<String>,
    /// Destination directory.
    path: Option<String>,
    /// File name inside that directory.
    name: Option<String>,
    lang: Option<String>,
}

/// Negotiates a response language from an optional `?lang=` and the headers.
fn negotiate(lang: Option<&str>, headers: &HeaderMap) -> &'static str {
    let accept = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok());
    i18n::negotiate(lang, accept)
}

/// Localized 400 response for one file-manager error key.
fn bad_request(language: &str, key: &str, detail: impl Into<String>) -> (StatusCode, String) {
    (
        StatusCode::BAD_REQUEST,
        i18n::translate(language, key, &[("detail", detail.into())]),
    )
}

/// Turns a dispatch [`FsError`] into the HTTP error tuple.
fn fail(error: FsError, language: &str) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, error.translate(language))
}

/// Parses the `source` parameter (`local` when absent), localized on failure.
fn parse_source(raw: Option<&str>, language: &str) -> Result<FsSource, (StatusCode, String)> {
    let raw = raw.unwrap_or("local");
    FsSource::parse(raw).ok_or_else(|| bad_request(language, "files.error.badSource", raw))
}

/// Normalizes a path parameter (`/` when absent), localized on failure.
fn parse_path(raw: Option<&str>, language: &str) -> Result<String, (StatusCode, String)> {
    let raw = raw.unwrap_or("/");
    files::normalize_path(raw).ok_or_else(|| bad_request(language, "files.error.badPath", raw))
}

/// Validates a single file/folder name: non-empty, no separators, no dots.
fn parse_name(raw: Option<String>, language: &str) -> Result<String, (StatusCode, String)> {
    let name = raw.unwrap_or_default();
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(bad_request(language, "files.error.badPath", name));
    }
    Ok(name)
}

/// Parses the mutation body into `(source, path)` plus the language.
fn parse_mutation(
    body: &FileMutationBody,
    headers: &HeaderMap,
) -> Result<(&'static str, FsSource, String), (StatusCode, String)> {
    let language = negotiate(body.lang.as_deref(), headers);
    let source = parse_source(Some(&body.source), language)?;
    let path = parse_path(Some(&body.path), language)?;
    Ok((language, source, path))
}

/// `GET /api/files/sources`: which file systems exist and are reachable.
pub(crate) async fn get_files_sources(
    State(state): State<SharedController>,
    Query(query): Query<FilesGetQuery>,
    headers: HeaderMap,
) -> Json<Vec<files::FsSourceInfo>> {
    Json(files::sources(
        &state,
        negotiate(query.lang.as_deref(), &headers),
    ))
}

/// `GET /api/files/list?source=&path=`: one directory listing.
pub(crate) async fn get_files_list(
    State(state): State<SharedController>,
    Query(query): Query<FilesGetQuery>,
    headers: HeaderMap,
) -> Result<Json<files::FileListResult>, (StatusCode, String)> {
    let language = negotiate(query.lang.as_deref(), &headers);
    let source = parse_source(query.source.as_deref(), language)?;
    let path = parse_path(query.path.as_deref(), language)?;
    files::list(&state, source, &path)
        .await
        .map(Json)
        .map_err(|error| fail(error, language))
}

/// `GET /api/files/content?source=&path=`: UTF-8 text for the editor.
pub(crate) async fn get_files_content(
    State(state): State<SharedController>,
    Query(query): Query<FilesGetQuery>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let language = negotiate(query.lang.as_deref(), &headers);
    let source = parse_source(query.source.as_deref(), language)?;
    let path = parse_path(query.path.as_deref(), language)?;
    let data = files::read_bytes(&state, source, &path)
        .await
        .map_err(|error| fail(error, language))?;
    if data.len() > MAX_TEXT_BYTES {
        return Err(fail(
            FsError::new("files.error.tooLarge", format!("{} bytes", data.len())),
            language,
        ));
    }
    let content = String::from_utf8(data).map_err(|_| {
        fail(
            FsError::new("files.error.binary", base_name(&path)),
            language,
        )
    })?;
    Ok(Json(serde_json::json!({
        "name": base_name(&path),
        "content": content,
    })))
}

/// `GET /api/files/download?source=&path=`: raw bytes as an attachment.
pub(crate) async fn get_files_download(
    State(state): State<SharedController>,
    Query(query): Query<FilesGetQuery>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let language = negotiate(query.lang.as_deref(), &headers);
    let source = parse_source(query.source.as_deref(), language)?;
    let path = parse_path(query.path.as_deref(), language)?;
    let data = files::read_bytes(&state, source, &path)
        .await
        .map_err(|error| fail(error, language))?;
    // RFC 5987 encoding keeps non-ASCII file names safe in a header.
    let filename = files::percent_encode(base_name(&path));
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        )
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename*=UTF-8''{filename}"),
        )
        .body(axum::body::Body::from(data))
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "download build failed".to_string(),
            )
        })
}

/// `POST /api/files/mkdir` with `{source,path,name}`.
pub(crate) async fn post_files_mkdir(
    State(state): State<SharedController>,
    headers: HeaderMap,
    Json(body): Json<FileMutationBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (language, source, dir) = parse_mutation(&body, &headers)?;
    let name = parse_name(body.name, language)?;
    let path = files::join_path(&dir, &name);
    files::make_dir(&state, source, &path)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| fail(error, language))
}

/// `POST /api/files/delete` with `{source,path}`.
pub(crate) async fn post_files_delete(
    State(state): State<SharedController>,
    headers: HeaderMap,
    Json(body): Json<FileMutationBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (language, source, path) = parse_mutation(&body, &headers)?;
    if path == "/" {
        return Err(bad_request(language, "files.error.denied", "/"));
    }
    files::remove_path(&state, source, &path)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| fail(error, language))
}

/// `POST /api/files/rename` with `{source,path,name}`.
pub(crate) async fn post_files_rename(
    State(state): State<SharedController>,
    headers: HeaderMap,
    Json(body): Json<FileMutationBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (language, source, path) = parse_mutation(&body, &headers)?;
    let name = parse_name(body.name, language)?;
    files::rename_path(&state, source, &path, &name)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| fail(error, language))
}

/// Shared `{source,path,toDir,toSource?}` parsing for move and copy.
///
/// Returns `(language, source, path, destinationSource, destinationDir)`.
fn parse_transfer(
    body: &FileMutationBody,
    headers: &HeaderMap,
) -> Result<(&'static str, FsSource, String, FsSource, String), (StatusCode, String)> {
    let (language, source, path) = parse_mutation(body, headers)?;
    let destination = FsSource::parse(body.to_source.as_deref().unwrap_or(&body.source))
        .ok_or_else(|| {
            bad_request(
                language,
                "files.error.badSource",
                body.to_source.clone().unwrap_or_default(),
            )
        })?;
    let to_dir = parse_path(body.to_dir.as_deref(), language)?;
    Ok((language, source, path, destination, to_dir))
}

/// `POST /api/files/move` with `{source,path,toDir,toSource?}`.
pub(crate) async fn post_files_move(
    State(state): State<SharedController>,
    headers: HeaderMap,
    Json(body): Json<FileMutationBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (language, source, path, destination, to_dir) = parse_transfer(&body, &headers)?;
    files::move_path(&state, source, &path, destination, &to_dir)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| fail(error, language))
}

/// `POST /api/files/copy` with `{source,path,toDir,toSource?}`.
pub(crate) async fn post_files_copy(
    State(state): State<SharedController>,
    headers: HeaderMap,
    Json(body): Json<FileMutationBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (language, source, path, destination, to_dir) = parse_transfer(&body, &headers)?;
    files::copy_path(&state, source, &path, destination, &to_dir)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| fail(error, language))
}

/// `POST /api/files/upload?source=&path=&name=`: raw body bytes.
pub(crate) async fn post_files_upload(
    State(state): State<SharedController>,
    Query(query): Query<FilesUploadQuery>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Result<StatusCode, (StatusCode, String)> {
    let language = negotiate(query.lang.as_deref(), &headers);
    let source = parse_source(query.source.as_deref(), language)?;
    let dir = parse_path(query.path.as_deref(), language)?;
    let name = parse_name(query.name, language)?;
    // Explicit limit (no DefaultBodyLayer needed): oversized uploads fail
    // with a localized `tooLarge` instead of the generic 413.
    let data = axum::body::to_bytes(request.into_body(), MAX_UPLOAD_BYTES)
        .await
        .map_err(|_| {
            fail(
                FsError::new(
                    "files.error.tooLarge",
                    format!("{MAX_UPLOAD_BYTES} bytes max"),
                ),
                language,
            )
        })?;
    files::write_bytes(&state, source, &files::join_path(&dir, &name), &data)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| fail(error, language))
}
