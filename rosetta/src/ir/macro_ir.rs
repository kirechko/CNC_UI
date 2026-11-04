//! Macro IR — the single source of truth for authoring procedures.
//!
//! Implements contracts `CORE-M-01` (the data schema itself) and provides the
//! serde shape used by the declarative YAML frontend (`CORE-M-02`). The Macro
//! Engine (`rosetta::macro_engine`) treats [`MacroProgram`] as its only
//! representation (`MAC-C-01`): frontends produce it, emitters consume it, and
//! nothing else is ever stored.
//!
//! # Deviations from the ТЗ sketches
//!
//! * `triggers` lives on [`MacroProgram`] (not in the short §3.1 sketch) so
//!   that the YAML example of ТЗ №2 §4 round-trips through the IR unchanged.
//! * [`Trigger`] is an internally tagged enum (`type: button` …) instead of a
//!   flat struct with optional fields: every trigger kind gets exactly the
//!   fields it needs and typos are caught at parse time.
//! * `MacroId`/`TransformId` are plain `String` aliases. Newtypes would add
//!   friction for serde and cross-module calls without any type-safety win
//!   that matters in a single-crate system.

// The child modules group the schema by responsibility; public re-exports keep
// all established `ir::macro_ir::*` item paths stable.
mod expr;
mod program;
mod stmt;
mod trigger;
mod types;

pub use expr::{BinOp, Expr, UnOp};
pub use program::MacroProgram;
pub use stmt::Stmt;
pub use trigger::Trigger;
pub use types::{
    MacroClass, MacroId, MacroMeta, ParamDecl, Subroutine, TransformId, Value, ValueKind, VarDecl,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact YAML example of ТЗ №2 §4 must parse into the IR.
    #[test]
    fn spec_trigger_yaml_parses() {
        // serde_yaml_ng renders externally tagged Stmt/Expr as YAML tags
        // (see the serialization-shapes note on `Stmt`); triggers keep the
        // spec's `{ type: ... }` internal-tag map shape.
        let yaml = r#"
meta: { id: tool_change, name: "Tool change" }
body:
  - !gcode { template: "G91 G38.2 Z-50 F100" }
  - !wait { cond: !status probe_trig }
triggers:
  - { type: button, require_confirm: true }
  - { type: mcode, code: 6 }
  - { type: hotkey, keys: "Ctrl+T" }
"#;
        let program: MacroProgram = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(program.meta.id, "tool_change");
        assert_eq!(program.body.len(), 2);
        assert_eq!(program.triggers.len(), 3);
        assert!(program.triggers[1].matches_mcode(6));
        assert!(program.triggers[0].requires_confirm());
    }

    #[test]
    fn yaml_roundtrip_preserves_ir() {
        let program = MacroProgram {
            meta: MacroMeta {
                id: "drill_grid".into(),
                name: "Drill grid".into(),
                icon: None,
                category: Some("machining".into()),
                class: MacroClass::Generative,
                require_confirm: false,
            },
            params: vec![ParamDecl {
                name: "rows".into(),
                kind: ValueKind::Number,
                default: Some(Expr::Num(3.0)),
                description: None,
            }],
            locals: vec![],
            subs: vec![Subroutine {
                name: "peck".into(),
                params: vec![ParamDecl {
                    name: "depth".into(),
                    kind: ValueKind::Number,
                    default: None,
                    description: None,
                }],
                description: None,
                body: vec![Stmt::GCode {
                    template: "G1 Z-{depth}".into(),
                }],
            }],
            triggers: vec![Trigger::Button {
                require_confirm: true,
            }],
            body: vec![Stmt::ForEach {
                var: "r".into(),
                iter: Expr::Range {
                    start: Box::new(Expr::Num(0.0)),
                    end: Box::new(Expr::Var("rows".into())),
                    step: None,
                },
                body: vec![Stmt::Call {
                    name: "peck".into(),
                    args: vec![Expr::Num(-5.0)],
                }],
            }],
        };
        let text = serde_yaml_ng::to_string(&program).unwrap();
        let back: MacroProgram = serde_yaml_ng::from_str(&text).unwrap();
        assert_eq!(program, back);
    }

    #[test]
    fn validate_rejects_duplicate_names() {
        let mut program = MacroProgram {
            meta: MacroMeta {
                id: "x".into(),
                name: "x".into(),
                icon: None,
                category: None,
                class: MacroClass::default(),
                require_confirm: false,
            },
            params: vec![
                ParamDecl {
                    name: "a".into(),
                    kind: ValueKind::Number,
                    default: None,
                    description: None,
                },
                ParamDecl {
                    name: "a".into(),
                    kind: ValueKind::Number,
                    default: None,
                    description: None,
                },
            ],
            locals: vec![],
            subs: vec![],
            triggers: vec![],
            body: vec![],
        };
        assert!(program.validate().is_err());
        program.params.pop();
        assert!(program.validate().is_ok());
    }

    #[test]
    fn calls_are_collected_recursively() {
        let stmts = vec![Stmt::If {
            cond: Expr::Bool(true),
            then: vec![Stmt::Call {
                name: "peck".into(),
                args: vec![],
            }],
            otherwise: vec![Stmt::While {
                cond: Expr::Bool(false),
                body: vec![Stmt::Call {
                    name: "home".into(),
                    args: vec![],
                }],
            }],
        }];
        let program = MacroProgram {
            meta: MacroMeta {
                id: "m".into(),
                name: "m".into(),
                icon: None,
                category: None,
                class: MacroClass::default(),
                require_confirm: false,
            },
            params: vec![],
            locals: vec![],
            subs: vec![],
            triggers: vec![],
            body: stmts,
        };
        let mut calls = program.called_names();
        calls.sort();
        assert_eq!(calls, vec!["home", "peck"]);
    }
}
