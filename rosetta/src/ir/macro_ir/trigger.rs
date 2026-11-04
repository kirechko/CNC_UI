use serde::{Deserialize, Serialize};

/// Launch trigger of a macro (ТЗ №2 §4). Internally tagged by `type` exactly
/// as in the specification YAML example.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Trigger {
    /// UI tile press; may demand a confirmation modal.
    Button {
        #[serde(default)]
        require_confirm: bool,
    },
    /// Keyboard chord such as `"Ctrl+Shift+Z"`.
    Hotkey {
        keys: String,
        #[serde(default)]
        require_confirm: bool,
    },
    /// Intercept of an M-code while the program/macro streams (`M6`).
    MCode {
        code: u16,
        #[serde(default)]
        require_confirm: bool,
    },
    /// Machine event such as `on_homed` / `on_probe_done`.
    Event {
        name: String,
        #[serde(default)]
        require_confirm: bool,
    },
}

impl Trigger {
    /// Whether this trigger fires on a program M-code equal to `code`.
    pub fn matches_mcode(&self, code: u16) -> bool {
        matches!(self, Trigger::MCode { code: c, .. } if *c == code)
    }

    /// Whether this trigger fires on the machine event `name`
    /// (case-sensitive, spec uses `on_homed` style).
    pub fn matches_event(&self, name: &str) -> bool {
        matches!(self, Trigger::Event { name: n, .. } if n == name)
    }

    /// Safety: the engine must ask for confirmation before running through
    /// this trigger.
    pub fn requires_confirm(&self) -> bool {
        match self {
            Trigger::Button { require_confirm } => *require_confirm,
            Trigger::Hotkey {
                require_confirm, ..
            } => *require_confirm,
            Trigger::MCode {
                require_confirm, ..
            } => *require_confirm,
            Trigger::Event {
                require_confirm, ..
            } => *require_confirm,
        }
    }
}
