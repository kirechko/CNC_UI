import { invokeOrFetch } from "./api.js";

/**
 * Calls a local-data operation through the shared Tauri/HTTP bridge so the browser and desktop UI share one contract.
 * @param {string} command - Registered Tauri command name.
 * @param {unknown} args - Command arguments, serialized as a JSON body in browser mode.
 * @param {string} endpoint - Axum API endpoint.
 * @param {string} [method="GET"] - HTTP method used in browser mode.
 * @returns {Promise<unknown>} Parsed data or null for empty success responses.
 */
function request(command, args, endpoint, method = "GET") {
  return invokeOrFetch(command, args, endpoint, method);
}

/** Loads a profile/tool list, active profile id, resolved snapshot, or a versioned export envelope. */
export function loadLocalMachineData(collection) {
  const entries = {
    profiles: ["get_machine_profiles", "/api/machine-profiles"],
    activeProfile: ["get_active_machine_profile", "/api/machine-profiles/active"],
    resolved: ["get_resolved_machine_configuration", "/api/machine-profiles/resolved"],
    profilesExport: ["export_machine_profiles", "/api/machine-profiles/export"],
    tools: ["get_tools", "/api/tools"],
    toolTypes: ["get_tool_types", "/api/tool-types"],
    toolsExport: ["export_tools", "/api/tools/export"],
  };
  const config = entries[collection];
  if (!config) throw new Error("Unknown local machine data collection");
  return request(config[0], null, config[1]);
}

/** Selects or clears the local profile used by static preflight checks. */
export function selectLocalMachineProfile(profileId) {
  return request("select_machine_profile", { profileId }, "/api/machine-profiles/active", "PUT");
}

/** Saves one local profile/tool record through the shared persistence API. */
export function saveLocalMachineRecord(kind, record) {
  const config = kind === "profile"
    ? ["save_machine_profile", "/api/machine-profiles", { profile: record }]
    : kind === "tool"
      ? ["save_tool", "/api/tools", { tool: record }]
      : null;
  if (!config) throw new Error("Unknown local machine data record type");
  return request(config[0], config[2], config[1], "PUT");
}

/** Imports a versioned collection using backend transactional validation. */
export function importLocalMachineData(kind, document) {
  const config = kind === "profiles"
    ? ["import_machine_profiles", "/api/machine-profiles/import"]
    : kind === "tools"
      ? ["import_tools", "/api/tools/import"]
      : null;
  if (!config) throw new Error("Unknown local machine data collection");
  return request(config[0], { document }, config[1], "POST");
}

/** Creates or updates a type definition through the shared Tauri/HTTP bridge. */
export function saveLocalToolType(record) {
  return request("save_tool_type", { toolType: record }, "/api/tool-types", "PUT");
}

/** Deletes one independent tool type definition by stable id. */
export function deleteLocalToolType(id) {
  return request("delete_tool_type", { id }, `/api/tool-types/${encodeURIComponent(id)}`, "DELETE");
}

/** Deletes one local record by stable id without issuing controller commands. */
export function deleteLocalMachineRecord(kind, id) {
  const config = kind === "profile"
    ? ["delete_machine_profile", "/api/machine-profiles"]
    : kind === "tool"
      ? ["delete_tool", "/api/tools"]
      : null;
  if (!config) throw new Error("Unknown local machine data record type");
  return request(config[0], { id }, `${config[1]}/${encodeURIComponent(id)}`, "DELETE");
}
