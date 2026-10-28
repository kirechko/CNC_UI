//! Tracks whether the controller is executing the loaded program.
//!
//! Traversed-path progress is meaningful only while a program runs, because a
//! tool that jogs near the path must not darken it. The flag turns on when a
//! `ProgramRun` command is accepted and turns off when the controller reports
//! the program finished (interpreter back to Idle after it was running), an
//! E-Stop or alarm, an abort, or a new program load.

/// Worker-side state machine deciding when toolpath progress is real.
#[derive(Debug, Default)]
pub(super) struct ProgramTracking {
    /// `true` from an accepted run command until the program stops.
    active: bool,
    /// `true` once the controller has reported a running or paused state for this run.
    seen_running: bool,
}

impl ProgramTracking {
    /// Marks the program as started by an accepted run command.
    pub(super) fn started(&mut self) {
        self.active = true;
        self.seen_running = false;
    }

    /// Marks the program as stopped by an abort or a new program load.
    pub(super) fn stopped(&mut self) {
        self.active = false;
        self.seen_running = false;
    }

    /// Updates the tracking state from the latest controller report.
    ///
    /// Idle is ignored until the controller has reported running, because a
    /// freshly accepted run command may still read as Idle on the first poll.
    pub(super) fn observe(
        &mut self,
        task_state: crate::cnc_api::TaskState,
        interpreter_state: crate::cnc_api::InterpreterState,
    ) {
        if !self.active {
            return;
        }
        if task_state == crate::cnc_api::TaskState::Estop {
            self.stopped();
            return;
        }
        match interpreter_state {
            crate::cnc_api::InterpreterState::Reading | crate::cnc_api::InterpreterState::Paused => {
                self.seen_running = true;
            }
            crate::cnc_api::InterpreterState::Idle if self.seen_running => self.stopped(),
            _ => {}
        }
    }

    /// Returns `true` while progress should be reported to the UI.
    pub(super) fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(test)]
mod program_tracking_tests {
    use super::ProgramTracking;
    use crate::cnc_api::{InterpreterState as Interp, TaskState as Task};

    /// Progress must stay off when no run command was accepted, even while jogging.
    #[test]
    fn inactive_without_run_command() {
        let mut tracking = ProgramTracking::default();
        tracking.observe(Task::On, Interp::Reading);

        assert!(!tracking.is_active());
    }

    /// Progress turns off once the program finishes and the controller returns to Idle.
    #[test]
    fn stops_after_running_returns_to_idle() {
        let mut tracking = ProgramTracking::default();
        tracking.started();
        tracking.observe(Task::On, Interp::Reading);
        assert!(tracking.is_active());

        tracking.observe(Task::On, Interp::Idle);
        assert!(!tracking.is_active());
    }

    /// An Idle report right after the run command must not stop tracking.
    #[test]
    fn idle_before_running_does_not_stop_tracking() {
        let mut tracking = ProgramTracking::default();
        tracking.started();
        tracking.observe(Task::On, Interp::Idle);

        assert!(tracking.is_active());
    }

    /// Pause keeps the program active; resume continues to report progress.
    #[test]
    fn pause_keeps_tracking_active() {
        let mut tracking = ProgramTracking::default();
        tracking.started();
        tracking.observe(Task::On, Interp::Reading);
        tracking.observe(Task::On, Interp::Paused);

        assert!(tracking.is_active());
    }

    /// E-Stop stops tracking immediately, even while the interpreter still reads.
    #[test]
    fn estop_stops_tracking() {
        let mut tracking = ProgramTracking::default();
        tracking.started();
        tracking.observe(Task::On, Interp::Reading);
        tracking.observe(Task::Estop, Interp::Reading);

        assert!(!tracking.is_active());
    }

    /// Abort or a new program load stops tracking explicitly.
    #[test]
    fn explicit_stop_clears_tracking() {
        let mut tracking = ProgramTracking::default();
        tracking.started();
        tracking.stopped();

        assert!(!tracking.is_active());
    }
}
