//! The macro registry (`CORE-M-06` / `MAC-C-06`): the single catalog of all
//! macro programs the engine can run, plus the cross-object reference
//! validation the core requires from the engine (`CORE-M-05`).
//!
//! Programs are stored by id; ids are unique (registering a duplicate is a
//! validation error, not a silent overwrite — macros are user data). The
//! registry is the answer to the core-side question "run macro X with these
//! arguments": [`crate::macro_engine::Invoker`] resolves ids here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{RosettaError, RosettaResult};
use crate::ir::macro_ir::{MacroClass, MacroId, MacroProgram};

/// Compact, UI-facing descriptor of one registered macro (`MAC-C-06`: the
/// panel lists macros without cloning whole programs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroRef {
    /// Stable program id.
    pub id: MacroId,
    /// Human-readable name.
    pub name: String,
    /// Generative vs reactive (routing decision, ТЗ №2 §5).
    pub class: MacroClass,
    /// Optional icon token for the UI tile.
    #[serde(default)]
    pub icon: Option<String>,
    /// Optional grouping category.
    #[serde(default)]
    pub category: Option<String>,
}

/// Ordered catalog of macro programs.
#[derive(Debug, Default, Clone)]
pub struct MacroRegistry {
    /// Id → program. `BTreeMap` keeps [`MacroRegistry::list`] deterministic.
    programs: BTreeMap<MacroId, MacroProgram>,
}

impl MacroRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        MacroRegistry { programs: BTreeMap::new() }
    }

    /// Insert `program`, running the locally decidable checks first
    /// (`CORE-M-02`) and refusing duplicate ids (`MAC-C-06`: macros are
    /// user-authored data; silent overwrite would lose work).
    pub fn register(&mut self, program: MacroProgram) -> RosettaResult<()> {
        program.validate()?;
        // Reject duplicates *before* touching the map so a failed
        // registration never mutates the catalog.
        if self.programs.contains_key(&program.meta.id) {
            return Err(RosettaError::Validation {
                reason: format!("macro id \"{}\" is already registered", program.meta.id),
            });
        }
        self.programs.insert(program.meta.id.clone(), program);
        Ok(())
    }

    /// Replace the program stored under its own id (load-from-disk path), or
    /// insert it when absent.
    pub fn upsert(&mut self, program: MacroProgram) -> RosettaResult<()> {
        program.validate()?;
        self.programs.insert(program.meta.id.clone(), program);
        Ok(())
    }

    /// Look one program up by id.
    pub fn get(&self, id: &MacroId) -> Option<&MacroProgram> {
        self.programs.get(id)
    }

    /// Whether `id` is present.
    pub fn contains(&self, id: &MacroId) -> bool {
        self.programs.contains_key(id)
    }

    /// Remove and return one program.
    pub fn remove(&mut self, id: &MacroId) -> Option<MacroProgram> {
        self.programs.remove(id)
    }

    /// Number of registered programs.
    pub fn len(&self) -> usize {
        self.programs.len()
    }

    /// Whether the catalog is empty.
    pub fn is_empty(&self) -> bool {
        self.programs.is_empty()
    }

    /// All descriptors in id order (the macro panel data source).
    pub fn list(&self) -> Vec<MacroRef> {
        self.programs
            .values()
            .map(|p| MacroRef {
                id: p.meta.id.clone(),
                name: p.meta.name.clone(),
                class: p.meta.class,
                icon: p.meta.icon.clone(),
                category: p.meta.category.clone(),
            })
            .collect()
    }

    /// Cross-object validation (`CORE-M-05`): every reference that leaves a
    /// program must resolve. `Stmt::Call` targets must name a subroutine of
    /// the same program; `Stmt::InvokeTransform` ids must exist in the
    /// transform catalog, which the caller supplies as a predicate (keeps the
    /// engine free of a hard dependency on the transform module's types).
    /// Returns human-readable issues, empty when the catalog is consistent.
    pub fn validate_references(&self, transform_exists: &dyn Fn(&str) -> bool) -> Vec<String> {
        let mut issues = Vec::new();
        for program in self.programs.values() {
            // 1. Sub-call targets resolve within the same program.
            for name in program.called_names() {
                if program.find_sub(&name).is_none() {
                    issues.push(format!(
                        "macro \"{}\": call target \"{name}\" is not a subroutine of this program",
                        program.meta.id
                    ));
                }
            }
            // 2. Transform ids resolve against the host catalog.
            collect_transform_ids(&program.body, &mut |id| {
                if !transform_exists(id) {
                    issues.push(format!(
                        "macro \"{}\": unknown transform id \"{id}\"",
                        program.meta.id
                    ));
                }
            });
            for sub in &program.subs {
                collect_transform_ids(&sub.body, &mut |id| {
                    if !transform_exists(id) {
                        issues.push(format!(
                            "macro \"{}\", sub \"{}\": unknown transform id \"{id}\"",
                            program.meta.id, sub.name
                        ));
                    }
                });
            }
        }
        issues
    }
}

/// Internal: walk a statement block collecting every `InvokeTransform` id.
fn collect_transform_ids(block: &[crate::ir::macro_ir::Stmt], visit: &mut dyn FnMut(&str)) {
    use crate::ir::macro_ir::Stmt;
    for stmt in block {
        match stmt {
            Stmt::InvokeTransform { id, .. } => visit(id.as_str()),
            Stmt::If { then, otherwise, .. } => {
                collect_transform_ids(then, visit);
                collect_transform_ids(otherwise, visit);
            }
            Stmt::While { body, .. } | Stmt::Repeat { body, .. } | Stmt::ForEach { body, .. } => {
                collect_transform_ids(body, visit);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::macro_ir::MacroMeta;

    /// Minimal program with a given id.
    fn program(id: &str) -> MacroProgram {
        MacroProgram {
            meta: MacroMeta {
                id: id.into(),
                name: id.to_uppercase(),
                icon: None,
                category: None,
                class: MacroClass::Reactive,
                require_confirm: false,
            },
            params: Vec::new(),
            locals: Vec::new(),
            subs: Vec::new(),
            triggers: Vec::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn register_rejects_duplicate_ids() {
        let mut registry = MacroRegistry::new();
        registry.register(program("a")).unwrap();
        let error = registry.register(program("a")).unwrap_err();
        assert!(matches!(error, RosettaError::Validation { .. }));
        // The original entry survived the failed registration untouched.
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn list_exposes_descriptors_in_id_order() {
        let mut registry = MacroRegistry::new();
        registry.register(program("zulu")).unwrap();
        registry.register(program("alpha")).unwrap();
        // Bind first: `list()` returns an owned Vec of descriptors.
        let refs = registry.list();
        let ids: Vec<&str> = refs.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["alpha", "zulu"]);
    }

    #[test]
    fn validate_references_finds_missing_subs_and_transforms() {
        let mut registry = MacroRegistry::new();
        let mut p = program("m");
        p.body = vec![
            crate::ir::macro_ir::Stmt::Call { name: "ghost".into(), args: Vec::new() },
            crate::ir::macro_ir::Stmt::InvokeTransform { id: "Nope".into(), args: Vec::new() },
        ];
        registry.upsert(p).unwrap();
        let issues = registry.validate_references(&|id| id == "Rotate");
        assert_eq!(issues.len(), 2);
        assert!(issues.iter().any(|i| i.contains("ghost")));
        assert!(issues.iter().any(|i| i.contains("Nope")));
        // With the transform catalog aware of "Nope" only the sub remains.
        let issues = registry.validate_references(&|id| id == "Nope");
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("ghost"));
    }
}
