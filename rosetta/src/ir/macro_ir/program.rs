use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{MacroMeta, ParamDecl, Stmt, Subroutine, Trigger, VarDecl};

/// A complete macro program — the root of the Macro IR (`CORE-M-01`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacroProgram {
    pub meta: MacroMeta,
    #[serde(default)]
    pub params: Vec<ParamDecl>,
    #[serde(default)]
    pub locals: Vec<VarDecl>,
    #[serde(default)]
    pub subs: Vec<Subroutine>,
    /// Launch triggers (deviation: kept on the root so YAML round-trips).
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    pub body: Vec<Stmt>,
}

impl MacroProgram {
    /// Structural self-validation used by the YAML frontend right after
    /// deserialization. It checks the locally decidable rules only
    /// (empty id, duplicate names); cross-object reference checks live in
    /// `macro_engine::registry::MacroRegistry::validate_references`
    /// (`CORE-M-05`).
    pub fn validate(&self) -> Result<(), crate::error::RosettaError> {
        // Duplicate names across params/locals/subs are validation errors.
        let mut names = BTreeSet::new();
        let mut all: Vec<&str> = Vec::new();
        all.extend(self.params.iter().map(|p| p.name.as_str()));
        all.extend(self.locals.iter().map(|v| v.name.as_str()));
        all.extend(self.subs.iter().map(|s| s.name.as_str()));
        for name in &all {
            if !names.insert(*name) {
                return Err(crate::error::RosettaError::Validation {
                    reason: format!("duplicate name \"{name}\" in params/locals/subs"),
                });
            }
        }
        let mut sub_names = BTreeSet::new();
        for sub in &self.subs {
            if !sub_names.insert(sub.name.as_str()) {
                return Err(crate::error::RosettaError::Validation {
                    reason: format!("duplicate subroutine name \"{}\"", sub.name),
                });
            }
        }
        if self.meta.id.is_empty() {
            return Err(crate::error::RosettaError::Validation {
                reason: "macro id must not be empty".to_owned(),
            });
        }
        Ok(())
    }

    /// Convenience: look up a subroutine by name (`CORE-M-05` resolution).
    pub fn find_sub(&self, name: &str) -> Option<&Subroutine> {
        self.subs.iter().find(|s| s.name == name)
    }

    /// Collect the names of every `Stmt::Call` target in the program body
    /// and all subs — the core's static reference scan used by
    /// `validate_references` (`CORE-M-05`).
    pub fn called_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        for stmt in &self.body {
            collect_calls(stmt, &mut out);
        }
        for sub in &self.subs {
            for stmt in &sub.body {
                collect_calls(stmt, &mut out);
            }
        }
        out
    }
}

/// Internal: recursive `Stmt::Call` scanner used by [`MacroProgram::called_names`].
fn collect_calls(stmt: &Stmt, out: &mut Vec<String>) {
    match stmt {
        Stmt::Call { name, .. } => out.push(name.clone()),
        Stmt::If {
            then, otherwise, ..
        } => {
            for s in then.iter().chain(otherwise) {
                collect_calls(s, out);
            }
        }
        Stmt::While { body, .. } | Stmt::Repeat { body, .. } | Stmt::ForEach { body, .. } => {
            for s in body {
                collect_calls(s, out);
            }
        }
        _ => {}
    }
}
