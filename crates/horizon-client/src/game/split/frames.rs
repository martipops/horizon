//! The frame-time readout (`HORIZON_SPLIT_STATS=1`). Rendering the players on threads was tried
//! and measured slower (aeae9cb30, ec13017bd): their renders hold each other up inside wgpu.
use super::*;

impl<'a> Split<'a> {
    /// `HORIZON_SPLIT_STATS=1`: every 5 s, each player's frame rate and its CPU and GPU time a
    /// frame, in the log (the dev launcher's Performance readout).
    pub(super) fn print_stats(&mut self) {
        let Some(since) = self.stats else {
            return;
        };
        if since.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.stats = Some(Instant::now());
        let games =
            std::iter::once(&mut self.host).chain(self.guests.iter_mut().map(|g| &mut *g.game));
        let frames = self.spent_frames.max(1);
        let [turns, presents, wait, outside] =
            std::mem::take(&mut self.spent).map(|d| d.as_secs_f64() * 1000.0 / f64::from(frames));
        self.spent_frames = 0;
        let mut line = format!(
            " a frame {:.2} ms: turns {turns:.2} (with the renders), presents {presents:.2}, gpu wait {wait:.2}, outside {outside:.2}.",
            turns + presents + wait + outside,
        );
        for (i, game) in games.enumerate() {
            let s = game.frame_stats.summary();
            let ms = |k: &str| s[k]["avg_ms"].as_f64().unwrap_or(0.0);
            line += &format!(
                "  P{}: {:.0} fps, cpu {:.2} ms (render {:.2} ms), gpu {:.2} ms",
                i + 1,
                s["avg_fps"].as_f64().unwrap_or(0.0),
                ms("cpu"),
                s["phases_avg_ms"]["render"].as_f64().unwrap_or(0.0),
                ms("gpu"),
            );
            game.frame_stats.reset();
        }
        log::info!("Horizon: splitscreen frames:{line}");
    }
}
