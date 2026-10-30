//! The transform catalog (ТЗ №3 §9.2, contracts `TRF-M-03`/`TRF-M-04`).
//!
//! The registry answers two questions for the rest of the system:
//!
//! 1. *Validation* (`TRF-M-04`): the macro engine calls
//!    [`TransformRegistry::exists`] to check every `Stmt::InvokeTransform`
//!    id at load time (`CORE-M-05` / `MAC-T-04`).
//! 2. *Invocation* (`TRF-M-03`): [`TransformRegistry::build_from_args`] turns
//!    a positional argument list from a macro (`Rotate(90, 120, 40)`) into a
//!    concrete [`TransformKind`], which the [`crate::transform::pipeline`]
//!    can then apply.
//!
//! The stable ids are exactly the strings stored inside the core's
//! `Resolution::ApplyTransform` rules (`"Rotate"`, `"CutterCompBake"` …), so
//! a resolution rule names a real catalog entry without any module cycle.

use std::collections::BTreeMap;

use crate::error::{RosettaError, RosettaResult};
use crate::ir::macro_ir::{TransformId, Value};
use crate::ir::toolpath::{Code, Point2, Point3, Units};
use crate::transform::model::{
    CompSide, CoordSystem, CycleCode, LeadKind, LeadSpec, MirrorAxes, TransformFamily,
    TransformKind,
};

/// One catalog entry: metadata plus the argument decoder (`TRF-M-03`).
#[derive(Clone)]
pub struct TransformSpec {
    /// Stable id == serde variant name == resolution-rule string.
    pub id: TransformId,
    /// Family shown in the UI ("аффинная / геометрическая / структурная").
    pub family: TransformFamily,
    /// One-line human description for the transform picker.
    pub description: &'static str,
    /// Decode positional macro/CLI arguments into the concrete kind.
    /// Fails with `Validation` naming the expected shape on mismatch.
    pub build: fn(args: &[Value]) -> RosettaResult<TransformKind>,
}

impl std::fmt::Debug for TransformSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformSpec")
            .field("id", &self.id)
            .field("family", &self.family)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

/// Compact listing row (`TransformRegistry::list` in the ТЗ sketch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformRef {
    pub id: TransformId,
    pub family: TransformFamily,
    pub description: &'static str,
}

/// The `TRF-M-04` catalog. `Sync`-safe by construction: entries are static
/// metadata with function-pointer builders.
#[derive(Debug, Default, Clone)]
pub struct TransformRegistry {
    specs: BTreeMap<TransformId, TransformSpec>,
}

impl TransformRegistry {
    /// Empty catalog.
    pub fn new() -> Self {
        TransformRegistry { specs: BTreeMap::new() }
    }

    /// The built-in catalog with every kind of ТЗ №3 §4 registered; the
    /// geometric family is present but its executor still refuses until
    /// Phase D (see [`crate::transform::structural::GeometricStub`]).
    pub fn with_builtins() -> Self {
        let mut registry = TransformRegistry::new();
        for spec in builtin_specs() {
            registry.specs.insert(spec.id.clone(), spec);
        }
        registry
    }

    /// Register (or replace) one spec — embedders add machine-specific ops.
    pub fn register(&mut self, spec: TransformSpec) {
        self.specs.insert(spec.id.clone(), spec);
    }

    /// Look up one spec by id.
    pub fn get(&self, id: &str) -> Option<&TransformSpec> {
        self.specs.get(id)
    }

    /// `MAC-T-04`/`CORE-M-05` link check for `Stmt::InvokeTransform`.
    pub fn exists(&self, id: &str) -> bool {
        self.specs.contains_key(id)
    }

    /// All entries in id order.
    pub fn list(&self) -> Vec<TransformRef> {
        self.specs
            .values()
            .map(|s| TransformRef {
                id: s.id.clone(),
                family: s.family,
                description: s.description,
            })
            .collect()
    }

    /// Decode args into a kind and validate the id exists (`TRF-M-03`).
    pub fn build_from_args(&self, id: &str, args: &[Value]) -> RosettaResult<TransformKind> {
        let spec = self
            .get(id)
            .ok_or_else(|| RosettaError::NotFound { kind: "transform".into(), id: id.into() })?;
        (spec.build)(args)
    }
}

/// The built-in specs of the default catalog.
fn builtin_specs() -> Vec<TransformSpec> {
    use TransformKind as K;
    vec![
        TransformSpec {
            id: "Rotate".into(),
            family: TransformFamily::Affine,
            description: "повернуть траекторию вокруг центра (angle, cx, cy)",
            build: |args| {
                let angle = number(args, 0, "Rotate.angle_deg")?;
                let center = Point2::new(number(args, 1, "Rotate.center_x")?, number(args, 2, "Rotate.center_y")?);
                Ok(K::Rotate { angle_deg: angle, center })
            },
        },
        TransformSpec {
            id: "Translate".into(),
            family: TransformFamily::Affine,
            description: "сместить траекторию (dx, dy, dz=0)",
            build: |args| {
                let delta = Point3::new(
                    number(args, 0, "Translate.dx")?,
                    number(args, 1, "Translate.dy")?,
                    optional_number(args, 2).unwrap_or(0.0),
                );
                Ok(K::Translate { delta })
            },
        },
        TransformSpec {
            id: "Scale".into(),
            family: TransformFamily::Affine,
            description: "масштаб (sx, sy, sz, origin_x=0, origin_y=0, origin_z=0)",
            build: |args| {
                let factors = Point3::new(
                    number(args, 0, "Scale.sx")?,
                    number(args, 1, "Scale.sy")?,
                    number(args, 2, "Scale.sz")?,
                );
                let origin = Point3::new(
                    optional_number(args, 3).unwrap_or(0.0),
                    optional_number(args, 4).unwrap_or(0.0),
                    optional_number(args, 5).unwrap_or(0.0),
                );
                Ok(K::Scale { factors, origin })
            },
        },
        TransformSpec {
            id: "Mirror".into(),
            family: TransformFamily::Affine,
            description: "зеркало по осям (\"x\"|\"y\"|\"xy\")",
            build: |args| {
                let axes = match text(args, 0, "Mirror.axes")?.to_ascii_lowercase().as_str() {
                    "x" => MirrorAxes::X,
                    "y" => MirrorAxes::Y,
                    "xy" | "yx" => MirrorAxes::Xy,
                    other => {
                        return Err(RosettaError::Validation {
                            reason: format!("Mirror.axes must be \"x\", \"y\" or \"xy\", got \"{other}\""),
                        })
                    }
                };
                Ok(K::Mirror { axes })
            },
        },
        TransformSpec {
            id: "UnitsConvert".into(),
            family: TransformFamily::Affine,
            description: "смена единиц (\"mm\"|\"in\", \"mm\"|\"in\")",
            build: |args| {
                let from = parse_units(text(args, 0, "UnitsConvert.from")?)?;
                let to = parse_units(text(args, 1, "UnitsConvert.to")?)?;
                Ok(K::UnitsConvert { from, to })
            },
        },
        TransformSpec {
            id: "FeedSpeedOverride".into(),
            family: TransformFamily::Structural,
            description: "множитель подач и оборотов (feed_factor, speed_factor)",
            build: |args| {
                Ok(K::FeedSpeedOverride {
                    feed_factor: number(args, 0, "FeedSpeedOverride.feed_factor")?,
                    speed_factor: number(args, 1, "FeedSpeedOverride.speed_factor")?,
                })
            },
        },
        TransformSpec {
            id: "CoordSystemRemap".into(),
            family: TransformFamily::Structural,
            description: "перевязка СК станка (54..59 → 54..59)",
            build: |args| {
                let from = parse_coord(number(args, 0, "CoordSystemRemap.from")?)?;
                let to = parse_coord(number(args, 1, "CoordSystemRemap.to")?)?;
                Ok(K::CoordSystemRemap { from, to })
            },
        },
        TransformSpec {
            id: "CycleSubstitute".into(),
            family: TransformFamily::Structural,
            description: "заменить цикл макросом-эмулятором (81..86/73/76, macro_id)",
            build: |args| {
                let cycle = CycleCode::from_code(&Code::G(number(args, 0, "CycleSubstitute.cycle")? as u16))
                    .ok_or_else(|| RosettaError::Validation {
                        reason: "CycleSubstitute.cycle must be 73/76/81..86".into(),
                    })?;
                Ok(K::CycleSubstitute { from: cycle, to: text(args, 1, "CycleSubstitute.macro_id")? })
            },
        },
        TransformSpec {
            id: "StripUnsupported".into(),
            family: TransformFamily::Structural,
            description: "вырезать коды (\"G41\", \"M6\", ...)",
            build: |args| {
                if args.is_empty() {
                    return Err(RosettaError::Validation {
                        reason: "StripUnsupported needs at least one code".into(),
                    });
                }
                let codes = args
                    .iter()
                    .map(|value| {
                        let token = value.to_catalog_token();
                        Code::parse(&token).ok_or_else(|| RosettaError::Validation {
                            reason: format!("StripUnsupported code \"{token}\" is not a G/M word"),
                        })
                    })
                    .collect::<RosettaResult<Vec<_>>>()?;
                Ok(K::StripUnsupported { codes })
            },
        },
        // Deferred geometric family (Phase D): ids exist for validation and
        // for the roadmap UI; only argument decoding is provided today.
        TransformSpec {
            id: "CutterCompBake".into(),
            family: TransformFamily::Geometric,
            description: "запечь G41/G42 (\"left\"|\"right\", tool_radius [, lead_len])",
            build: |args| {
                let side = match text(args, 0, "CutterCompBake.side")?.to_ascii_lowercase().as_str() {
                    "left" | "g41" => CompSide::Left,
                    "right" | "g42" => CompSide::Right,
                    other => {
                        return Err(RosettaError::Validation {
                            reason: format!("CutterCompBake.side must be left/right, got \"{other}\""),
                        })
                    }
                };
                let lead = optional_number(args, 2).map(|length| LeadSpec {
                    kind: LeadKind::Linear,
                    length,
                    feed_factor: None,
                });
                Ok(K::CutterCompBake { side, tool_radius: number(args, 1, "CutterCompBake.tool_radius")?, lead })
            },
        },
        TransformSpec {
            id: "LeadInOut".into(),
            family: TransformFamily::Geometric,
            description: "врезки/выезды (\"linear\"|\"arc\", length)",
            build: |args| {
                let kind = match text(args, 0, "LeadInOut.kind")?.to_ascii_lowercase().as_str() {
                    "linear" => LeadKind::Linear,
                    "arc" => LeadKind::Arc,
                    other => {
                        return Err(RosettaError::Validation {
                            reason: format!("LeadInOut.kind must be linear/arc, got \"{other}\""),
                        })
                    }
                };
                Ok(K::LeadInOut {
                    spec: LeadSpec { kind, length: number(args, 1, "LeadInOut.length")?, feed_factor: None },
                })
            },
        },
        TransformSpec {
            id: "CornerRound".into(),
            family: TransformFamily::Geometric,
            description: "скругление острых углов (radius)",
            build: |args| Ok(K::CornerRound { radius: number(args, 0, "CornerRound.radius")? }),
        },
    ]
}

/// Positional required number argument.
fn number(args: &[Value], index: usize, what: &str) -> RosettaResult<f64> {
    args.get(index).map(Value::as_num).ok_or_else(|| RosettaError::Validation {
        reason: format!("{what}: missing numeric argument #{index}"),
    })
}

/// Positional optional number argument.
fn optional_number(args: &[Value], index: usize) -> Option<f64> {
    args.get(index).map(Value::as_num)
}

/// Positional required text argument (accepts any value via display).
fn text(args: &[Value], index: usize, what: &str) -> RosettaResult<String> {
    args.get(index)
        .map(|value| match value {
            Value::Str(text) => text.clone(),
            other => other.to_catalog_token(),
        })
        .ok_or_else(|| RosettaError::Validation {
            reason: format!("{what}: missing argument #{index}"),
        })
}

/// `"mm"|"in"|"millimeters"|"inches"` → [`Units`].
fn parse_units(token: String) -> RosettaResult<Units> {
    match token.to_ascii_lowercase().as_str() {
        "mm" | "millimeters" | "millimeter" | "metric" => Ok(Units::Millimeters),
        "in" | "inches" | "inch" | "imperial" => Ok(Units::Inches),
        other => Err(RosettaError::Validation {
            reason: format!("units must be mm/in, got \"{other}\""),
        }),
    }
}

/// 54..59 → [`CoordSystem`].
fn parse_coord(n: f64) -> RosettaResult<CoordSystem> {
    match n as u16 {
        54 => Ok(CoordSystem::G54),
        55 => Ok(CoordSystem::G55),
        56 => Ok(CoordSystem::G56),
        57 => Ok(CoordSystem::G57),
        58 => Ok(CoordSystem::G58),
        59 => Ok(CoordSystem::G59),
        other => Err(RosettaError::Validation {
            reason: format!("work coordinate system must be 54..59, got {other}"),
        }),
    }
}

/// Small display helper so `Value` can serve as an argument token without
/// pulling the interpreter's private trait into this module.
trait ToCatalogToken {
    fn to_catalog_token(&self) -> String;
}

impl ToCatalogToken for Value {
    fn to_catalog_token(&self) -> String {
        match self {
            Value::Str(text) => text.clone(),
            Value::Bool(flag) => (if *flag { "true" } else { "false" }).to_owned(),
            Value::Num(n) => crate::util::fmt_num(*n),
            Value::Array(items) => items
                .iter()
                .map(ToCatalogToken::to_catalog_token)
                .collect::<Vec<_>>()
                .join(","),
            Value::Null => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_cover_resolution_rule_names() {
        let registry = TransformRegistry::with_builtins();
        for id in ["Rotate", "Translate", "Scale", "Mirror", "UnitsConvert", "CutterCompBake"] {
            assert!(registry.exists(id), "{id} missing");
        }
        assert!(!registry.exists("Teleport"));
        // list() is sorted and complete (12 kinds of ТЗ §4 minus Trochoidal).
        assert_eq!(registry.list().len(), 12);
    }

    #[test]
    fn build_rotate_from_positional_args() {
        let registry = TransformRegistry::with_builtins();
        let args = vec![Value::Num(90.0), Value::Num(120.0), Value::Num(40.0)];
        match registry.build_from_args("Rotate", &args).unwrap() {
            TransformKind::Rotate { angle_deg, center } => {
                assert_eq!(angle_deg, 90.0);
                assert_eq!(center, Point2::new(120.0, 40.0));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn missing_or_bad_args_validate() {
        let registry = TransformRegistry::with_builtins();
        assert!(matches!(
            registry.build_from_args("Rotate", &[Value::Num(1.0)]),
            Err(RosettaError::Validation { .. })
        ));
        assert!(matches!(
            registry.build_from_args("Mirror", &[Value::Str("q".into())]),
            Err(RosettaError::Validation { .. })
        ));
        assert!(matches!(
            registry.build_from_args("Nope", &[]),
            Err(RosettaError::NotFound { .. })
        ));
    }

    #[test]
    fn strip_codes_decode_from_strings() {
        let registry = TransformRegistry::with_builtins();
        let args = vec![Value::Str("G41".into()), Value::Str("M6".into())];
        match registry.build_from_args("StripUnsupported", &args).unwrap() {
            TransformKind::StripUnsupported { codes } => {
                assert_eq!(codes, vec![Code::G(41), Code::M(6)]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
