import { deleteLocalMachineRecord, importLocalMachineData, loadLocalMachineData, saveLocalMachineRecord, selectLocalMachineProfile } from "./local-machine-data.js";
import { createToolTypeManager } from "./tool-type-manager.js";
import { t } from "./i18n.js";

/**
 * Initializes the Settings profile/tool manager with advisory metadata editing,
 * persisted preflight selection, per-field source priorities, and read-only resolved values.
 * Metadata operations never configure the controller or send machine commands.
 * @returns {{refresh: () => Promise<void>}} A handle for refreshing visible metadata.
 */
export function initMachineDataDialog({ setMachineProfile = () => {} } = {}) {
  const typeSelect = document.querySelector("#machine-data-type");
  const list = document.querySelector("#machine-data-list");
  const activeProfileSummary = document.querySelector("#active-profile-summary");
  const profileSelectorDialog = document.querySelector("#profile-selector-dialog");
  const profileSelectorList = document.querySelector("#profile-selector-list");
  const profileDetails = document.querySelector("#active-profile-details");
  const toolReferenceSelect = document.querySelector('[name="toolReference"]');
  const form = document.querySelector("#machine-data-form");
  const status = document.querySelector("#machine-data-status");
  const snapshot = document.querySelector("#machine-data-snapshot");
  const editorView = document.querySelector("#machine-data-editor-view");
  const helpView = document.querySelector("#machine-data-help-view");
  if (!typeSelect || !list || !form || !status) return { refresh: async () => {} };

  let profiles = [];
  let activeProfileId = null;
  let resolvedSnapshot = null;
  let availableTools = [];
  let tools = [];
  let toolTypes = [];

  /** Resolves a localized message while retaining a usable English fallback for incomplete locales. */
  function localized(key, fallback) {
    const translated = t(key);
    return translated === key ? fallback : translated;
  }

  /** Writes plain-text status feedback without interpreting it as markup. */
  function showStatus(message) {
    status.textContent = message;
  }

  /** Converts blank optional number fields to null so unknown remains distinct from zero. */
  function optionalNumber(value) {
    return value.trim() === "" ? null : Number(value);
  }

  /** Rebuilds the profile's tool reference picker and preserves its selected local tool id. */
  function renderToolReferences(selectedId = toolReferenceSelect?.value ?? "") {
    if (!toolReferenceSelect) return;
    toolReferenceSelect.replaceChildren(new Option(localized("machineData.noTool", "No tool associated"), ""));
    for (const tool of tools) {
      const type = toolTypes.find((candidate) => candidate.id === tool.toolTypeId);
      const display = type?.name ?? tool.toolType;
      toolReferenceSelect.add(new Option(`${display}${tool.diameter == null ? "" : ` · Ø${tool.diameter}`}`, tool.id));
    }
    toolReferenceSelect.value = selectedId;
  }

  /** Formats missing metadata explicitly rather than implying an unlimited machine range. */
  function displayValue(value) {
    return value === null || value === undefined ? localized("machineData.unknown", "Unknown") : String(value);
  }

  /** Reads whether a profile contains a value for one optional power/axis field. */
  function profileHasValue(record, field, axis) {
    if (!record) return false;
    if (field === "power") return record.power != null;
    if (field === "travel") return record.travel?.[`${axis.toLowerCase()}Max`] != null || record.travel?.[`${axis.toLowerCase()}Min`] != null;
    if (field === "speeds") return record.speeds?.[axis.toLowerCase()] != null;
    return true;
  }

  /** Applies an explicit source class so values and provenance stay visually aligned. */
  function markSource(element, source) {
    element.classList.add("machine-data-source-value", `source-${source || "unknown"}`);
    if (source === "profile") element.title = localized("machineData.sourceProfile", "Value from local profile");
    else if (source === "controller") element.title = localized("machineData.sourceController", "Value from controller snapshot");
    else element.title = localized("machineData.sourceUnknown", "Value source unknown");
  }

  /** Displays each configured field's actual winning source and the values supplied by both sources. */
  function renderSnapshot(configuration) {
    if (!snapshot) return;
    const activeProfile = profiles.find((item) => item.id === configuration.profileId);
    snapshot.replaceChildren();
    const summary = document.createElement("p");
    const machineType = configuration.machineType ?? {};
    const axisSet = configuration.effectiveAxes ?? {};
    const units = configuration.effectiveUnits ?? {};
    for (const [title, resolved] of [
      [localized("machineData.machineType", "Machine type"), machineType],
      [localized("machineData.axes", "Axes"), axisSet],
      [localized("machineData.units", "Units"), units],
    ]) {
      const item = document.createElement("span");
      item.textContent = `${title}: ${Array.isArray(resolved.value) ? resolved.value.join(", ") : displayValue(resolved.value)}`;
      markSource(item, resolved.source);
      if (summary.childNodes.length > 0) summary.append(document.createTextNode(" · "));
      summary.append(item);
    }
    snapshot.append(summary);

    const table = document.createElement("table");
    table.className = "machine-data-snapshot-table";
    const headings = [
      localized("machineData.axis", "Axis"),
      localized("machineData.profileTravel", "Profile travel"),
      localized("machineData.controllerMaxTravel", "Controller max travel"),
      localized("machineData.effectiveTravel", "Effective travel"),
      localized("machineData.profileMaxSpeed", "Profile max speed"),
      localized("machineData.controllerMaxSpeed", "Controller max speed"),
      localized("machineData.effectiveSpeed", "Effective speed"),
    ];
    const header = document.createElement("tr");
    for (const title of headings) {
      const cell = document.createElement("th");
      cell.scope = "col";
      cell.textContent = title;
      header.append(cell);
    }
    const head = document.createElement("thead");
    head.append(header);
    table.append(head);
    const body = document.createElement("tbody");
    for (const axis of configuration.axes ?? []) {
      const row = document.createElement("tr");
      const effectiveTravel = `${displayValue(axis.effectiveTravelMin?.value)} – ${displayValue(axis.effectiveTravelMax?.value)}`;
      const effectiveSpeed = displayValue(axis.effectiveMaxSpeed?.value);
      const cells = [
        [axis.axis, "unknown"],
        [`${displayValue(axis.profileTravel?.[0])} – ${displayValue(axis.profileTravel?.[1])}`, "profile"],
        [displayValue(axis.controllerMaxTravel), "controller"],
        [effectiveTravel, axis.effectiveTravelMax?.source ?? axis.effectiveTravelMin?.source ?? "unknown"],
        [displayValue(axis.profileMaxSpeed), "profile"],
        [displayValue(axis.controllerMaxSpeed), "controller"],
        [effectiveSpeed, axis.effectiveMaxSpeed?.source ?? "unknown"],
      ];
      for (const [value, source] of cells) {
        const cell = document.createElement("td");
        cell.textContent = value;
        markSource(cell, source);
        row.append(cell);
      }
      body.append(row);
    }
    table.append(body);
    snapshot.append(table);

    const power = configuration.processPower?.value;
    const powerSummary = document.createElement("p");
    powerSummary.textContent = `${localized("machineData.powerLimits", "Process limits")}: ${power ? `${power.kind} ${displayValue(power.min)}–${displayValue(power.max)}` : localized("machineData.noPower", "No process limits")}`;
    markSource(powerSummary, configuration.processPower?.source);
    snapshot.append(powerSummary);

    const linkedTool = configuration.toolReference
      ? tools.find((item) => item.id === configuration.toolReference)
      : null;
    if (linkedTool) {
      const type = toolTypes.find((item) => item.id === linkedTool.toolTypeId);
      const toolSummary = document.createElement("p");
      toolSummary.textContent = `${localized("machineData.toolReference", "Associated tool")}: ${type?.name ?? linkedTool.toolType}${linkedTool.diameter == null ? "" : ` · Ø${linkedTool.diameter}`} (${localized("machineData.referenceFound", "found in local library")})`;
      snapshot.append(toolSummary);
    } else if (configuration.toolReference) {
      const missing = document.createElement("p");
      missing.className = "machine-data-conflict";
      missing.textContent = `${localized("machineData.toolReference", "Associated tool")}: ${localized("machineData.referenceMissing", "missing from local library")}`;
      snapshot.append(missing);
    }
    for (const conflict of configuration.conflicts ?? []) {
      const warning = document.createElement("p");
      warning.className = "machine-data-conflict";
      warning.textContent = conflict;
      snapshot.append(warning);
    }
    if (activeProfile) {
      const profileSummary = document.createElement("p");
      profileSummary.textContent = `${localized("machineData.profileFirst", "Profile values")}: ${activeProfile.name}`;
      snapshot.append(profileSummary);
    }
  }

  /** Updates priority controls and records controller-derived values temporarily for reversible editing. */
  async function renderPriorityControls(record) {
    const fields = form.querySelector("[data-profile-fields]");
    if (!fields) return;
    for (const existing of fields.querySelectorAll("[data-source-priority]")) existing.closest("label")?.remove();
    const resolved = await loadLocalMachineData("resolved").catch(() => null);
    const defaults = { machineType: "profile_first", axes: "profile_first", units: "profile_first", travel: "profile_first", speeds: "profile_first", power: "profile_first" };
    const priorities = { ...defaults, ...(record?.sourcePriorities ?? {}) };
    form.dataset.sourcePriorities = JSON.stringify(priorities);
    const entries = [
      ["machineData.priorityMachineType", "machineType"],
      ["machineData.priorityAxes", "axes"],
      ["machineData.priorityUnits", "units"],
      ["machineData.priorityTravel", "travel"],
      ["machineData.prioritySpeeds", "speeds"],
      ["machineData.priorityPower", "power"],
    ];
    for (const [labelKey, field] of entries) {
      const label = document.createElement("label");
      label.className = "machine-data-priority-control";
      const caption = document.createElement("span");
      caption.textContent = localized(labelKey, labelKey);
      const select = document.createElement("select");
      select.dataset.sourcePriority = field;
      select.add(new Option(localized("machineData.profileFirst", "Prefer profile"), "profile_first"));
      select.add(new Option(localized("machineData.controllerFirst", "Prefer machine"), "controller_first"));
      select.value = priorities[field];
      const updateSource = () => {
        const prefersProfile = select.value === "profile_first";
        label.querySelector("small.machine-data-source-value")?.remove();
        const configuredProfileValue = field === "power"
          ? Boolean(record?.power ?? form.dataset.profilePowerDefault)
          : field === "travel"
            ? ["x", "y", "z"].some((axis) => record?.travel?.[`${axis}Min`] != null || record?.travel?.[`${axis}Max`] != null || form.querySelector(`[name="${axis}Min"]`)?.value || form.querySelector(`[name="${axis}Max"]`)?.value || form.dataset[`profile${axis.toUpperCase()}MaxDefault`])
            : field === "speeds"
              ? ["x", "y", "z"].some((axis) => record?.speeds?.[axis] != null || form.querySelector(`[name="speed${axis.toUpperCase()}"]`)?.value || form.dataset[`profile${axis.toUpperCase()}SpeedDefault`])
              : Boolean(record) || field === "machineType" || field === "axes" || field === "units";
        const controllerValue = field === "units" ? null : field === "power" ? resolved?.processPower : field === "machineType" ? resolved?.machineType : field === "axes" ? resolved?.effectiveAxes : (resolved?.axes ?? []).find((axis) => axis.controllerMaxTravel != null || axis.controllerMaxSpeed != null);
        const controllerHasValue = field === "travel" || field === "speeds"
          ? Boolean((resolved?.axes ?? []).some((axis) => axis.controllerMaxTravel != null || axis.controllerMaxSpeed != null))
          : Boolean(controllerValue?.source === "controller");
        label.dataset.effectiveSource = prefersProfile
          ? configuredProfileValue ? "profile" : controllerHasValue ? "controller" : "unknown"
          : controllerHasValue ? "controller" : configuredProfileValue ? "profile" : "unknown";
        label.dataset.tooltipTitle = localized(`machineData.source${label.dataset.effectiveSource[0].toUpperCase()}${label.dataset.effectiveSource.slice(1)}`, label.dataset.effectiveSource);
        const indicator = document.createElement("small");
        indicator.className = `machine-data-source-value source-${label.dataset.effectiveSource}`;
        indicator.textContent = localized(`machineData.source${label.dataset.effectiveSource[0].toUpperCase()}${label.dataset.effectiveSource.slice(1)}`, label.dataset.effectiveSource);
        label.append(indicator);
      };
      select.addEventListener("change", async () => {
        const updated = JSON.parse(form.dataset.sourcePriorities || "{}");
        updated[field] = select.value;
        form.dataset.sourcePriorities = JSON.stringify(updated);
        const activeProfile = profiles.find((item) => item.id === form.elements.id.value);
        const controllerSnapshot = await loadLocalMachineData("resolved").catch(() => null);
        if (select.value === "controller_first" && controllerSnapshot) {
          if (field === "power" && controllerSnapshot.processPower?.source === "controller") {
            const value = controllerSnapshot.processPower.value;
            form.querySelector('[name="powerKind"]').value = value.kind;
            form.querySelector('[name="powerMin"]').value = value.min ?? "";
            form.querySelector('[name="powerMax"]').value = value.max ?? "";
            if (!activeProfile?.power) form.dataset.profilePowerDefault = JSON.stringify(value);
          }
          if (field === "travel" || field === "speeds") {
            for (const axis of controllerSnapshot.axes ?? []) {
              if (field === "travel" && axis.controllerMaxTravel != null) {
                const key = `${axis.axis.toLowerCase()}Max`;
                form.querySelector(`[name="${key}"]`).value = axis.controllerMaxTravel;
                if (activeProfile?.travel?.[key] == null) form.dataset[`profile${axis.axis}MaxDefault`] = String(axis.controllerMaxTravel);
              }
              if (field === "speeds" && axis.controllerMaxSpeed != null) {
                const key = `speed${axis.axis}`;
                form.querySelector(`[name="${key}"]`).value = axis.controllerMaxSpeed;
                if (activeProfile?.speeds?.[axis.axis.toLowerCase()] == null) form.dataset[`profile${axis.axis}SpeedDefault`] = String(axis.controllerMaxSpeed);
              }
            }
          }
        } else if (select.value === "profile_first" && activeProfile) {
          if (field === "power") {
            const value = activeProfile.power ?? (form.dataset.profilePowerDefault ? JSON.parse(form.dataset.profilePowerDefault) : null);
            if (value) {
              form.querySelector('[name="powerKind"]').value = value.kind;
              form.querySelector('[name="powerMin"]').value = value.min ?? "";
              form.querySelector('[name="powerMax"]').value = value.max ?? "";
            }
          }
          if (field === "travel" || field === "speeds") {
            for (const axis of ["x", "y", "z"]) {
              const capitalized = axis.toUpperCase();
              if (field === "travel") {
                const key = `${axis}Max`;
                form.querySelector(`[name="${key}"]`).value = activeProfile.travel?.[key] ?? form.dataset[`profile${capitalized}MaxDefault`] ?? "";
              }
              if (field === "speeds") {
                form.querySelector(`[name="speed${capitalized}"]`).value = activeProfile.speeds?.[axis] ?? form.dataset[`profile${capitalized}SpeedDefault`] ?? "";
              }
            }
          }
        }
        updateSource();
      });
      label.append(caption, select);
      fields.append(label);
      updateSource();
    }
  }

  /** Re-renders the last resolved data after local tool metadata changes. */
  function renderSnapshotFromCache() {
    if (snapshot && resolvedSnapshot) renderSnapshot(resolvedSnapshot);
  }

  /** Refreshes all local libraries, active profile, and the cached read-only controller snapshot. */
  async function loadAll() {
    [profiles, tools, toolTypes] = await Promise.all([loadLocalMachineData("profiles"), loadLocalMachineData("tools"), loadLocalMachineData("toolTypes")]);
    availableTools = tools;
    renderToolTypeOptions();
    renderList();
    renderToolReferences();
    activeProfileId = await loadLocalMachineData("activeProfile");
    if (activeProfileSummary) {
      const selectedProfile = profiles.find((profile) => profile.id === activeProfileId);
      activeProfileSummary.textContent = `${localized("machineData.profileLabel", "Profile")}: “${selectedProfile?.name ?? localized("machineData.noProfile", "Not selected")}”`;
    }
    renderProfileSelector();
    resolvedSnapshot = await loadLocalMachineData("resolved");
    const currentProfile = activeProfileId ? profiles.find((item) => item.id === activeProfileId) ?? null : null;
    setMachineProfile(currentProfile, tools);
    renderActiveProfileDetails(currentProfile, resolvedSnapshot);
    if (snapshot) renderSnapshot(resolvedSnapshot);
  }


  /** Renders the active profile's effective fields with source-colored backgrounds. */
  function renderActiveProfileDetails(profile, configuration) {
    if (!profileDetails) return;
    profileDetails.replaceChildren();
    const fields = [
      [localized("machineData.machineType", "Machine type"), configuration?.machineType],
      [localized("machineData.axes", "Axes"), configuration?.effectiveAxes],
      [localized("machineData.units", "Units"), configuration?.effectiveUnits],
      ...((configuration?.axes ?? []).flatMap((axis) => [
        [`${axis.axis} ${localized("machineData.effectiveTravel", "Travel")}`, { value: `${displayValue(axis.effectiveTravelMin?.value)}–${displayValue(axis.effectiveTravelMax?.value)}`, source: axis.effectiveTravelMax?.source ?? axis.effectiveTravelMin?.source }],
        [`${axis.axis} ${localized("machineData.effectiveSpeed", "Max speed")}`, axis.effectiveMaxSpeed],
      ])),
      [localized("machineData.powerLimits", "Spindle / laser limits"), configuration?.processPower],
    ];
    for (const [label, resolved] of fields) {
      if (!resolved) continue;
      const row = document.createElement("div");
      const title = document.createElement("strong");
      title.textContent = `${label}: `;
      const value = document.createElement("span");
      value.textContent = Array.isArray(resolved.value) ? resolved.value.join(", ") : typeof resolved.value === "object" && resolved.value ? `${resolved.value.kind} ${displayValue(resolved.value.min)}–${displayValue(resolved.value.max)}` : displayValue(resolved.value);
      row.className = `profile-field-value source-${resolved.source ?? "unknown"}`;
      row.append(title, value);
      profileDetails.append(row);
    }
    if (!profile) profileDetails.dataset.empty = "true";
    else delete profileDetails.dataset.empty;
  }

  /** Renders a separate profile selector without mixing it with record editing controls. */
  function renderProfileSelector() {
    if (!profileSelectorList) return;
    profileSelectorList.replaceChildren();
    for (const profile of profiles) {
      const row = document.createElement("div");
      row.className = "machine-data-row";
      const label = document.createElement("span");
      label.textContent = `${profile.name} — ${profile.machineType} (${profile.units})`;
      const choose = document.createElement("button");
      choose.type = "button";
      choose.textContent = activeProfileId === profile.id
        ? localized("machineData.activeProfile", "Active preflight profile")
        : localized("machineData.setActive", "Set working profile");
      choose.disabled = activeProfileId === profile.id;
      choose.addEventListener("click", async () => {
        try {
          await selectLocalMachineProfile(profile.id);
          activeProfileId = profile.id;
          await loadAll();
          profileSelectorDialog?.close();
          showStatus(localized("machineData.activeProfileWarning", "The local profile is now selected for preflight; controller settings were not changed."));
        } catch (error) { showStatus(error.message); }
      });
      row.append(label, choose);
      profileSelectorList.append(row);
    }
    const clearRow = document.createElement("div");
    clearRow.className = "machine-data-row";
    const clearLabel = document.createElement("span");
    clearLabel.textContent = localized("machineData.noProfile", "No profile selected");
    const clearButton = document.createElement("button");
    clearButton.type = "button";
    clearButton.textContent = activeProfileId === null ? localized("machineData.activeProfile", "Active preflight profile") : localized("machineData.clearProfile", "Clear profile");
    clearButton.disabled = activeProfileId === null;
    clearButton.addEventListener("click", async () => {
      try {
        await selectLocalMachineProfile(null);
        activeProfileId = null;
        await loadAll();
        profileSelectorDialog?.close();
        showStatus(localized("machineData.activeProfileWarning", "The local profile is now selected for preflight; controller settings were not changed."));
      } catch (error) { showStatus(error.message); }
    });
    clearRow.append(clearLabel, clearButton);
    profileSelectorList.append(clearRow);
    if (profiles.length === 0) {
      const empty = document.createElement("p");
      empty.textContent = localized("machineData.empty", "No profiles configured.");
      profileSelectorList.append(empty);
    }
  }

  /** Renders profile/tool summaries with safe text-only content and actions. */
  function renderList() {
    const kind = typeSelect.value;
    const records = kind === "profile" ? profiles : tools;
    list.replaceChildren();
    for (const record of records) {
      const row = document.createElement("div");
      row.className = "machine-data-row";
      const label = document.createElement("span");
      if (kind === "profile") {
        const active = activeProfileId === record.id;
        label.textContent = `${active ? "● " : ""}${record.name} — ${record.machineType} (${record.units})`;
      } else {
        const type = toolTypes.find((item) => item.id === record.toolTypeId);
        label.textContent = `${type?.name ?? record.toolType}${record.diameter == null ? "" : ` · Ø${record.diameter}`}${record.photo ? ` · ${record.photo}` : ""}`;
      }
      const edit = document.createElement("button");
      edit.type = "button";
      edit.textContent = localized("machineData.edit", "Edit");
      edit.addEventListener("click", () => fillForm(record));
      const remove = document.createElement("button");
      remove.type = "button";
      remove.textContent = localized("machineData.delete", "Delete");
      remove.addEventListener("click", async () => {
        if (!window.confirm(`${localized("machineData.delete", "Delete")} ${label.textContent}?`)) return;
        try {
          await deleteLocalMachineRecord(kind, record.id);
          await loadAll();
          fillForm();
          showStatus(localized("machineData.deleted", "Local record deleted."));
        } catch (error) {
          showStatus(error.message);
        }
      });
      row.append(label, edit, remove);
      list.append(row);
    }
    if (records.length === 0) {
      const empty = document.createElement("p");
      empty.textContent = localized("machineData.empty", "No records in this collection yet.");
      list.append(empty);
    }
  }

  /** Disables the inactive record kind so native form validation applies only to visible controls. */
  function setRecordFieldsDisabled(profileMode) {
    form.querySelector("[data-profile-fields]").disabled = !profileMode;
    form.querySelector("[data-tool-fields]").disabled = profileMode;
  }

  /** Refreshes the tool-type selector while preserving the current type reference. */
  function renderToolTypeOptions(selectedId = form.elements.toolTypeId?.value ?? "") {
    const select = form.elements.toolTypeId;
    if (!select) return;
    select.replaceChildren();
    for (const item of toolTypes) select.add(new Option(`${item.name} · ${item.cuttingType}`, item.id));
    select.value = toolTypes.some((item) => item.id === selectedId) ? selectedId : (toolTypes[0]?.id ?? "");
  }

  /** Resets the editor or fills every supported profile/tool field from one record. */
  function fillForm(record = null) {
    const profileMode = typeSelect.value === "profile";
    form.reset();
    form.dataset.profilePowerDefault = "";
    for (const axis of ["X", "Y", "Z"]) {
      delete form.dataset[`profile${axis}MaxDefault`];
      delete form.dataset[`profile${axis}SpeedDefault`];
    }
    setRecordFieldsDisabled(profileMode);
    form.elements.id.value = record?.id ?? crypto.randomUUID();
    form.querySelector('[name="name"]').value = record?.name ?? "";
    form.querySelector('[name="category"]').value = record?.machineType ?? "mill";
    form.querySelector('[name="units"]').value = record?.units ?? "millimeters";
    for (const axis of ["x", "y", "z"]) {
      const capitalized = axis.toUpperCase();
      form.querySelector(`[name="${axis}Min"]`).value = record?.travel?.[`${axis}Min`] ?? "";
      form.querySelector(`[name="${axis}Max"]`).value = record?.travel?.[`${axis}Max`] ?? "";
      form.querySelector(`[name="speed${capitalized}"]`).value = record?.speeds?.[axis] ?? "";
    }
    form.querySelector('[name="powerKind"]').value = record?.power?.kind ?? (record?.machineType === "laser" ? "laser" : "spindle");
    form.querySelector('[name="powerMin"]').value = record?.power?.min ?? "";
    form.querySelector('[name="powerMax"]').value = record?.power?.max ?? "";
    renderToolReferences(record?.toolReference ?? "");
    form.querySelector('[name="toolNumber"]').value = record?.number ?? "";
    renderToolTypeOptions(record?.toolTypeId ?? toolTypes.find((item) => item.name.toLowerCase() === (record?.toolType ?? "end mill").toLowerCase())?.id ?? "");
    form.querySelector('[name="diameter"]').value = record?.diameter ?? "";
    form.querySelector('[name="length"]').value = record?.length ?? "";
    form.querySelector('[name="material"]').value = record?.material ?? "";
    form.querySelector('[name="rpm"]').value = record?.rpm ?? "";
    form.querySelector('[name="feed"]').value = record?.feed ?? "";
    form.querySelector('[name="photo"]').value = record?.photo ?? "";
    form.dataset.recordType = profileMode ? "profile" : "tool";
    if (profileMode) void renderPriorityControls(record).catch((error) => showStatus(error.message));
  }

  /** Persists exactly one local metadata record without touching the active controller. */
  async function save(event) {
    event.preventDefault();
    const kind = form.dataset.recordType || typeSelect.value;
    const now = Date.now();
    const id = form.elements.id.value;
    try {
      if (kind === "profile") {
        const existing = profiles.find((item) => item.id === id);
        const powerMin = optionalNumber(form.querySelector('[name="powerMin"]').value);
        const powerMax = optionalNumber(form.querySelector('[name="powerMax"]').value);
        const sourcePriorities = Object.fromEntries([...form.querySelectorAll("[data-source-priority]")].map((control) => [control.dataset.sourcePriority, control.value]));
        const power = powerMin === null && powerMax === null ? null : { kind: form.querySelector('[name="powerKind"]').value, min: powerMin, max: powerMax };
        const rememberedPower = existing?.power ?? (form.dataset.profilePowerDefault ? JSON.parse(form.dataset.profilePowerDefault) : null);
        await saveLocalMachineRecord("profile", {
          id,
          name: form.querySelector('[name="name"]').value,
          machineType: form.querySelector('[name="category"]').value,
          units: form.querySelector('[name="units"]').value,
          travel: {
            xMin: optionalNumber(form.querySelector('[name="xMin"]').value), xMax: optionalNumber(form.querySelector('[name="xMax"]').value),
            yMin: optionalNumber(form.querySelector('[name="yMin"]').value), yMax: optionalNumber(form.querySelector('[name="yMax"]').value),
            zMin: optionalNumber(form.querySelector('[name="zMin"]').value), zMax: optionalNumber(form.querySelector('[name="zMax"]').value),
          },
          speeds: {
            x: optionalNumber(form.querySelector('[name="speedX"]').value),
            y: optionalNumber(form.querySelector('[name="speedY"]').value),
            z: optionalNumber(form.querySelector('[name="speedZ"]').value),
          },
          power: power ?? rememberedPower ?? null,
          sourcePriorities,
          toolReference: form.querySelector('[name="toolReference"]').value || null,
          extensions: existing?.extensions ?? {},
          createdAtMs: existing?.createdAtMs ?? now,
          updatedAtMs: now,
        });
      } else {
        const existing = tools.find((item) => item.id === id);
        const selectedType = toolTypes.find((item) => item.id === form.elements.toolTypeId.value);
        await saveLocalMachineRecord("tool", {
          id,
          number: Number(form.elements.toolNumber.value),
          toolTypeId: form.elements.toolTypeId.value || null,
          toolType: selectedType?.name ?? "other",
          geometry: existing?.geometry ?? selectedType?.geometryModel ?? null,
          diameter: optionalNumber(form.elements.diameter.value),
          length: optionalNumber(form.querySelector('[name="length"]').value),
          material: form.querySelector('[name="material"]').value || null,
          rpm: optionalNumber(form.querySelector('[name="rpm"]').value),
          feed: optionalNumber(form.querySelector('[name="feed"]').value),
          photo: form.querySelector('[name="photo"]').value || null,
          extensions: existing?.extensions ?? {},
        });
      }
      await loadAll();
      showStatus(localized("machineData.saved", "Saved as local metadata. Machine settings and motion were not changed."));
    } catch (error) {
      showStatus(error.message);
    }
  }

  /** Downloads the versioned JSON document for the selected library. */
  async function exportSelected() {
    const kind = typeSelect.value === "profile" ? "profiles" : "tools";
    try {
      const exportDocument = await loadLocalMachineData(kind === "profiles" ? "profilesExport" : "toolsExport");
      const blob = new Blob([JSON.stringify(exportDocument, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `${kind}.json`;
      anchor.click();
      URL.revokeObjectURL(url);
      showStatus(localized("machineData.exportDownloaded", "Export downloaded."));
    } catch (error) { showStatus(error.message); }
  }

  /** Parses a selected JSON file and delegates validation to the transactional backend importer. */
  async function importSelected(file) {
    if (!file) return;
    try {
      const imported = JSON.parse(await file.text());
      const kind = typeSelect.value === "profile" ? "profiles" : "tools";
      await importLocalMachineData(kind, imported);
      await loadAll();
      showStatus(localized("machineData.importCompleted", "Import completed. No machine commands were sent."));
    } catch (error) { showStatus(error.message); }
  }

  /** Switches between the manager and separate contextual help content. */
  function showHelp(show) {
    if (editorView) editorView.hidden = show;
    if (helpView) helpView.hidden = !show;
  }

  /** Refreshes local metadata and the read-only snapshot when the Settings tab opens. */
  async function refresh() {
    showStatus(localized("machineData.loading", "Loading local profiles and tools…"));
    try {
      await loadAll();
      fillForm();
      showStatus(localized("machineData.warning", "Advisory metadata only; controller settings are not modified."));
    } catch (error) {
      list.replaceChildren();
      if (snapshot) snapshot.textContent = error.message;
      showStatus(error.message);
    }
  }


  createToolTypeManager({
    manager: document.querySelector("#tool-type-manager"),
    list: document.querySelector("#tool-type-list"),
    form: document.querySelector("#tool-type-form"),
    openButton: document.querySelector("#machine-data-manage-tool-types"),
    closeButton: document.querySelector("#tool-type-manager-close"),
    editorView,
    onChange: (records) => { toolTypes = records; renderToolTypeOptions(); renderToolReferences(); renderSnapshotFromCache(); },
    onStatus: showStatus,
  });

  document.querySelector("#profile-selector-open")?.addEventListener("click", () => profileSelectorDialog?.showModal());
  document.querySelector("#profile-selector-close")?.addEventListener("click", () => profileSelectorDialog?.close());
  window.addEventListener("cnc-open-settings-machine-data", () => void refresh());
  typeSelect.addEventListener("change", () => { fillForm(); renderList(); });
  form.addEventListener("submit", (event) => void save(event));
  document.querySelector("#machine-data-new")?.addEventListener("click", () => fillForm());
  document.querySelector("#machine-data-export")?.addEventListener("click", () => void exportSelected());
  document.querySelector("#machine-data-import")?.addEventListener("change", (event) => {
    void importSelected(event.target.files?.[0]);
    event.target.value = "";
  });
  document.querySelector("#machine-data-help-open")?.addEventListener("click", () => showHelp(true));
  document.querySelector("#machine-data-help-close")?.addEventListener("click", () => showHelp(false));
  document.querySelector("#settings-machine-data-tab")?.addEventListener("click", () => void refresh());
  window.addEventListener("cnc-locale-changed", () => { renderList(); renderToolReferences(); });
  fillForm();
  showHelp(false);
  return { refresh };
}
