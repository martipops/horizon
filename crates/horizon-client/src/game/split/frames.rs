//! Every player's frame rendered after the turns (one after another, or on render workers with
//! `HORIZON_SPLIT_THREADS=1`), and the frame-time readout.
use super::*;
use horizon_client::gfx::{Frame, Renderer};
use std::sync::mpsc;

type Rendered = (Renderer, Frame, std::result::Result<bool, String>);

/// A thread that renders one player's frames for the split's whole life.
pub(super) struct RenderWorker {
    jobs: mpsc::Sender<(Renderer, Frame)>,
    done: mpsc::Receiver<Rendered>,
}

impl RenderWorker {
    fn start(seat: usize) -> Self {
        let (jobs, inbox) = mpsc::channel::<(Renderer, Frame)>();
        let (outbox, done) = mpsc::channel();
        std::thread::Builder::new()
            .name(format!("horizon-render-{seat}"))
            .spawn(move || {
                for (mut renderer, frame) in inbox {
                    let result = renderer.render(&frame).map_err(|e| e.to_string());
                    if outbox.send((renderer, frame, result)).is_err() {
                        break;
                    }
                }
            })
            .expect("a render worker thread");
        Self { jobs, done }
    }
}

impl<'a> Split<'a> {
    /// Every player's frame, built in its turn (`Game::render` keeps it in splitscreen), renders:
    /// one after another, or at once (`threads`): each but the last on a worker of its own that
    /// lives as long as the split (a thread a frame cost more than it saved on Windows: every new
    /// thread runs each DLL's thread setup and becomes a GUI thread), the last on this one. The
    /// renderer goes to its worker and comes back with the frame. Then each game gets its storage
    /// back.
    pub(super) fn render_all(&mut self) {
        let mut games: Vec<&mut Game<'a>> = std::iter::once(&mut self.host)
            .chain(self.guests.iter_mut().map(|g| &mut g.game))
            .collect();
        let mut jobs = Vec::new();
        for (k, game) in games.iter_mut().enumerate() {
            let Some(frame) = game.pending_frame.take() else {
                continue;
            };
            match game.renderer.take() {
                Some(renderer) => jobs.push((k, renderer, frame)),
                None => game.take_back(frame),
            }
        }
        let render = |mut renderer: Renderer, frame: Frame| {
            let result = renderer.render(&frame).map_err(|e| e.to_string());
            (renderer, frame, result)
        };
        let here = jobs.pop();
        let mut done = Vec::with_capacity(jobs.len() + 1);
        let mut sent = Vec::new();
        if self.threads {
            while self.workers.len() < jobs.len() {
                self.workers
                    .push(RenderWorker::start(self.workers.len() + 2));
            }
            for ((k, renderer, frame), worker) in jobs.into_iter().zip(&self.workers) {
                worker
                    .jobs
                    .send((renderer, frame))
                    .expect("a render worker stopped");
                sent.push(k);
            }
        } else {
            for (k, renderer, frame) in jobs {
                done.push((k, render(renderer, frame)));
            }
        }
        if let Some((k, renderer, frame)) = here {
            done.push((k, render(renderer, frame)));
        }
        for (k, worker) in sent.into_iter().zip(&self.workers) {
            done.push((k, worker.done.recv().expect("a render worker stopped")));
        }
        for (k, (renderer, frame, result)) in done {
            games[k].renderer = Some(renderer);
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
