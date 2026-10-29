//! Jog command formatting and the chunked continuous-jog sender.
//!
//! The `CncApi` trait impl lives entirely in `api.rs` (Rust permits only one
//! impl block per trait/type pair), so this file provides the `pub(super)`
//! helpers the trait methods delegate to.
//!
//! GRBL has no "start continuous jog" word: the adapter keeps sending fixed
//! [`super::JOG_CHUNK_MM`]-millimeter `$J=G91` moves while the controller
//! reports `Idle`, which is what makes holding a UI button behave like a
//! continuous motion.

use super::{ContinuousJog, GrblAdapter, Session};
use crate::{Axis, CncResult, ControllerError};

impl GrblAdapter {
    /// Builds an incremental single-axis jog command in millimeters.
    /// `distance` is signed: the direction already multiplied into it.
    pub(super) fn format_axis_jog_command(
        axis: Axis,
        distance: f64,
        feed_rate: f64,
    ) -> CncResult<String> {
        if !distance.is_finite() || distance == 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog distance must be finite and non-zero",
            ));
        }
        if !feed_rate.is_finite() || feed_rate <= 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog feed rate must be finite and positive",
            ));
        }

        let command = format!(
            "$J=G21 G91 {}{distance} F{feed_rate}",
            Self::axis_letter(axis)
        );
        Self::validate_command(&command)?;
        Ok(command)
    }

    /// Builds a diagonal XY jog chunk: the unit direction vector is scaled to
    /// the chunk length so diagonal speed matches a single-axis move.
    pub(super) fn format_xy_jog_command(
        x_direction: i8,
        y_direction: i8,
        feed_rate: f64,
    ) -> CncResult<String> {
        if ![-1, 0, 1].contains(&x_direction)
            || ![-1, 0, 1].contains(&y_direction)
            || (x_direction == 0 && y_direction == 0)
            || !feed_rate.is_finite()
            || feed_rate <= 0.0
        {
            return Err(ControllerError::new("bad_jog", "invalid continuous XY jog"));
        }

        let norm = f64::from(x_direction * x_direction + y_direction * y_direction).sqrt();
        let x = super::JOG_CHUNK_MM * f64::from(x_direction) / norm;
        let y = super::JOG_CHUNK_MM * f64::from(y_direction) / norm;
        let command = format!("$J=G21 G91 X{x:.4} Y{y:.4} F{feed_rate}");
        Self::validate_command(&command)?;
        Ok(command)
    }

    /// Sends one chunk of a continuous jog, choosing the single-axis or the
    /// two-axis formatter depending on whether an axis was pinned.
    pub(super) async fn send_jog_chunk(session: &mut Session, jog: ContinuousJog) -> CncResult<()> {
        let command = if let Some(axis) = jog.axis {
            Self::format_axis_jog_command(
                axis,
                super::JOG_CHUNK_MM * f64::from(jog.x_direction),
                jog.feed_rate,
            )?
        } else {
            Self::format_xy_jog_command(jog.x_direction, jog.y_direction, jog.feed_rate)?
        };

        Self::send_command(session, &command).await
    }

    /// Starts or replaces a continuous jog under the session lock.
    ///
    /// Shared by the single-axis and XY trait entry points: when a chunk is
    /// still in flight, a realtime Feed Hold (0x85) stops it and the new
    /// direction simply waits for the next Idle poll to replay; otherwise the
    /// first chunk is sent immediately.
    pub(super) async fn run_continuous_jog(&self, jog: ContinuousJog) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        let chunk_active = session.jog_chunk_active;
        session.continuous_jog = Some(jog);

        if chunk_active {
            Self::send_realtime(&mut session, 0x85).await?;
            session.jog_chunk_active = false;
            return Ok(());
        }

        if let Err(error) = Self::send_jog_chunk(&mut session, jog).await {
            session.continuous_jog = None;
            return Err(error);
        }
        session.jog_chunk_active = true;
        Ok(())
    }
}
