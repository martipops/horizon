//! Every player's frame rendered after the turns (one after another, or on threads with
//! `HORIZON_SPLIT_THREADS=1`), and the frame-time readout.
use super::*;

impl<'a> Split<'a> {
    /// Every player's frame, built in its turn (`Game::render` keeps it in splitscreen), renders:
    /// one after another, or at once on threads (`threads`: one each, the last on this one).
    /// Then each game gets its storage back.
    // ponytail: threads spawned per frame; measured slower anyway (renders contend in wgpu).
    pub(super) fn render_all(&mut self) {
        let threads = self.threads;
        let mut games: Vec<&mut Game<'a>> = std::iter::once(&mut self.host)
            .chain(self.guests.iter_mut().map(|g| &mut g.game))
            .collect();
        let mut frames: Vec<Option<horizon_client::gfx::Frame>> =
            games.iter_mut().map(|g| g.pending_frame.take()).collect();
        let done = std::thread::scope(|scope| {
            let mut jobs: Vec<_> = games
                .iter_mut()
                .zip(&mut frames)
                .enumerate()
                .filter_map(|(k, (game, frame))| Some((k, game.renderer.as_mut()?, frame.take()?)))
                .collect();
            let render = |renderer: &mut horizon_client::gfx::Renderer, frame| {
                let result = renderer.render(&frame).map_err(|e| e.to_string());
                (frame, result)
            };
            if !threads {
                return jobs
                    .into_iter()
                    .map(|(k, renderer, frame)| (k, render(renderer, frame)))
                    .collect();
            }
            let here = jobs.pop();
            let threads: Vec<_> = jobs
                .into_iter()
                .map(|(k, renderer, frame)| (k, scope.spawn(move || render(renderer, frame))))
                .collect();
            let mut done: Vec<_> = here
                .map(|(k, renderer, frame)| (k, render(renderer, frame)))
                .into_iter()
                .collect();
            for (k, thread) in threads {
                match thread.join() {
                    Ok(out) => done.push((k, out)),
                    Err(panic) => std::panic::resume_unwind(panic),
                }
            }
            done
        });
        for (k, (frame, result)) in done {
            games[k].take_back(frame);
            if let Err(e) = result {
                log::warn!("Horizon: splitscreen: P{} could not render: {e}", k + 1);
            }
        }
    }

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
            std::iter::once(&mut self.host).chain(self.guests.iter_mut().map(|g| &mut g.game));
        let frames = self.spent_frames.max(1);
        let [turns, renders, presents, wait, outside] =
            std::mem::take(&mut self.spent).map(|d| d.as_secs_f64() * 1000.0 / f64::from(frames));
        self.spent_frames = 0;
        let mut line = format!(
            " a frame {:.2} ms: turns {turns:.2}, renders {renders:.2} ({}), presents {presents:.2}, gpu wait {wait:.2}, outside {outside:.2}.",
            turns + renders + presents + wait + outside,
            if self.threads {
                "threads"
            } else {
                "one after another"
            },
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
