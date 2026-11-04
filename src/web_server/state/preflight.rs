//! Static preflight analysis composed from the canonical Rosetta Toolpath IR, local profiles and tools.
//!
//! This is a source-level safety aid, not a controller simulator, proof of firmware capability, or
//! substitute for an operator check. Unknown profile limits remain warnings and never become safe bounds.
//! Rotary-axis travel, unbounded rotary declarations, and full arc envelopes are not analyzed yet.

use super::AppState;
use crate::web_server::machine_profiles::Units as ProfileUnits;
use crate::web_server::tool_library::ToolRecord;
use rosetta::ir::capability::{BackendType, CapabilityRegistry};
use rosetta::ir::toolpath::{Code, Toolpath};
use serde::Serialize;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};

/// Severity level for one static preflight finding.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightSeverity {
    /// The program exceeds a known configured travel bound and must not be run unchanged.
    Blocker,
    /// The static analyzer cannot establish a safety-relevant fact and needs operator review.
    Warning,
    /// Informational metadata such as a matched tool record.
    Info,
}

/// One actionable finding tied to an optional 1-based source line.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightFinding {
    /// Classification used by the UI to gate Run separately from advisory items.
    pub severity: PreflightSeverity,
    /// Short stable category suitable for UI localization and filtering.
    pub code: String,
    /// 1-based source line when the finding comes from a specific code frame.
    pub line: Option<usize>,
    /// Human-readable detail; localization can be layered by the frontend later.
    pub detail: String,
}

/// Static report returned by `POST /api/program/preflight`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    /// Backend family used by the existing Rosetta capability matrix.
    pub backend: String,
    /// Selected local profile id, or `None` when no profile was supplied.
    pub profile_id: Option<String>,
    /// Stable hash over the complete analyzed source, selected profile and findings.
    pub fingerprint: String,
    /// True when at least one known hard travel-bound violation was found.
    pub blocked: bool,
    /// Findings in source traversal order followed by profile-level advisories.
    pub findings: Vec<PreflightFinding>,
    /// Read-only resolved profile/controller snapshot and conflict provenance.
    pub configuration: Option<crate::web_server::machine_profiles::ResolvedConfiguration>,
}

/// Checks parsed linear segment endpoints against explicitly known effective travel bounds only.
fn check_profile_bounds(
    path: &Toolpath,
    profile_name: Option<&str>,
    units: ProfileUnits,
    axes: &[crate::web_server::machine_profiles::resolution::ResolvedAxis],
    findings: &mut Vec<PreflightFinding>,
) {
    let factor = match units {
        ProfileUnits::Millimeters => path
            .units
            .factor_to(rosetta::ir::toolpath::Units::Millimeters),
        ProfileUnits::Inches => path.units.factor_to(rosetta::ir::toolpath::Units::Inches),
    };
    for (axis_index, axis) in axes.iter().enumerate() {
        let min = axis.effective_travel_min.value;
        let max = axis.effective_travel_max.value;
        if min.is_none() || max.is_none() {
            findings.push(PreflightFinding {
                severity: PreflightSeverity::Warning,
                code: "travel_limit_unknown".into(),
                line: None,
                detail: format!(
                    "{} travel limit is not fully configured{}; this axis was not checked",
                    axis.axis,
                    profile_name
                        .map(|name| format!(" in profile `{name}`"))
                        .unwrap_or_default()
                ),
            });
            continue;
        }
        let (Some(min), Some(max)) = (min, max) else {
            continue;
        };
        for segment in &path.segments {
            for point in [&segment.from, &segment.to] {
                let coordinate = match axis_index {
                    0 => point.x,
                    1 => point.y,
                    _ => point.z,
                };
                let value = coordinate * factor;
                if value < min || value > max {
                    findings.push(PreflightFinding {
                        severity: PreflightSeverity::Blocker,
                        code: "travel_limit_exceeded".into(),
                        line: Some(segment.line),
                        detail: format!("{}={value:.4} is outside effective configured range [{min:.4}, {max:.4}]", axis.axis),
                    });
                    break;
                }
            }
        }
    }
}

/// Finds programmed T words and reports which local tool records can provide operator-facing metadata.
fn check_tools(path: &Toolpath, tools: &[ToolRecord], findings: &mut Vec<PreflightFinding>) {
    let known: HashSet<u32> = tools.iter().map(|tool| tool.number).collect();
    let mut seen = HashSet::new();
    for frame in &path.codes {
        for code in &frame.codes {
            if let Code::Other(token) = code {
                let upper = token.to_ascii_uppercase();
                if let Some(number) = upper
                    .strip_prefix('T')
                    .and_then(|value| value.parse::<u32>().ok())
                {
                    if !seen.insert(number) {
                        continue;
                    }
                    let is_known = known.contains(&number);
                    findings.push(PreflightFinding {
                        severity: if is_known { PreflightSeverity::Info } else { PreflightSeverity::Warning },
                        code: if is_known { "tool_record_found" } else { "tool_record_missing" }.into(),
                        line: Some(frame.line),
                        detail: if is_known { format!("T{number} matches a local tool record; no automatic tool change is performed") } else { format!("T{number} has no local tool record") },
                    });
                }
            }
        }
    }
}

/// Adds advisory findings for machine codes outside the preflight's explicitly reviewed common-code set.
fn warn_unverified_codes(
    path: &Toolpath,
    backend: BackendType,
    findings: &mut Vec<PreflightFinding>,
) {
    let common_g_codes = [
        0, 1, 2, 3, 4, 17, 20, 21, 28, 30, 40, 49, 53, 54, 55, 56, 57, 58, 59, 80, 90, 91, 94,
    ];
    let common_m_codes = [0, 1, 2, 3, 4, 5, 7, 8, 9, 30];
    for frame in &path.codes {
        for code in &frame.codes {
            let verified = match code {
                Code::G(number) => {
                    common_g_codes.contains(number)
                        || matches!(
                            (backend, *number),
                            (BackendType::Grbl, 38) | (BackendType::FluidNc, 38)
                        )
                }
                Code::M(number) => common_m_codes.contains(number),
                Code::Other(token) => token.starts_with('G') || token.starts_with('M'),
            };
            if !verified {
                findings.push(PreflightFinding {
                    severity: PreflightSeverity::Warning,
                    code: "code_capability_unknown".into(),
                    line: Some(frame.line),
                    detail: format!("{} capability for {} is not verified by this static preflight; review the controller documentation", backend.label(), code.canonical()),
                });
            }
        }
        if frame
            .codes
            .iter()
            .any(|code| matches!(code, Code::G(2 | 3)))
        {
            findings.push(PreflightFinding {
                severity: PreflightSeverity::Warning,
                code: "arc_bounds_approximate".into(),
                line: Some(frame.line),
                detail:
                    "travel check currently inspects arc endpoints, not the full swept arc envelope"
                        .into(),
            });
        }
    }
}

impl AppState {
    /// Runs source-level capability, travel-bound and tool-registry checks without contacting the machine.
    pub(crate) fn preflight_program(
        &self,
        source: &str,
        profile_id: Option<&str>,
    ) -> Result<PreflightReport, String> {
        let backend = self.rosetta_backend();
        let bridge = self.transform_bridge_for_preflight()?;
        let path = rosetta::io::gcode::parse_toolpath(source).map_err(|error| error.to_string())?;
        let mut findings = Vec::new();
        warn_unverified_codes(&path, backend, &mut findings);
        for finding in bridge.matrix.scan_incompatibilities(&path, backend) {
            let severity = match finding.severity {
                rosetta::ir::capability::Severity::Blocker => PreflightSeverity::Blocker,
                rosetta::ir::capability::Severity::Warning => PreflightSeverity::Warning,
                rosetta::ir::capability::Severity::Info => PreflightSeverity::Info,
            };
            findings.push(PreflightFinding {
                severity,
                code: "capability_finding".into(),
                line: Some(finding.line),
                detail: finding.detail,
            });
        }
        if !matches!(
            backend,
            BackendType::Klipper | BackendType::LinuxCnc | BackendType::Emulator
        ) {
            findings.push(PreflightFinding {
                severity: PreflightSeverity::Warning,
                code: "backend_capabilities_unverified".into(),
                line: None,
                detail: format!("{} probing, individual WCS support, and recovery semantics are not fully verified by this preflight", backend.label()),
            });
        }
        if backend == BackendType::Klipper
            && path.codes.iter().any(|frame| {
                frame
                    .codes
                    .iter()
                    .any(|code| matches!(code, Code::G(55 | 56 | 57 | 58 | 59)))
            })
        {
            findings.push(PreflightFinding {
                severity: PreflightSeverity::Blocker,
                code: "unsupported_wcs".into(),
                line: path.codes.iter().find(|frame| frame.codes.iter().any(|code| matches!(code, Code::G(55 | 56 | 57 | 58 | 59)))).map(|frame| frame.line),
                detail: "Klipper adapter exposes one coordinate offset; this program requests multiple G54-G59 systems".into(),
            });
        }
        let profile = if let Some(id) = profile_id {
            let profiles = self.list_machine_profiles()?;
            Some(
                profiles
                    .into_iter()
                    .find(|profile| profile.id == id)
                    .ok_or_else(|| "selected machine profile does not exist".to_string())?,
            )
        } else {
            None
        };
        let tools = self.list_tools()?;
        check_tools(&path, &tools, &mut findings);
        if let Some(profile) = &profile {
            match profile.tool_reference.as_deref() {
                Some(reference) if tools.iter().any(|tool| tool.id == reference) => findings.push(PreflightFinding { severity: PreflightSeverity::Info, code: "profile_tool_reference_found".into(), line: None, detail: format!("profile tool reference `{reference}` resolves to local tool metadata; no tool-selection command is sent") }),
                Some(reference) => findings.push(PreflightFinding { severity: PreflightSeverity::Warning, code: "profile_tool_reference_missing".into(), line: None, detail: format!("profile tool reference `{reference}` has no matching local tool record") }),
                None => {},
            }
        }
        let controller_configuration = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?
            .get_status_snapshot()
            .machine_configuration;
        let configuration = Some(crate::web_server::machine_profiles::resolution::resolve(
            profile.as_ref(),
            controller_configuration.as_ref(),
            &tools,
        ));
        if let Some(snapshot) = &configuration {
            check_profile_bounds(
                &path,
                profile.as_ref().map(|item| item.name.as_str()),
                snapshot.units,
                &snapshot.axes,
                &mut findings,
            );
            for conflict in &snapshot.conflicts {
                findings.push(PreflightFinding {
                    severity: PreflightSeverity::Warning,
                    code: "profile_controller_conflict".into(),
                    line: None,
                    detail: conflict.clone(),
                });
            }
        }
        if matches!(backend, BackendType::Grbl | BackendType::FluidNc) {
            findings.push(PreflightFinding {
                severity: PreflightSeverity::Warning,
                code: "backend_capabilities_partially_verified".into(),
                line: None,
                detail: format!("{} capability data is incomplete; static findings do not prove firmware execution support", backend.label()),
            });
        }
        let blocked = findings
            .iter()
            .any(|finding| matches!(finding.severity, PreflightSeverity::Blocker));
        let selected_profile_id = profile.map(|profile| profile.id);
        let fingerprint = report_fingerprint(source, selected_profile_id.as_deref(), &findings);
        Ok(PreflightReport {
            backend: backend.label().to_owned(),
            profile_id: selected_profile_id,
            fingerprint,
            blocked,
            findings,
            configuration,
        })
    }

    /// Builds the same capability matrix, including persisted resolution presets, as the Rosetta validator.
    fn transform_bridge_for_preflight(&self) -> Result<PreflightTransformBridge, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "settings database lock poisoned".to_string())?;
        Ok(PreflightTransformBridge {
            matrix: crate::rosetta_host::registry::matrix_with_presets(&connection),
        })
    }
}

/// Narrow wrapper carrying the exact capability matrix shared with Rosetta validation.
struct PreflightTransformBridge {
    /// Matrix returned by the application's established registry builder.
    matrix: rosetta::ir::capability::CapabilityMatrix,
}

/// Hashes the exact source and all deterministic findings to prevent stale UI confirmation.
fn report_fingerprint(
    source: &str,
    profile_id: Option<&str>,
    findings: &[PreflightFinding],
) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    profile_id.hash(&mut hasher);
    for finding in findings {
        finding.code.hash(&mut hasher);
        finding.line.hash(&mut hasher);
        finding.detail.hash(&mut hasher);
        match finding.severity {
            PreflightSeverity::Blocker => 0_u8,
            PreflightSeverity::Warning => 1_u8,
            PreflightSeverity::Info => 2_u8,
        }
        .hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests;
