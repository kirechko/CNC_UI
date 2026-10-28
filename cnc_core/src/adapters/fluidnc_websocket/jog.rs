//! Continuous-jog chunk emission for [`FluidNcWebSocketAdapter`]. The
//! `CncApi` jog trait methods stay in `api.rs`; this file carries only the
//! inherent helper that turns a stored [`ContinuousJog`] into one `$J=` chunk.

use super::{ContinuousJog, FluidNcWebSocketAdapter, Session, JOG_CHUNK_MM};
use crate::CncResult;

impl FluidNcWebSocketAdapter {
    /// Send one fixed-distance jog chunk in the requested direction. The chunk
    /// keeps the physical tool moving while the UI status cycle re-arms it.
    pub(super) async fn send_jog_chunk(
        &self,
        session: &mut Session,
        jog: ContinuousJog,
    ) -> CncResult<()> {
        let command = if let Some(axis) = jog.axis {
            format!(
                "$J=G21 G91 {}{:.3} F{:.0}",
                Self::axis_letter(axis),
                JOG_CHUNK_MM * f64::from(jog.x),
                jog.feed_rate,
            )
        } else {
            let norm = f64::from(jog.x * jog.x + jog.y * jog.y).sqrt();
            let x = JOG_CHUNK_MM * f64::from(jog.x) / norm;
            let y = JOG_CHUNK_MM * f64::from(jog.y) / norm;
            format!("$J=G21 G91 X{x:.3} Y{y:.3} F{:.0}", jog.feed_rate)
        };
        self.send_command(session, &command).await
    }
}
