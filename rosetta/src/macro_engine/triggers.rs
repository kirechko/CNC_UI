//! Launch triggers (ТЗ №2 §4): the mapping from a UI/system event to the
//! macro that must run. The engine only *matches* events against the
//! [`Trigger`] list stored on each program; delivery (wiring buttons,
//! hotkeys, M-code interception, machine events) is the host application's
//! job, which keeps this crate UI-agnostic.

use crate::ir::macro_ir::Trigger;

/// A concrete event the host observed and wants to route through the macro
/// catalog (the runtime mirror of the [`Trigger`] variants).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerEvent {
    /// A macro tile was pressed in the UI.
    UiClick,
    /// A keyboard chord fired; `keys` uses the same spelling as the trigger
    /// definition, e.g. `"Ctrl+Shift+Z"` (comparison is case-sensitive).
    Hotkey(String),
    /// The streamed program contained `M<code>` and M-code routing is on.
    MCode(u16),
    /// A named machine event such as `"on_homed"` or `"on_probe_done"`.
    MachineEvent(String),
}

impl TriggerEvent {
    /// Whether `trigger` fires for this event (`MAC-C-05` routing input).
    pub fn matches(&self, trigger: &Trigger) -> bool {
        match (self, trigger) {
            (TriggerEvent::UiClick, Trigger::Button { .. }) => true,
            (TriggerEvent::Hotkey(keys), Trigger::Hotkey { keys: want, .. }) => keys == want,
            (TriggerEvent::MCode(code), trigger) => trigger.matches_mcode(*code),
            (TriggerEvent::MachineEvent(name), trigger) => trigger.matches_event(name),
            _ => false,
        }
    }
}

/// Find the first trigger in `triggers` matching `event`.
///
/// Host usage: for each registered program call
/// `find_trigger(&program.triggers, &event)`; a `Some` result means the
/// program should run, and [`Trigger::requires_confirm`] (or
/// `meta.require_confirm`) decides whether the confirmation modal appears
/// first (ТЗ №2 §9.3).
pub fn find_trigger<'t>(triggers: &'t [Trigger], event: &TriggerEvent) -> Option<&'t Trigger> {
    triggers.iter().find(|trigger| event.matches(trigger))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_matching_is_exact_and_case_sensitive() {
        let triggers = vec![Trigger::Hotkey { keys: "Ctrl+Shift+Z".into(), require_confirm: true }];
        assert!(find_trigger(&triggers, &TriggerEvent::Hotkey("Ctrl+Shift+Z".into())).is_some());
        assert!(find_trigger(&triggers, &TriggerEvent::Hotkey("ctrl+shift+z".into())).is_none());
        let found = find_trigger(&triggers, &TriggerEvent::Hotkey("Ctrl+Shift+Z".into())).unwrap();
        assert!(found.requires_confirm());
    }

    #[test]
    fn mcode_and_event_triggers_route_correctly() {
        let triggers = vec![
            Trigger::MCode { code: 6, require_confirm: true },
            Trigger::Event { name: "on_homed".into(), require_confirm: false },
        ];
        assert!(matches!(
            find_trigger(&triggers, &TriggerEvent::MCode(6)),
            Some(Trigger::MCode { code: 6, .. })
        ));
        assert!(find_trigger(&triggers, &TriggerEvent::MCode(7)).is_none());
        assert!(find_trigger(&triggers, &TriggerEvent::MachineEvent("on_homed".into())).is_some());
        // A UI click matches no trigger here.
        assert!(find_trigger(&triggers, &TriggerEvent::UiClick).is_none());
    }
}
