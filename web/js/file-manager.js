// File-manager API client: one thin wrapper per `/api/files/*` endpoint plus
// the file-kind classifier the UI uses to decide what "open" means.
//
// All requests carry `lang=<active locale>` so backend error strings arrive
// already translated, which lets the UI show `error.message` verbatim.
//
// NOTE: unlike the other commands this module always uses `fetch` (no Tauri
// `invoke`): uploads/downloads stream raw bytes and the desktop shell does
// not expose the file-manager as Tauri commands yet.
import { getLocale } from "./i18n.js";

/** G-code program extensions opened as toolpath in the 3D viewer. */
const GCODE_EXTENSIONS = new Set(["nc", "ngc", "gcode", "tap", "gcd", "ufp", "mpf"]);

/** Stock (3D model) extensions opened as the workpiece mesh. */
const STOCK_EXTENSIONS = new Set(["stl", "obj"]);

/** Plain-text extensions opened in the G-code editor dialog. */
const TEXT_EXTENSIONS = new Set([
  "txt", "md", "csv", "log", "ini", "cfg", "conf", "yaml", "yaml", "yml", "json",
]);

/**
 * Lowercased extension of a file name without the dot ("" when absent).
 * @param {string} name File or folder name.
 * @returns {string} Extension.
 */
function extensionOf(name) {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

/**
 * Classifies a file for the "open" action.
 * @param {string} name Entry name.
 * @returns {"gcode"|"stock"|"text"|"binary"} The kind; unknown = binary.
 */
export function fileKind(name) {
  const extension = extensionOf(name);
  if (GCODE_EXTENSIONS.has(extension)) return "gcode";
  if (STOCK_EXTENSIONS.has(extension)) return "stock";
  if (TEXT_EXTENSIONS.has(extension)) return "text";
  return "binary";
}

/** Builds an absolute `/api/...` URL with query parameters plus the locale. */
function buildUrl(endpoint, params) {
  const url = new URL(endpoint, window.location.origin);
  Object.entries(params ?? {}).forEach(([key, value]) => {
    if (value !== undefined && value !== null) url.searchParams.set(key, String(value));
  });
  url.searchParams.set("lang", getLocale());
  return url;
}

/** GETs a JSON file-manager endpoint; throws with the translated body text. */
async function apiGet(endpoint, params) {
  const response = await fetch(buildUrl(endpoint, params), { cache: "no-store" });
  const text = await response.text();
  if (!response.ok) throw new Error(text || `HTTP ${response.status}`);
  return text.trim() ? JSON.parse(text) : null;
}

/** POSTs a JSON body to a file-manager endpoint and returns the parsed JSON (or null). */
async function apiPost(endpoint, body) {
  const response = await fetch(buildUrl(endpoint), {
    method: "POST",
    cache: "no-store",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ ...body, lang: getLocale() }),
  });
  const text = await response.text();
  if (!response.ok) throw new Error(text || `HTTP ${response.status}`);
  return text.trim() ? JSON.parse(text) : null;
}

/** Lists the available file-system sources: `[{id,label,available}]`. */
export function fetchSources() {
  return apiGet("/api/files/sources");
}

/** Lists one directory: `{path,entries,totalBytes,usedBytes,occupationPercent}`. */
export function fetchList(source, path) {
  return apiGet("/api/files/list", { source, path });
}

/** Fetches a file decoded as UTF-8 text: `{name,content}`. */
export function fetchContent(source, path) {
  return apiGet("/api/files/content", { source, path });
}

/** Absolute download URL for a file (used by `<a download>` and byte fetches). */
export function downloadUrl(source, path) {
  return buildUrl("/api/files/download", { source, path }).toString();
}

/**
 * Downloads a file as raw bytes (for the 3D stock parsers).
 * @returns {Promise<ArrayBuffer>} Full file content.
 */
export async function fetchBytes(source, path) {
  const response = await fetch(downloadUrl(source, path), { cache: "no-store" });
  if (!response.ok) throw new Error((await response.text()) || `HTTP ${response.status}`);
  return response.arrayBuffer();
}

/** Creates a folder inside `dir` on the given source. */
export function makeFolder(source, dir, name) {
  return apiPost("/api/files/mkdir", { source, path: dir, name });
}

/** Deletes a file or folder. */
export function deleteEntry(source, path) {
  return apiPost("/api/files/delete", { source, path });
}

/** Renames an entry inside its own directory. */
export function renameEntry(source, path, name) {
  return apiPost("/api/files/rename", { source, path, name });
}

/** Moves an entry into another directory (possibly another source). */
export function moveEntry(source, path, toSource, toDir) {
  return apiPost("/api/files/move", { source, path, toSource, toDir });
}

/** Copies an entry into another directory (possibly another source). */
export function copyEntry(source, path, toSource, toDir) {
  return apiPost("/api/files/copy", { source, path, toSource, toDir });
}

/**
 * Uploads raw bytes (or text) as `name` inside `dir`.
 * @param {string} source Fs id.
 * @param {string} dir Destination directory.
 * @param {string} name File name.
 * @param {Blob|string|ArrayBuffer|Uint8Array} data Content.
 */
export function uploadFile(source, dir, name, data) {
  const url = buildUrl("/api/files/upload", { source, path: dir, name });
  return fetch(url, { method: "POST", cache: "no-store", body: data }).then(async (response) => {
    if (!response.ok) throw new Error((await response.text()) || `HTTP ${response.status}`);
    return null;
  });
}
