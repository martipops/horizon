//! When a writer thread looks for its pad. The DualSense's writer ([`crate::device`]) and the
//! Xbox one ([`crate::xbox`]) each open their pad on a thread of their own and look for it again
//! when it goes; this says when, for both, so neither can look in a loop.
use std::time::{Duration, Instant};

/// The looks in a row after one finds nothing (the pad's hidraw node may appear after the
/// controller does), and the pads in a row that may open and fail straight away.
pub(crate) const TRIES: u32 = 5;
/// The time between two looks, and how long a pad has to stay open to count as one that worked.
pub(crate) const EVERY: Duration = Duration::from_secs(1);

pub(crate) struct Rescan {
    /// The looks left; with none the writer waits to be asked.
    left: u32,
    /// When the next look is due.
    next: Instant,
    /// When the pad now open, or the last one, opened.
    opened: Instant,
    /// The pads opened in a row that failed before [`EVERY`] was over.
    dead: u32,
}

impl Rescan {
    /// A link that just started: it looks once, now.
    pub(crate) fn new(now: Instant) -> Self {
        Rescan {
            left: 1,
            next: now,
            opened: now,
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

    /// The pad opened; whether it takes reports is still to see.
    pub(crate) fn opened(&mut self, now: Instant) {
        self.left = 0;
        self.opened = now;
    }

    /// The open pad failed and its handle was dropped. One that stayed open for [`EVERY`] worked,
    /// then slept or was unplugged, and may come back: look now. One that failed sooner (its
    /// first read or write, usually) is opened again [`EVERY`] later, not at once, and after
    /// [`TRIES`] of those in a row it is left alone until the game asks: such a pad was reopened
    /// in a loop with no wait, for as long as it kept failing. Returns whether it was given up.
    pub(crate) fn lost(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.opened) >= EVERY {
            self.dead = 0;
            self.left = TRIES;
            self.next = now;
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

    /// Run a writer's loop for `length` from `start`, on a clock that moves by [`STEP`], with a
    /// pad that opens every time it is looked for and fails `lives` later: the times it was
    /// opened at, counted from `start`.
    fn opens(
        rescan: &mut Rescan,
        start: Instant,
        length: Duration,
        lives: Duration,
    ) -> Vec<Duration> {
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
                if lives.is_zero() {
                    rescan.lost(now);
                } else {
                    fails = Some(now + lives);
                }
            }
            now += STEP;
        }
        opened
    }

    /// A pad that opens and fails its first report used to be opened again at once, forever.
    #[test]
    fn a_pad_that_fails_as_it_opens_is_opened_a_second_apart_and_then_left_alone() {
        let start = Instant::now();
        let mut rescan = Rescan::new(start);
        let minute = Duration::from_secs(60);
        assert_eq!(
            opens(&mut rescan, start, minute, Duration::ZERO),
            [0, 1, 2, 3, 4].map(Duration::from_secs)
        );
        // A controller connects: the game asks, and the pad gets as many tries again.
        let later = start + minute;
        rescan.asked(later);
        assert_eq!(opens(&mut rescan, later, minute, Duration::ZERO).len(), 5);
    }

    /// One that takes its first report and fails a moment later is no different: it can't be
    /// reopened faster than once a second either.
    #[test]
    fn a_pad_that_fails_soon_after_opening_is_given_up_too() {
        let start = Instant::now();
        let mut rescan = Rescan::new(start);
        let opened = opens(
            &mut rescan,
            start,
            Duration::from_secs(60),
            Duration::from_millis(200),
        );
        assert_eq!(opened.len(), 5);
        assert!(opened.windows(2).all(|pair| pair[1] - pair[0] >= EVERY));
    }

    /// A pad that worked and then went (asleep, unplugged) is still looked for at once, however
    /// many times it goes.
    #[test]
    fn a_pad_that_worked_is_looked_for_at_once_each_time_it_goes() {
        let start = Instant::now();
        let mut rescan = Rescan::new(start);
        assert_eq!(
            opens(
                &mut rescan,
                start,
                Duration::from_secs(30),
                Duration::from_secs(3)
            ),
            [0, 3, 6, 9, 12, 15, 18, 21, 24, 27].map(Duration::from_secs)
        );
    }
}
