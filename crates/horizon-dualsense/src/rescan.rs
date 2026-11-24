//! When a writer thread looks for its pad. The DualSense's writer ([`crate::device`]) and the
//! Xbox one ([`crate::xbox`]) each open their pad on a thread of their own and look for it again
//! when it goes; this says when, for both, so neither can look in a loop.
use std::time::{Duration, Instant};

/// The looks in a row after one finds nothing (the pad's hidraw node may appear after the
/// controller does), and the pads in a row that may open and fail their first reports.
pub(crate) const TRIES: u32 = 5;
/// The time between two looks, and the least between two opens.
pub(crate) const EVERY: Duration = Duration::from_secs(1);

pub(crate) struct Rescan {
    /// The looks left; with none the writer waits to be asked.
    left: u32,
    /// When the next look is due.
    next: Instant,
    /// When the pad now open, or the last one, opened.
    opened: Instant,
    /// Whether that pad took its first reports.
    started: bool,
    /// The pads opened in a row that failed before taking them.
    dead: u32,
}

impl Rescan {
    /// A link that just started: it looks once, now.
    pub(crate) fn new(now: Instant) -> Self {
        Rescan {
            left: 1,
            next: now,
            opened: now,
            started: false,
            dead: 0,
        }
    }

    /// Whether to look for the pad now.
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.left > 0 && now >= self.next
    }

    /// When the next look is due, while one is.
    pub(crate) fn wake(&self) -> Option<Instant> {
        (self.left > 0).then_some(self.next)
    }

    /// A look starts: it uses one, and the next is [`EVERY`] away.
    pub(crate) fn look(&mut self, now: Instant) {
        self.left = self.left.saturating_sub(1);
        self.next = now + EVERY;
    }

    /// The game asked (a controller came or went): look now, with every try back.
    pub(crate) fn asked(&mut self, now: Instant) {
        self.left = TRIES;
        self.next = now;
        self.dead = 0;
    }

    /// The pad can't be opened at all (no access to it): no more looks until the game asks.
    pub(crate) fn stop(&mut self) {
        self.left = 0;
    }

    /// The pad opened; whether it takes its first reports is still to see.
    pub(crate) fn opened(&mut self, now: Instant) {
        self.left = 0;
        self.opened = now;
        self.started = false;
    }

    /// The open pad took its first reports: it works.
    pub(crate) fn started(&mut self) {
        self.started = true;
        self.dead = 0;
    }

    /// The open pad failed and its handle was dropped. Returns whether it was given up.
    ///
    /// One that had taken its first reports slept or was unplugged, and may come back: look
    /// now, though never sooner than [`EVERY`] after it opened.
    ///
    /// One that failed on them is opened again [`EVERY`] later, not at once, and after [`TRIES`]
    /// of those in a row it is left alone until the game asks: such a pad was reopened in a loop
    /// with no wait, for as long as it kept failing. How long the failure took doesn't count:
    /// hidapi on Windows gives a write a second before it fails, and that is still a pad that
    /// never took a report.
    pub(crate) fn lost(&mut self, now: Instant) -> bool {
        if std::mem::take(&mut self.started) {
            self.left = TRIES;
            self.next = now.max(self.opened + EVERY);
            return false;
        }
        self.dead += 1;
        if self.dead >= TRIES {
            self.left = 0;
            return true;
        }
        self.left = TRIES;
        self.next = now + EVERY;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The writer's loop wakes at least this often while a look is due.
    const STEP: Duration = Duration::from_millis(5);

    /// A pad that opens every time it is looked for.
    struct Pad {
        /// How long its first reports block the writer's thread.
        startup: Duration,
        /// Whether it takes them.
        takes: bool,
        /// How long it then works before it fails.
        lives: Duration,
    }

    /// Run a writer's loop for `length` from `start`, on a clock that moves by [`STEP`] and by
    /// what the pad's first reports take: the times the pad was opened at, counted from `start`.
    fn opens(rescan: &mut Rescan, start: Instant, length: Duration, pad: &Pad) -> Vec<Duration> {
        let mut opened = Vec::new();
        let mut fails = None;
        let mut now = start;
        while now < start + length {
            if fails.is_some_and(|at| now >= at) {
                fails = None;
                rescan.lost(now);
            }
            if fails.is_none() && rescan.due(now) {
                rescan.look(now);
                rescan.opened(now);
                opened.push(now - start);
                now += pad.startup;
                if pad.takes {
                    rescan.started();
                    fails = Some(now + pad.lives);
                } else {
                    rescan.lost(now);
                }
            }
            now += STEP;
        }
        opened
    }

    const MINUTE: Duration = Duration::from_secs(60);

    /// A pad that opens and fails its first report used to be opened again at once, forever.
    #[test]
    fn a_pad_that_fails_as_it_opens_is_opened_a_second_apart_and_then_left_alone() {
        let start = Instant::now();
        let mut rescan = Rescan::new(start);
        let dead = Pad {
            startup: Duration::ZERO,
            takes: false,
            lives: Duration::ZERO,
        };
        assert_eq!(
            opens(&mut rescan, start, MINUTE, &dead),
            [0, 1, 2, 3, 4].map(Duration::from_secs)
        );
        // A controller connects: the game asks, and the pad gets as many tries again.
        let later = start + MINUTE;
        rescan.asked(later);
        assert_eq!(opens(&mut rescan, later, MINUTE, &dead).len(), 5);
    }

    /// hidapi on Windows gives a write a second before it fails. A pad whose first write times
    /// out never took a report, however long that took: it is given up like the one above.
    #[test]
    fn a_first_report_that_times_out_is_still_a_pad_that_never_worked() {
        let start = Instant::now();
        let mut rescan = Rescan::new(start);
        let times_out = Pad {
            startup: Duration::from_secs(1),
            takes: false,
            lives: Duration::ZERO,
        };
        assert_eq!(
            opens(&mut rescan, start, MINUTE, &times_out),
            [0, 2, 4, 6, 8].map(Duration::from_secs)
        );
    }

    /// A pad that takes its first reports works, even when it goes within the second: it is
    /// never given up, and never opened more than once a second.
    #[test]
    fn a_pad_that_worked_briefly_is_reopened_once_a_second_and_never_given_up() {
        let start = Instant::now();
        let mut rescan = Rescan::new(start);
        let brief = Pad {
            startup: Duration::ZERO,
            takes: true,
            lives: Duration::from_millis(200),
        };
        let opened = opens(&mut rescan, start, MINUTE, &brief);
        assert_eq!(opened.len(), 60);
        assert!(opened.windows(2).all(|pair| pair[1] - pair[0] == EVERY));
    }

    /// A pad that worked for a while and then went (asleep, unplugged) is still looked for at
    /// once, however many times it goes.
    #[test]
    fn a_pad_that_worked_is_looked_for_at_once_each_time_it_goes() {
        let start = Instant::now();
        let mut rescan = Rescan::new(start);
        let works = Pad {
            startup: Duration::ZERO,
            takes: true,
            lives: Duration::from_secs(3),
        };
        assert_eq!(
            opens(&mut rescan, start, Duration::from_secs(30), &works),
            [0, 3, 6, 9, 12, 15, 18, 21, 24, 27].map(Duration::from_secs)
        );
    }
}
