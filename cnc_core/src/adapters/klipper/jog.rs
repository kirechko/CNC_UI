//! Bounded continuous-jog emulation for Klipper, which has no jog-cancel API.

use super::{KlipperAdapter, Session};
use crate::{CncResult, JogRequest};

/// Maximum queued movement duration; shorter segments reduce the release latency and stop pulsing.
const JOG_CHUNK_SECONDS: f64 = 0.05;

impl KlipperAdapter {
    /// Queues one short relative segment and leaves the next segment for the following status cycle.
    pub(super) async fn send_next_jog_chunk(&self, session: &mut Session) -> CncResult<()> {
        let Some((axis, direction, feed_rate)) = session.continuous_jog else {
            return Ok(());
        };
        let distance = (feed_rate / 60.0 * JOG_CHUNK_SECONDS).max(0.01);
        self.incremental_jog(
            session,
            JogRequest {
                axis,
                direction,
                distance,
                feed_rate: Some(feed_rate),
            },
        )
        .await
    }
}
