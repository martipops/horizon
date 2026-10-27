//! Splitscreen (Horizon's Splitscreen page, off by default): players drop in and out of P1's world
//! on this computer, as on a console. P1 is the game as launched, with the keyboard, the mouse and
//! the first controller used in a menu. In a world, another controller pressing a button is
//! offered to join (several at once: each its own offer); pressing A adds a player, another `Game` in this process that joins P1's
//! server thread through memory, after it picks a profile and where to play in its view
//! (`title/profiles.rs`). A player who disconnects (pause menu) drops out; when P1 leaves the
//! world, all do.
//!
//! Each player plays in a window: Window 1 (the game's), Window 2, 3... (opened when someone picks
//! New window, closed with its last player), or a monitor's (borderless over it). Players in one
//! window share it: halves for two (side by side or top and bottom), three one wide and two (or
//! in columns), quarters for four.
//! A joining player starts in Window 1 and moves with its controller's bumpers before it picks
//! its profile; the Splitscreen page moves anyone later. Every player renders offscreen on one device and each
//! window's compositor copies its players' frames. Turning splitscreen on rebuilds P1's renderer
//! that way at once (the world is meshed again: a moment, in the menu); off, back.
//!
//! A controller that disconnects leaves its player waiting, its view dimmed: the same controller
//! reconnecting takes it back, A on another takes it over, B on another lets the player leave; a
//! player still picking a profile leaves at once. P1 keeps the keyboard and the mouse (a notice,
//! not a question).
//!
//! Keyboard and mouse drive the player whose view was clicked last (Esc frees the mouse, then
//! click another view); closing a player's own window drops that player. The players' frames are
//! paced once, at the fastest any wants.
//!
//! `HORIZON_SPLIT=N` (testing): splitscreen on, and N-1 players without controllers join once P1
//! is in a world, as P2, P3... (`HORIZON_SPLIT_PICK=1`: picking profiles instead). The player in
//! seat N takes `horizon-ctl` on P1's socket path + "N".

use super::{
    Game, UiResources, build_renderer,
    pad::{PadEvent, PadId, PadRouter},
    start_game,
};
use horizon_client::split_layout::{self as layout, MAX_PLAYERS, Placement, Spot, View, place};
use horizon_client::{
    Result,
    gamepad::{Button, PadState},
    gfx::compositor::Compositor,
    hud::{HudBatch, rgba},
    widgets,
};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize},
    event::{DeviceEvent, DeviceId, ElementState, WindowEvent},
    event_loop::ActiveEventLoop,
    monitor::MonitorHandle,
    window::{Window, WindowId},
};

/// How long a controller's offer to join stays up.
const OFFER: Duration = Duration::from_secs(10);
/// The game's window, the first of the numbered ones.
const WINDOW_1: &str = "Window 1";
const NEW_WINDOW: &str = "New window";

thread_local! {
    /// The options file of the player whose turn it is (none for P1, whose is the game's own):
    /// every player runs on this thread, one after another.
    static OPTIONS: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// The `options.txt` of the player whose turn it is; `None` for P1.
pub(super) fn options_path() -> Option<PathBuf> {
    OPTIONS.with_borrow(Clone::clone)
}

/// What splitscreen draws over a player's view (`Game::split_hud`): its corner badge (who, and a
/// moment where), a question its controller answers, and notices nobody has to answer.
#[derive(Default)]
pub(super) struct Hud {
    badge: Option<String>,
    place: Option<(String, Instant)>,
    prompt: Option<Prompt>,
    notice: Option<horizon_client::toasts::SystemToast>,
}

/// A question on a player's view, answered with its controller's buttons.
#[derive(Clone, PartialEq)]
pub(super) struct Prompt {
    ask: Ask,
    title: String,
    choices: Vec<(Button, &'static str)>,
}

/// What a prompt asks: a controller may join (every view, a strip at the top: nobody stops
/// playing for it), this player leaving, P1's controller going (the view dimmed beneath).
#[derive(Clone, Copy, PartialEq)]
enum Ask {
    Join,
    Leave,
    Controller,
    /// Its controller disconnected: any controller answers.
    Lost,
}

/// A player whose controller disconnected, waiting for one: its seat (1 = P1), and the
/// controller's id and kind (`PadRouter::uuid`) to know it back.
struct Lost<Id> {
    seat: usize,
    id: Id,
    uuid: [u8; 16],
}

/// What a button on a controller no player has does while players wait for one.
#[derive(Debug, PartialEq)]
enum Claim {
    /// It goes to `lost[k]`.
    Give(usize),
    /// `lost[k]` leaves instead.
    Leave(usize),
    /// Nothing: the others wait (no offer to join meanwhile).
    Wait,
    /// Nobody but P1 waits, and P1 has the keyboard: as if nobody did.
    Pass,
}

/// The same kind of controller back (a new id on macOS) takes back its player with any button;
/// another takes over the player waiting longest with A, or lets it leave with B. P1 is only
/// given its own back (another joins as P1 through the offer to join).
fn claim<Id>(lost: &[Lost<Id>], uuid: [u8; 16], button: Button) -> Claim {
    if uuid != [0; 16]
        && let Some(k) = lost.iter().position(|l| l.uuid == uuid)
    {
        return Claim::Give(k);
    }
    let Some(k) = lost.iter().position(|l| l.seat != 1) else {
        return Claim::Pass;
    };
    match button {
        Button::South => Claim::Give(k),
        Button::East => Claim::Leave(k),
        _ => Claim::Wait,
    }
}

/// The question on a waiting player's view.
fn lost_prompt(seat: usize) -> Prompt {
    Prompt {
        ask: Ask::Lost,
        title: format!("P{seat}: controller disconnected"),
        choices: vec![(Button::South, "Use another"), (Button::East, "Leave")],
    }
}

/// Holding Select 2 s asks a question the controller then answers; meanwhile its buttons answer
/// that, not the game.
#[derive(Default)]
struct HoldSelect {
    since: Option<Instant>,
    last: PadState,
    asking: bool,
}

/// What holding Select did this frame.
enum Hold {
    Nothing,
    /// Held 2 s: ask now.
    Ask,
    /// One of the answers, pressed while asked.
    Answer(Button),
}

impl HoldSelect {
    fn update(&mut self, pad: PadState, answers: &[Button]) -> Hold {
        let last = std::mem::replace(&mut self.last, pad);
        if self.asking {
            return match answers.iter().find(|&&b| pad.held(b) && !last.held(b)) {
                Some(&b) => {
                    self.asking = false;
                    Hold::Answer(b)
                }
                None => Hold::Nothing,
            };
        }
        if !pad.held(Button::Select) {
            self.since = None;
            return Hold::Nothing;
        }
        if self.since.get_or_insert_with(Instant::now).elapsed() < Duration::from_secs(2) {
            return Hold::Nothing;
        }
        self.since = None;
        self.asking = true;
        Hold::Ask
    }
}

/// Where a monitor is and its size, as windows are placed: macOS places them in points, each
/// monitor at its own scale (pixels would be read at the window's scale: a window for a 1x TV
/// beside a 2x screen would open on the screen, half size); Windows and X11 in desktop pixels.
fn cover(m: &MonitorHandle) -> (winit::dpi::Position, winit::dpi::Size) {
    #[cfg(target_os = "macos")]
    {
        let scale = m.scale_factor();
        (
            m.position().to_logical::<f64>(scale).into(),
            m.size().to_logical::<f64>(scale).into(),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        (m.position().into(), m.size().into())
    }
}

/// Where a new 854 x 480 window sits centred on monitor `m`, in `cover`'s units.
fn centred(m: &MonitorHandle) -> winit::dpi::Position {
    let (at, size) = cover(m);
    // The numbers as they are, logical or physical.
    let (at, size) = (at.to_physical::<f64>(1.0), size.to_physical::<f64>(1.0));
    let u = if cfg!(target_os = "macos") {
        1.0
    } else {
        m.scale_factor()
    };
    let x = at.x + (size.width - 854.0 * u) / 2.0;
    let y = at.y + (size.height - 480.0 * u) / 2.0;
    if cfg!(target_os = "macos") {
        winit::dpi::LogicalPosition::new(x, y).into()
    } else {
        winit::dpi::PhysicalPosition::new(x, y).into()
    }
}

/// Player `game` into `window`; when it `moved` from another, the mouse it held there goes free
/// (a click takes it in the new one).
fn into_window(game: &mut Game<'_>, window: Arc<Window>, moved: bool) {
    if moved && game.captured {
        game.capture(false);
    }
    game.window = Some(window);
}

/// A window showing players' views, and the compositor that copies their frames into it.
struct Surface {
    /// Which window this is, by number (1: the game's), and where it was opened: a number
    /// given again to a window placed elsewhere opens again there.
    key: u8,
    placement: Placement,
    window: Arc<Window>,
    compositor: Compositor,
    size: PhysicalSize<u32>,
    focused: bool,
    /// The title it was last given.
    title: String,
}

struct Guest<'a> {
    game: Game<'a>,
    /// The profile it plays as; `None` while it picks one.
    name: Option<String>,
    /// Its seat (2 to `MAX_PLAYERS`): its number on screen (P2...), its control socket, its sounds.
    seat: usize,
    /// The window it plays in, by number.
    window: u8,
    pad: Option<PadId>,
    options: PathBuf,
    /// Its buttons and focus targets while another player's game runs.
    widgets: widgets::Saved,
    /// The bumpers it held at the last frame (LB, RB), while it chooses where to play.
    bumpers: [bool; 2],
    /// Leaving from the controller (hold Select).
    hold: HoldSelect,
}

pub(super) struct Split<'a> {
    pub(super) host: Game<'a>,
    guests: Vec<Guest<'a>>,
    /// Splitscreen is on: every player renders through the compositors (else P1 draws on its
    /// window and every event goes straight to it).
    enabled: bool,
    /// `HORIZON_SPLIT=N`: players without controllers still to add once P1 is in a world.
    test_players: usize,
    pads: PadRouter,
    p1_pad: Option<PadId>,
    p1_window: u8,
    /// The windows players play in (Window 1, the game's, always first), each free or filling a
    /// monitor: what the setup screen's draft starts from (`layout::Draft`).
    windows: Vec<layout::Window>,
    /// Controllers offered to join, each since when: several may join at once.
    offers: Vec<(PadId, Instant)>,
    /// Players switching profile from the controller: its pad and where it played, to choose
    /// again there once it has left.
    rejoin: Vec<(Option<PadId>, layout::Window)>,
    /// P1's controller: holding Select asks whether it goes (P1 hosts: it leaves from the pause
    /// menu, but its controller can go).
    p1_hold: HoldSelect,
    /// Pads whose buttons still count as the press that joined them: released first.
    fresh: Vec<PadId>,
    /// Players whose controller disconnected, longest waiting first.
    lost: Vec<Lost<PadId>>,
    /// `HORIZON_SPLIT_STATS`: when the frame times were last printed.
    stats: Option<Instant>,
    /// The game's window and its compositor first, then monitors' and players' own.
    surfaces: Vec<Surface>,
    /// The views the players were last fitted to.
    applied: Vec<View>,
    /// Where each numbered window was when it closed, to open there again (this session).
    window_rects: std::collections::HashMap<u8, (LogicalPosition<f64>, LogicalSize<f64>)>,
    /// The monitors with their names, left to right, and the one Window 1 is on: read once a
    /// frame (`refresh_monitors`), the system's names are slow to ask for.
    screens: Vec<(String, MonitorHandle)>,
    main_screen: Option<String>,
    /// The seats the Setup screens last showed: a draft is by place in this list, so a change
    /// drops every draft.
    setup_seats: Vec<usize>,
    /// The player keyboard and mouse drive (0 = P1), and the pointer in the window it moved in.
    focus: usize,
    cursor: PhysicalPosition<f64>,
    limiter: horizon_client::window::FramerateLimiter,
    clock: horizon_client::window::SystemClock,
    /// P1's resources, shared by the other players' games for the process's lifetime.
    res: Option<&'static Arc<super::Resources>>,
    /// Whose game runs now (0 = P1), and P1's buttons while another's does.
    turn: usize,
    host_widgets: widgets::Saved,
}

impl<'a> Split<'a> {
    /// P1's game, with its compositor when it was made on the loader's surface.
    pub(super) fn new(mut host: Game<'a>, compositor: Option<Compositor>) -> Self {
        let test_players = std::env::var("HORIZON_SPLIT")
            .ok()
            .and_then(|n| n.parse::<usize>().ok())
            .map_or(0, |n| n.clamp(1, MAX_PLAYERS) - 1);
        if test_players > 0 {
            host.settings.extras.splitscreen = true;
        }
        let enabled = host.settings.extras.splitscreen;
        if enabled {
            host.options.offscreen = true;
            host.options.paced_by_split = true;
        }
        // Made after the window on an ordinary launch (no `resumed` follows).
        let surfaces = match (compositor, &host.window) {
            (Some(compositor), Some(window)) => vec![Surface {
                key: 1,
                placement: Placement::Free(None),
                window: window.clone(),
                compositor,
                size: window.inner_size(),
                focused: true,
                title: String::new(),
            }],
            _ => Vec::new(),
        };
        Self {
            host,
            guests: Vec::new(),
            enabled,
            test_players,
            pads: PadRouter::default(),
            p1_pad: None,
            p1_window: 1,
            windows: vec![layout::Window {
                number: 1,
                placement: Placement::Free(None),
            }],
            offers: Vec::new(),
            rejoin: Vec::new(),
            p1_hold: HoldSelect::default(),
            fresh: Vec::new(),
            lost: Vec::new(),
            stats: std::env::var_os("HORIZON_SPLIT_STATS").map(|_| Instant::now()),
            surfaces,
            applied: Vec::new(),
            window_rects: Default::default(),
            screens: Vec::new(),
            main_screen: None,
            setup_seats: Vec::new(),
            focus: 0,
            cursor: PhysicalPosition::new(0.0, 0.0),
            limiter: horizon_client::window::FramerateLimiter::new(0),
            clock: Default::default(),
            res: None,
            turn: 0,
            host_widgets: Default::default(),
        }
    }

    fn players(&self) -> usize {
        1 + self.guests.len()
    }

    /// The first seat free (2 to `MAX_PLAYERS`).
    fn free_seat(&self) -> Option<usize> {
        (2..=MAX_PLAYERS).find(|n| self.guests.iter().all(|g| g.seat != *n))
    }

    /// The splitscreen folder in the game folder: `profiles/<name>/` holds each profile.
    fn folder() -> Result<PathBuf> {
        Ok(std::env::current_dir()?.join("splitscreen"))
    }

    /// The window player `i` chose, by number.
    fn window_number(&self, i: usize) -> u8 {
        match i {
            0 => self.p1_window,
            _ => self.guests[i - 1].window,
        }
    }

    fn set_window(&mut self, i: usize, number: u8) {
        match i {
            0 => self.p1_window = number,
            _ => self.guests[i - 1].window = number,
        }
    }

    fn placement(&self, number: u8) -> Option<&Placement> {
        self.windows
            .iter()
            .find(|w| w.number == number)
            .map(|w| &w.placement)
    }

    /// The monitors now, with their names, left to right, and the one Window 1 is on.
    fn refresh_monitors(&mut self) {
        let Some(window) = self.surfaces.first().map(|s| s.window.clone()) else {
            return;
        };
        self.screens = window
            .available_monitors()
            .filter_map(|m| Some((m.name()?, m)))
            .collect();
        self.screens
            .sort_by_key(|(_, m)| (m.position().x, m.position().y));
        self.main_screen = window.current_monitor().and_then(|m| m.name());
    }

    /// The monitors, by name, left to right (as of this frame).
    fn monitors(&self) -> &[(String, MonitorHandle)] {
        &self.screens
    }

    /// The monitor the game's window is on: Window 1 already shows there.
    fn main_monitor(&self) -> Option<&String> {
        self.main_screen.as_ref()
    }

    /// How a monitor is called: numbered left to right, with its size (the system's names are
    /// ids on some).
    fn monitor_label(&self, name: &str) -> Option<String> {
        let (n, (_, m)) = self
            .monitors()
            .iter()
            .enumerate()
            .find(|(_, (n, _))| n == name)?;
        let size = m.size();
        Some(format!(
            "Monitor {} ({}x{})",
            n + 1,
            size.width,
            size.height
        ))
    }

    /// The window player `i` plays in now: Window 1 for a window that fills a monitor that's
    /// gone, or the one Window 1 is on.
    fn window_of(&self, i: usize) -> u8 {
        let number = self.window_number(i);
        let shown = match self.placement(number) {
            Some(Placement::Fill(name)) => {
                Some(name) != self.main_monitor() && self.monitors().iter().any(|(n, _)| n == name)
            }
            Some(Placement::Free(_)) => true,
            None => false,
        };
        if shown { number } else { 1 }
    }

    /// Where player `i` can play, as the screens say it: Window 1 (the game's), the other windows
    /// open, a new window, and each monitor no window fills but Window 1's. The page's button
    /// steps forward through them (buttons take the left click only), a joining player's bumpers
    /// both ways. A player alone in its window is at "New window" or its monitor's choice (its
    /// window is that), so the list stays the same as it steps.
    fn choices(&self, i: usize) -> Vec<(String, Spot)> {
        let alone = self.alone(i);
        let mine = self.window_of(i);
        let mut open: Vec<u8> = (0..self.players())
            .map(|j| self.window_of(j))
            .filter(|&n| !(alone && n == mine))
            .collect();
        open.sort_unstable();
        open.dedup();
        let filled: Vec<&String> = open
            .iter()
            .filter_map(|&n| match self.placement(n) {
                Some(Placement::Fill(name)) => Some(name),
                _ => None,
            })
            .collect();
        let main = self.main_monitor();
        let monitors = self.monitors().iter().filter_map(|(name, _)| {
            if Some(name) == main || filled.contains(&name) {
                return None;
            }
            Some((self.monitor_label(name)?, Spot::Fill(name.clone())))
        });
        std::iter::once((WINDOW_1.to_owned(), Spot::Window(1)))
            .chain(
                open.into_iter()
                    .filter(|&n| n != 1)
                    .map(|n| (self.window_label(n), Spot::Window(n))),
            )
            .chain(std::iter::once((NEW_WINDOW.to_owned(), Spot::New)))
            .chain(monitors)
            .collect()
    }

    /// Player `i` plays alone in a window of its own (not Window 1).
    fn alone(&self, i: usize) -> bool {
        let mine = self.window_of(i);
        mine != 1 && (0..self.players()).all(|j| j == i || self.window_of(j) != mine)
    }

    /// Player `i`'s place among its choices: its window, or alone, what its window is.
    fn current(&self, i: usize) -> Spot {
        let mine = self.window_of(i);
        match self.placement(mine) {
            Some(Placement::Free(_)) if self.alone(i) => Spot::New,
            Some(Placement::Fill(name)) if self.alone(i) => Spot::Fill(name.clone()),
            _ => Spot::Window(mine),
        }
    }

    /// How window `number` is shown: "Window 2", and the monitor it fills.
    fn window_label(&self, number: u8) -> String {
        match self.placement(number) {
            Some(Placement::Fill(name)) if number != 1 => match self.monitor_label(name) {
                Some(monitor) => format!("Window {number}, {monitor}"),
                None => format!("Window {number}"),
            },
            _ => format!("Window {number}"),
        }
    }

    /// Where player `i` plays, as the screens say it.
    fn label(&self, i: usize) -> String {
        self.window_label(self.window_of(i))
    }

    /// Player `i` into a window: one there is, or a new one with the first number free.
    fn choose(&mut self, i: usize, choice: Spot) {
        let placement = match choice {
            Spot::Window(n) => return self.set_window(i, n),
            Spot::New => Placement::Free(None),
            Spot::Fill(name) => Placement::Fill(name),
        };
        // Out of its window first: alone there, its number is free again.
        self.set_window(i, 1);
        self.forget_empty_windows();
        let number = (2..=u8::MAX)
            .find(|n| self.windows.iter().all(|w| w.number != *n))
            .unwrap_or(u8::MAX);
        self.windows.push(layout::Window { number, placement });
        self.set_window(i, number);
    }

    /// Windows nobody plays in close (Window 1 stays).
    fn forget_empty_windows(&mut self) {
        let used: Vec<u8> = (0..self.players()).map(|i| self.window_number(i)).collect();
        self.windows
            .retain(|w| w.number == 1 || used.contains(&w.number));
    }

    /// Every player's view, in the windows there are now.
    fn views(&self) -> Vec<View> {
        let windows: Vec<usize> = (0..self.players())
            .map(|i| {
                let number = self.window_of(i);
                self.surfaces
                    .iter()
                    .position(|s| s.key == number)
                    .unwrap_or(0)
            })
            .collect();
        let sizes: Vec<_> = self.surfaces.iter().map(|s| s.size).collect();
        let e = &self.host.settings.extras;
        place(&windows, &sizes, e.splitscreen_two, e.splitscreen_three)
    }

    /// Player `i`'s turn (0 = P1): its options file, and its own buttons and focus targets, which
    /// screens record while drawing and the control socket and the controller's cursor read.
    fn turn(&mut self, i: usize) {
        if i == self.turn {
            return;
        }
        let current = widgets::take();
        match self.turn {
            0 => self.host_widgets = current,
            t => {
                if let Some(g) = self.guests.get_mut(t - 1) {
                    g.widgets = current;
                }
            }
        }
        let next = match i {
            0 => std::mem::take(&mut self.host_widgets),
            _ => self
                .guests
                .get_mut(i - 1)
                .map(|g| std::mem::take(&mut g.widgets))
                .unwrap_or_default(),
        };
        widgets::put(next);
        OPTIONS.set(
            self.guests
                .get(i.wrapping_sub(1))
                .map(|g| g.options.clone()),
        );
        self.turn = i;
    }

    /// Player `i`'s game (0 = P1).
    fn game(&self, i: usize) -> Option<&Game<'a>> {
        match i {
            0 => Some(&self.host),
            _ => self.guests.get(i - 1).map(|g| &g.game),
        }
    }

    fn game_mut(&mut self, i: usize) -> Option<&mut Game<'a>> {
        match i {
            0 => Some(&mut self.host),
            _ => self.guests.get_mut(i - 1).map(|g| &mut g.game),
        }
    }

    /// Player `i`'s seat (P1's is 1) and name ("choosing" while it picks its profile).
    fn who(&self, i: usize) -> (usize, String) {
        match i {
            0 => (1, self.host.options.player_name()),
            _ => {
                let g = &self.guests[i - 1];
                (g.seat, g.name.clone().unwrap_or_else(|| "choosing".into()))
            }
        }
    }

    /// Player `i`'s game, in its turn, and its window's id (a game only takes its own window's
    /// events).
    fn player(&mut self, i: usize) -> Option<(&mut dyn ApplicationHandler, WindowId)> {
        self.turn(i);
        let game = self.game_mut(i)?;
        let id = game.window.as_ref()?.id();
        Some((game, id))
    }

    fn captured(&self, i: usize) -> bool {
        self.game(i).is_some_and(|g| g.captured)
    }

    /// The pointer as the focused player sees it: from its view's corner.
    fn cursor_in_focus(&self) -> PhysicalPosition<f64> {
        let [x, y] = self.views().get(self.focus).map_or([0, 0], |v| v.1);
        let at = self.cursor;
        PhysicalPosition::new(at.x - f64::from(x), at.y - f64::from(y))
    }

    /// Every player into its view: its window, then its size. Keyboard and mouse follow their
    /// player into another window; a player whose view moved or grew shows where on its badge.
    fn relayout(&mut self, event_loop: &ActiveEventLoop) {
        let views = self.views();
        let before = std::mem::replace(&mut self.applied, views);
        for (i, &(surface, _, size)) in self.applied.clone().iter().enumerate() {
            let Some(window) = self.surfaces.get(surface).map(|s| s.window.clone()) else {
                continue;
            };
            let moved = before.get(i).is_none_or(|b| b.0 != surface);
            if let Some(game) = self.game_mut(i) {
                into_window(game, window.clone(), moved);
            }
            if moved && i == self.focus && before.get(i).is_some() {
                window.focus_window();
            }
            if before.get(i) != self.applied.get(i) {
                self.tag(i, None);
            }
            if let Some((player, id)) = self.player(i) {
                player.window_event(event_loop, id, WindowEvent::Resized(size));
            }
        }
        self.turn(0);
    }

    /// Player `i` to the next place it could play (`back`: the one before); a new window is
    /// numbered now.
    fn step_screen(&mut self, i: usize, back: bool) {
        let choices = self.choices(i);
        let n = choices.len();
        if n == 0 {
            return;
        }
        let mine = self.current(i);
        let at = choices.iter().position(|(_, c)| *c == mine).unwrap_or(0);
        let next = choices[if back { (at + n - 1) % n } else { (at + 1) % n }]
            .1
            .clone();
        self.choose(i, next);
    }

    /// A joining player's bumper: its view moves to the next place it could play (`back`: the
    /// one before), at once, so it sees where it will be; choosing its profile keeps it there.
    fn place_joining(&mut self, i: usize, back: bool) {
        self.step_screen(i, back);
        let shown = self.label(i);
        self.guests[i - 1].game.split_hud.place = Some((
            format!("LB < {shown} > RB"),
            Instant::now() + Duration::from_secs(3),
        ));
    }

    /// Player `i`'s pad as its game read it (the controller or `horizon-ctl pad`), with the
    /// controller itself while its buttons are kept from the game.
    fn pad_now(&self, i: usize) -> PadState {
        let (state, pad) = match i {
            0 => (self.host.pad.state, self.p1_pad),
            _ => (self.guests[i - 1].game.pad.state, self.guests[i - 1].pad),
        };
        pad.map_or(state, |p| state.merge(&self.pads.state(p)))
    }

    /// Player `i` (not P1) holding Select 2 s is asked "Leave splitscreen?": A leaves, X
    /// switches profile (leaves, then chooses again where it was), Y opens the Setup screen, B
    /// stays.
    fn leave_by_controller(&mut self, i: usize) {
        let pad = self.pad_now(i);
        let g = &mut self.guests[i - 1];
        let answers = [Button::South, Button::West, Button::North, Button::East];
        match g.hold.update(pad, &answers) {
            Hold::Ask => {
                g.game.split_hud.prompt = Some(Prompt {
                    ask: Ask::Leave,
                    title: "Leave splitscreen?".to_owned(),
                    choices: vec![
                        (Button::South, "Leave"),
                        (Button::West, "Switch profile"),
                        (Button::North, "Setup"),
                        (Button::East, "Stay"),
                    ],
                });
            }
            Hold::Answer(answer) => {
                g.game.split_hud.prompt = None;
                if answer == Button::North {
                    g.game.open_split_setup();
                }
                if matches!(answer, Button::East | Button::North) {
                    return;
                }
                if answer == Button::West {
                    let number = g.window;
                    let window = self.windows.iter().find(|w| w.number == number).cloned();
                    self.rejoin.push((
                        g.pad,
                        window.unwrap_or(layout::Window {
                            number: 1,
                            placement: Placement::Free(None),
                        }),
                    ));
                }
                if let Err(e) = g.game.leave_server(None) {
                    log::warn!("Horizon: splitscreen: {e}");
                }
            }
            Hold::Nothing => {}
        }
    }

    /// P1 holding Select 2 s, as the others do to leave: P1 hosts the world (it ends splitscreen
    /// from the pause menu), but its controller can go: A takes it from P1 (keyboard and mouse
    /// stay), and in a world a button on it then offers to join as a new player; Y opens the
    /// Setup screen; B keeps it.
    fn p1_holds_select(&mut self) {
        let pad = self.pad_now(0);
        match self
            .p1_hold
            .update(pad, &[Button::South, Button::North, Button::East])
        {
            Hold::Ask => {
                self.host.split_hud.prompt = Some(Prompt {
                    ask: Ask::Controller,
                    title: "P1's controller".to_owned(),
                    choices: vec![
                        (Button::South, "Remove it from P1"),
                        (Button::North, "Setup"),
                        (Button::East, "Keep it"),
                    ],
                });
            }
            Hold::Answer(answer) => {
                self.host.split_hud.prompt = None;
                if answer == Button::North {
                    self.host.open_split_setup();
                }
                if answer == Button::South
                    && let Some(id) = self.p1_pad.take()
                {
                    self.fresh.retain(|&p| p != id);
                    self.host.split_hud.notice = Some(horizon_client::toasts::SystemToast::new(
                        "Controller removed".to_owned(),
                        Some("Press a button on it to join as a new player".to_owned()),
                    ));
                }
            }
            Hold::Nothing => {}
        }
    }

    /// `HORIZON_SPLIT_STATS=1`: every 5 s, each player's frame rate and its CPU and GPU time a
    /// frame, in the log (the dev launcher's Performance readout).
    fn print_stats(&mut self) {
        let Some(since) = self.stats else {
            return;
        };
        if since.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.stats = Some(Instant::now());
        let games =
            std::iter::once(&mut self.host).chain(self.guests.iter_mut().map(|g| &mut g.game));
        let mut line = String::new();
        for (i, game) in games.enumerate() {
            let s = game.frame_stats.summary();
            let ms = |k: &str| s[k]["avg_ms"].as_f64().unwrap_or(0.0);
            line += &format!(
                "  P{}: {:.0} fps, cpu {:.2} ms, gpu {:.2} ms",
                i + 1,
                s["avg_fps"].as_f64().unwrap_or(0.0),
                ms("cpu"),
                ms("gpu"),
            );
            game.frame_stats.reset();
        }
        log::info!("Horizon: splitscreen frames:{line}");
    }

    /// The join question on every player's view in the world (a strip at the top), or gone.
    fn prompt_join(&mut self, prompt: Option<Prompt>) {
        let views = std::iter::once(&mut self.host.split_hud).chain(
            self.guests
                .iter_mut()
                .filter(|g| g.name.is_some())
                .map(|g| &mut g.game.split_hud),
        );
        for hud in views {
            match &prompt {
                Some(p) => {
                    if hud.prompt.as_ref().is_none_or(|q| q.ask == Ask::Join) {
                        hud.prompt = Some(p.clone());
                    }
                }
                None if hud.prompt.as_ref().is_some_and(|q| q.ask == Ask::Join) => {
                    hud.prompt = None
                }
                None => {}
            }
        }
    }

    /// Player `i`'s badge says where it plays (or `note`) for a few seconds.
    fn tag(&mut self, i: usize, note: Option<&str>) {
        // A player still choosing its profile has nowhere to tell yet.
        if i > 0 && self.guests[i - 1].name.is_none() {
            return;
        }
        let note = note.map_or_else(|| self.label(i), str::to_owned);
        let place = Some((note, Instant::now() + Duration::from_secs(3)));
        if let Some(game) = self.game_mut(i) {
            game.split_hud.place = place;
        }
    }

    /// What a window's title says: its name and who plays in it.
    fn title(&self, k: usize) -> String {
        let views = &self.applied;
        let who: Vec<String> = (0..self.players())
            .filter(|&i| views.get(i).is_some_and(|v| v.0 == k))
            .map(|i| self.who(i).1)
            .collect();
        let name = match self.surfaces[k].key {
            1 if self.surfaces.len() == 1 => String::new(),
            n => format!(" - Window {n}"),
        };
        if self.players() > 1 && !who.is_empty() {
            format!("Horizon{name} · {}", who.join(", "))
        } else {
            format!("Horizon{name}")
        }
    }

    /// The windows the players' screens need: the game's (hidden while nobody plays on it), one
    /// on each monitor picked, one for each player who wants its own. True when they changed.
    fn sync_surfaces(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if self.surfaces.is_empty() {
            return false;
        }
        self.forget_empty_windows();
        let monitors = self.monitors().to_vec();
        let wanted: Vec<u8> = (0..self.players()).map(|i| self.window_of(i)).collect();
        let placements: std::collections::HashMap<u8, Placement> = self
            .windows
            .iter()
            .map(|w| (w.number, w.placement.clone()))
            .collect();
        let before = self.surfaces.len();
        for s in &self.surfaces {
            if s.key != 1
                && !wanted.contains(&s.key)
                && let Ok(at) = s.window.outer_position()
            {
                // In points: a window from a 1x TV opens there again beside a 2x screen.
                let scale = s.window.scale_factor();
                let size = s.window.inner_size().to_logical(scale);
                self.window_rects
                    .insert(s.key, (at.to_logical(scale), size));
            }
        }
        self.surfaces.retain(|s| {
            s.key == 1 || (wanted.contains(&s.key) && placements.get(&s.key) == Some(&s.placement))
        });
        let mut changed = self.surfaces.len() != before;
        // macOS would join new windows to the game's as tabs (when the system prefers tabs).
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::ActiveEventLoopExtMacOS;
            event_loop.set_allows_automatic_window_tabbing(false);
        }
        for key in &wanted {
            if self.surfaces.iter().any(|s| &s.key == key) {
                continue;
            }
            // Opened without the focus: whoever types in the game's window keeps the keyboard.
            let attributes = Window::default_attributes()
                .with_window_icon(horizon_client::window::icon())
                .with_active(false);
            // A monitor's window covers it, borderless; not the system's fullscreen, which on
            // macOS is a Space of its own that hides the other windows as it slides in.
            let placement = self.placement(*key).cloned();
            let cover = match &placement {
                Some(Placement::Fill(name)) => monitors
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, m)| cover(m)),
                _ => None,
            };
            let attributes = match cover {
                Some((at, size)) => attributes
                    .with_title("Horizon")
                    .with_decorations(false)
                    .with_position(at)
                    .with_inner_size(size),
                // A numbered window where it was last, else 854 x 480.
                None => {
                    let last = self.window_rects.get(key).copied();
                    match last {
                        Some((at, size)) => attributes.with_position(at).with_inner_size(size),
                        None => {
                            let main = self.main_monitor();
                            let home = monitors.iter().find(|(n, _)| Some(n) == main);
                            let attributes =
                                attributes.with_inner_size(LogicalSize::new(854.0, 480.0));
                            match home {
                                Some((_, m)) => attributes.with_position(centred(m)),
                                None => attributes,
                            }
                        }
                    }
                    .with_title("Horizon")
                }
            };
            let made = event_loop
                .create_window(attributes)
                .map_err(Into::into)
                .and_then(|w| {
                    let w = Arc::new(w);
                    let renderer = self.host.renderer.as_ref().ok_or("no renderer")?;
                    Ok::<_, Box<dyn std::error::Error>>((w.clone(), renderer.compositor(w)?))
                });
            if let (Ok((window, _)), Some((at, size))) = (&made, cover) {
                // macOS moves a new window under the menu bar: place it again, size first (it
                // keeps a window's bottom-left corner when it resizes).
                let _ = window.request_inner_size(size);
                window.set_outer_position(at);
            }
            match made {
                Ok((window, compositor)) => self.surfaces.push(Surface {
                    key: *key,
                    placement: placement.clone().unwrap_or(Placement::Free(None)),
                    size: window.inner_size(),
                    window,
                    compositor,
                    focused: false,
                    title: String::new(),
                }),
                Err(e) => log::warn!("Horizon: splitscreen: no Window {key}: {e}"),
            }
            changed = true;
        }
        // The game's window only while someone plays on it.
        let main_used = wanted.contains(&1);
        let main = &self.surfaces[0].window;
        if main.is_visible() != Some(main_used) {
            main.set_visible(main_used);
        }
        for k in 0..self.surfaces.len() {
            let title = self.title(k);
            if self.surfaces[k].title != title {
                self.surfaces[k].window.set_title(&title);
                self.surfaces[k].title = title;
            }
        }
        changed
    }

    /// The Splitscreen page on every player's options: who plays where. Its clicks move them.
    fn sync_page(&mut self) {
        let mut requests = std::mem::take(&mut self.host.ui.options.split_requests);
        for g in &mut self.guests {
            requests.append(&mut g.game.ui.options.split_requests);
        }
        for (i, back) in requests {
            if i < self.players() {
                self.step_screen(i, back);
            }
        }
        let rows: Vec<(String, String)> = (0..self.players())
            .map(|i| {
                let (seat, who) = self.who(i);
                (format!("P{seat} {who}"), self.label(i))
            })
            .collect();
        self.host.ui.options.split_players = rows.clone();
        for g in &mut self.guests {
            g.game.ui.options.split_players = rows.clone();
        }
    }

    /// The arrangement now, for the Setup screen: free windows where they are.
    fn draft(&self) -> layout::Draft {
        let windows = self
            .windows
            .iter()
            .map(|w| {
                let here = self.surfaces.iter().find(|s| s.key == w.number);
                let placement = match (&w.placement, here) {
                    (Placement::Free(_), Some(s)) => match s.window.outer_position() {
                        Ok(at) => {
                            // On the desktop as windows are placed: macOS in points.
                            let k = if cfg!(target_os = "macos") {
                                s.window.scale_factor()
                            } else {
                                1.0
                            };
                            let at = [(f64::from(at.x) / k) as i32, (f64::from(at.y) / k) as i32];
                            Placement::Free(Some((at, [s.size.width, s.size.height])))
                        }
                        Err(_) => w.placement.clone(),
                    },
                    (p, _) => p.clone(),
                };
                layout::Window {
                    number: w.number,
                    placement,
                }
            })
            .collect();
        let e = &self.host.settings.extras;
        layout::Draft {
            windows,
            players: (0..self.players()).map(|i| self.window_number(i)).collect(),
            two: e.splitscreen_two,
            three: e.splitscreen_three,
        }
    }

    /// The monitors for the Setup screen, left to right (as `monitor_label` numbers them), and
    /// the one Window 1 is on.
    fn setup_monitors(&self) -> (Vec<layout::Monitor>, String) {
        let monitors = self
            .monitors()
            .iter()
            .map(|(name, m)| layout::Monitor {
                name: name.clone(),
                at: [m.position().x, m.position().y],
                size: [m.size().width, m.size().height],
                scale: m.scale_factor(),
            })
            .collect();
        (monitors, self.main_monitor().cloned().unwrap_or_default())
    }

    /// The Setup screen applied: its windows, and every player in its window. Windows nobody
    /// plays in close (`forget_empty_windows`).
    fn apply(&mut self, draft: layout::Draft) {
        let mut windows: Vec<layout::Window> = draft
            .windows
            .into_iter()
            .map(|w| {
                // A free window stays where it is; a new one opens where the system puts it.
                let placement = match (&w.placement, self.placement(w.number)) {
                    (Placement::Free(_), Some(Placement::Free(p))) => Placement::Free(*p),
                    (Placement::Free(_), _) => Placement::Free(None),
                    (p, _) => p.clone(),
                };
                layout::Window {
                    number: w.number,
                    placement,
                }
            })
            .collect();
        if !windows.iter().any(|w| w.number == 1) {
            windows.insert(
                0,
                layout::Window {
                    number: 1,
                    placement: Placement::Free(None),
                },
            );
        }
        self.windows = windows;
        // Players who joined while it was open stay where they are.
        for (i, &number) in draft.players.iter().enumerate().take(self.players()) {
            let number = if self.placement(number).is_some() {
                number
            } else {
                1
            };
            self.set_window(i, number);
        }
        // P1's options file: its turn.
        self.turn(0);
        let e = &mut self.host.settings.extras;
        if (e.splitscreen_two, e.splitscreen_three) != (draft.two, draft.three) {
            e.splitscreen_two = draft.two;
            e.splitscreen_three = draft.three;
            self.host.settings.save(&Game::options_path());
        }
    }

    /// The Setup screen on any player's options: the arrangement now, the monitors and the
    /// players' names; Apply arranges them. True when it did.
    fn sync_setup(&mut self) -> bool {
        use horizon_client::options_ui::Page;
        let open = |g: &Game<'_>| g.ui.options.page() == Page::SplitSetup;
        if !(open(&self.host) || self.guests.iter().any(|g| open(&g.game))) {
            return false;
        }
        let live = self.draft();
        let (monitors, main) = self.setup_monitors();
        let names: Vec<String> = (0..self.players())
            .map(|i| {
                let (seat, who) = self.who(i);
                if self.lost.iter().any(|l| l.seat == seat) {
                    format!("P{seat} {who} (no controller)")
                } else {
                    format!("P{seat} {who}")
                }
            })
            .collect();
        let seats: Vec<usize> = (0..self.players()).map(|i| self.who(i).0).collect();
        let moved = std::mem::replace(&mut self.setup_seats, seats) != self.setup_seats;
        let (mut applied, mut identify) = (Vec::new(), false);
        let games =
            std::iter::once(&mut self.host).chain(self.guests.iter_mut().map(|g| &mut g.game));
        for (i, game) in games.enumerate() {
            let setup = &mut game.ui.options.split_setup;
            // Someone joined or left: every draft would put the players in the wrong places.
            if moved {
                setup.reset();
            }
            if let Some(draft) = setup.apply.take().filter(|_| i == 0) {
                applied.push((i, Ok(draft)));
            }
            if let Some(spot) = setup.apply_move.take().filter(|_| i > 0) {
                applied.push((i, Err(spot)));
            }
            identify |= std::mem::take(&mut setup.identify);
            // P1 arranges everyone; the others, themselves.
            setup.me = i;
            setup.host = i == 0;
            setup.live = Some(live.clone());
            setup.monitors = monitors.clone();
            setup.main = main.clone();
            setup.names = names.clone();
        }
        let changed = !applied.is_empty();
        for (i, change) in applied {
            match change {
                Ok(draft) => self.apply(draft),
                // Another player's move: a window it named may have closed since.
                Err(Spot::Window(n)) if self.placement(n).is_none() => {
                    self.choose(i, Spot::Window(1))
                }
                Err(spot) => self.choose(i, spot),
            }
        }
        // Every view says where it is: after a change, or asked.
        if changed || identify {
            for i in 0..self.players() {
                self.tag(i, None);
            }
        }
        changed
    }

    /// Splitscreen turned on: P1 renders offscreen on the same device and a compositor presents
    /// on its window; the world is meshed again for the new renderer.
    fn enable(&mut self) -> Result<()> {
        let window = self.host.window.clone().ok_or("no window")?;
        let mut old = self.host.renderer.take().ok_or("no renderer")?;
        old.release_surface();
        let api = self.host.settings.horizon.api;
        let built = build_renderer(
            window.clone(),
            true,
            &self.host.pack,
            &self.host.gui,
            Some(&old),
            None,
            api,
        );
        let renderer = match built {
            Ok(r) => r,
            Err(e) => {
                old.recreate_surface()?;
                self.host.renderer = Some(old);
                return Err(e);
            }
        };
        // A window it can't present on: P1 keeps drawing on it as before.
        let compositor = match old.compositor(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                drop(renderer);
                old.recreate_surface()?;
                self.host.renderer = Some(old);
                return Err(e);
            }
        };
        self.host.renderer = Some(renderer);
        self.host.options.offscreen = true;
        self.host.options.paced_by_split = true;
        // The new renderer takes the old one's present mode, settings and all: P1's again.
        self.host.apply_settings();
        self.host.clear_lod();
        self.host.all_sections_changed();
        self.surfaces = vec![Surface {
            key: 1,
            placement: Placement::Free(None),
            size: window.inner_size(),
            window,
            compositor,
            focused: true,
            title: String::new(),
        }];
        self.enabled = true;
        Ok(())
    }

    /// Splitscreen turned off: the others leave, and P1 draws on its window again.
    fn disable(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        // P1's own buttons and focus targets back first: the others' go with them.
        for k in (0..self.guests.len()).rev() {
            self.remove_guest(k);
        }
        self.host.pad.feed = None;
        self.surfaces.truncate(1);
        let Some(main) = self.surfaces.pop() else {
            self.enabled = false;
            return Ok(());
        };
        main.window.set_visible(true);
        let old = self.host.renderer.take().ok_or("no renderer")?;
        old.take_back(main.compositor);
        let api = self.host.settings.horizon.api;
        let built = build_renderer(
            main.window.clone(),
            false,
            &self.host.pack,
            &self.host.gui,
            Some(&old),
            None,
            api,
        );
        let renderer = match built {
            Ok(r) => r,
            Err(e) => {
                // P1 stays offscreen, presented as before (alone now).
                let compositor = old.compositor(main.window.clone())?;
                self.host.renderer = Some(old);
                self.surfaces.push(Surface { compositor, ..main });
                return Err(e);
            }
        };
        drop(old);
        self.host.renderer = Some(renderer);
        self.host.window = Some(main.window.clone());
        self.host.options.offscreen = false;
        self.host.options.paced_by_split = false;
        // From the offscreen renderer it took no vsync: P1's settings again.
        self.host.apply_settings();
        self.host.clear_lod();
        self.host.all_sections_changed();
        let id = main.window.id();
        self.host.window_event(
            event_loop,
            id,
            WindowEvent::Resized(main.window.inner_size()),
        );
        self.enabled = false;
        Ok(())
    }

    /// P1 is playing in a world others can join.
    fn in_world(&self) -> bool {
        self.host.title.is_none() && self.host.integrated.is_some() && self.host.player.is_some()
    }

    /// The pad's state for its player: nothing while the press that joined it is still held.
    fn pad_state(&mut self, pad: Option<PadId>) -> PadState {
        let Some(id) = pad else {
            return Default::default();
        };
        let state = self.pads.state(id);
        if self.fresh.contains(&id) {
            if state.buttons != 0 {
                return Default::default();
            }
            self.fresh.retain(|&p| p != id);
        }
        state
    }

    /// Controllers: P1's is the first used in a menu; in a world another one is offered to join
    /// (A), or to drive P1 (Y) while P1 has none. The pad whose player should join now.
    fn route_pads(&mut self) -> Vec<PadId> {
        let now = Instant::now();
        let mut joins = Vec::new();
        let mut presses = Vec::new();
        for event in self.pads.events() {
            match event {
                PadEvent::Press(id, button) => presses.push((id, button)),
                PadEvent::Gone(id) => self.pad_gone(id),
                PadEvent::Back(id) => {
                    if let Some(k) = self.lost.iter().position(|l| l.id == id) {
                        let seat = self.lost.remove(k).seat;
                        self.give_pad(seat, id);
                    }
                }
            }
        }
        self.forget_lost();
        for (id, button) in presses {
            if Some(id) == self.p1_pad || self.guests.iter().any(|g| g.pad == Some(id)) {
                continue;
            }
            // Pressed, then gone in the same frame.
            if !self.pads.connected(id) {
                continue;
            }
            if !self.in_world() {
                if self.p1_pad.is_none() {
                    self.p1_pad = Some(id);
                    self.fresh.push(id);
                }
                continue;
            }
            if self.claim_for_lost(id, button) {
                continue;
            }
            match self.offers.iter().position(|(o, _)| *o == id) {
                Some(k) => match button {
                    // Two pressing A together both join, in the order they pressed.
                    Button::South => {
                        self.offers.remove(k);
                        joins.push(id);
                    }
                    Button::North if self.p1_pad.is_none() => {
                        self.offers.remove(k);
                        self.p1_pad = Some(id);
                        self.fresh.push(id);
                    }
                    Button::East => {
                        self.offers.remove(k);
                    }
                    _ => {}
                },
                // An offer for each controller while seats are left.
                None if self.players() + joins.len() + self.offers.len() < MAX_PLAYERS => {
                    self.offers.push((id, now));
                    self.ask_to_join();
                }
                None => {}
            }
        }
        let in_world = self.in_world();
        let before = self.offers.len();
        self.offers
            .retain(|(_, since)| now - *since <= OFFER && in_world);
        if self.offers.is_empty() {
            self.prompt_join(None);
        } else if self.offers.len() != before || !joins.is_empty() {
            self.ask_to_join();
        }
        // Each player's glyphs follow its own controller (its game never reads the pads itself).
        if let Some(id) = self.p1_pad {
            self.host.pad.style = self.pads.style(id);
        }
        for g in &mut self.guests {
            if let Some(id) = g.pad {
                g.game.pad.style = self.pads.style(id);
            }
        }
        let p1 = self.pad_state(self.p1_pad);
        // Asked about its controller, P1's buttons answer that, not the game.
        self.host.pad.feed = Some(if self.p1_hold.asking {
            PadState::default()
        } else {
            p1
        });
        for i in 0..self.guests.len() {
            let state = self.pad_state(self.guests[i].pad);
            // Asked whether to leave, its buttons answer that, not the game.
            let state = if self.guests[i].hold.asking {
                PadState::default()
            } else {
                state
            };
            self.guests[i].game.pad.feed = Some(state);
        }
        joins
    }

    /// A controller disconnected: its offer, rejoin and fresh press go; its player waits for one
    /// (P1 keeps the keyboard and mouse; a player picking a profile leaves).
    fn pad_gone(&mut self, id: PadId) {
        self.fresh.retain(|&p| p != id);
        self.rejoin.retain(|(p, _)| *p != Some(id));
        let offered = self.offers.len();
        self.offers.retain(|(p, _)| *p != id);
        if self.offers.len() != offered && !self.offers.is_empty() {
            self.ask_to_join();
        }
        let uuid = self.pads.uuid(id);
        if self.p1_pad == Some(id) {
            self.p1_pad = None;
            self.p1_hold = HoldSelect::default();
            if self
                .host
                .split_hud
                .prompt
                .as_ref()
                .is_some_and(|p| p.ask == Ask::Controller)
            {
                self.host.split_hud.prompt = None;
            }
            if self.in_world() {
                self.lost.push(Lost { seat: 1, id, uuid });
                self.host.split_hud.notice = Some(horizon_client::toasts::SystemToast::new(
                    "Controller disconnected".to_owned(),
                    Some("Reconnect it, or press a button on another".to_owned()),
                ));
            }
            return;
        }
        let Some(g) = self.guests.iter_mut().find(|g| g.pad == Some(id)) else {
            return;
        };
        g.pad = None;
        g.hold = HoldSelect::default();
        if g.name.is_none() {
            // Still picking a profile: as if it pressed Cancel.
            if let Some(title) = g.game.title.as_mut() {
                title.escape();
            }
            return;
        }
        self.lost.push(Lost {
            seat: g.seat,
            id,
            uuid,
        });
        g.game.split_hud.prompt = Some(lost_prompt(g.seat));
    }

    /// Player in `seat` plays with controller `id` (back, or another): its question goes.
    fn give_pad(&mut self, seat: usize, id: PadId) {
        self.fresh.push(id);
        let hud = if seat == 1 {
            self.p1_pad = Some(id);
            &mut self.host.split_hud
        } else {
            let Some(g) = self.guests.iter_mut().find(|g| g.seat == seat) else {
                return;
            };
            g.pad = Some(id);
            &mut g.game.split_hud
        };
        if hud.prompt.as_ref().is_some_and(|p| p.ask == Ask::Lost) {
            hud.prompt = None;
        }
        hud.notice = Some(horizon_client::toasts::SystemToast::new(
            "Controller connected".to_owned(),
            None,
        ));
    }

    /// A button on a controller nobody has while players wait for one: true when it answered
    /// them (`claim`).
    fn claim_for_lost(&mut self, id: PadId, button: Button) -> bool {
        match claim(&self.lost, self.pads.uuid(id), button) {
            Claim::Give(k) => {
                let seat = self.lost.remove(k).seat;
                self.give_pad(seat, id);
            }
            Claim::Leave(k) => {
                let seat = self.lost.remove(k).seat;
                if let Some(g) = self.guests.iter_mut().find(|g| g.seat == seat) {
                    g.game.split_hud.prompt = None;
                    if let Err(e) = g.game.leave_server(None) {
                        log::warn!("Horizon: splitscreen: {e}");
                    }
                }
            }
            Claim::Wait => {}
            Claim::Pass => return false,
        }
        true
    }

    /// Nobody waits for a player who left or has a controller again (another way), nor for P1
    /// out of a world.
    fn forget_lost(&mut self) {
        let p1 = self.in_world() && self.p1_pad.is_none();
        let guests = &self.guests;
        self.lost.retain(|l| match l.seat {
            1 => p1,
            seat => guests.iter().any(|g| g.seat == seat && g.pad.is_none()),
        });
    }

    /// Every player sees the controllers waiting to join, whatever it is doing.
    fn ask_to_join(&mut self) {
        let title = match self.offers.as_slice() {
            [(id, _)] => format!("{} wants to join", self.pads.name(*id)),
            offers => format!("{} controllers want to join", offers.len()),
        };
        let mut choices = vec![(Button::South, "Join")];
        if self.p1_pad.is_none() {
            choices.push((Button::North, "Play as P1"));
        }
        choices.push((Button::East, "No"));
        self.prompt_join(Some(Prompt {
            ask: Ask::Join,
            title,
            choices,
        }));
    }

    /// Add a player driven by `pad`: it picks a profile and where to play in its view first,
    /// unless `name` is given (tests).
    fn join(&mut self, pad: Option<PadId>, name: Option<String>) -> Result<()> {
        let seat = self.free_seat().ok_or("every seat is taken")?;
        // Until it has a profile, a scratch options file: never P1's.
        let options = Self::folder()?.join(".joining/options.txt");
        OPTIONS.set(Some(options.clone()));
        let game = self.start_guest(seat, &options);
        OPTIONS.set(None);
        let mut game = game?;
        if let Some(pad) = pad {
            self.fresh.push(pad);
        }
        let profiles = Self::folder()?.join("profiles");
        let mut names: Vec<String> = std::fs::read_dir(&profiles)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| horizon_client::title::valid_profile_name(n))
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        let taken = self.taken();
        if let Some(title) = game.title.as_mut() {
            title.profile_picker(names, taken);
        }
        self.guests.push(Guest {
            game,
            name: None,
            seat,
            window: 1,
            pad,
            options,
            widgets: Default::default(),
            bumpers: [false; 2],
            hold: HoldSelect::default(),
        });
        if let Some(name) = name {
            self.enter(self.guests.len() - 1, name)?;
        }
        Ok(())
    }

    /// The names playing now: P1's and the joined players'.
    fn taken(&self) -> Vec<String> {
        std::iter::once(self.host.options.player_name())
            .chain(self.guests.iter().filter_map(|g| g.name.clone()))
            .collect()
    }

    /// Guest `i` plays as profile `name` where its view is now (moved with the bumpers): its
    /// options, then into P1's world.
    fn enter(&mut self, i: usize, name: String) -> Result<()> {
        if self.taken().contains(&name) {
            return Err(format!("{name} is already playing").into());
        }
        let profile = Self::folder()?.join(format!("profiles/{name}"));
        let options = profile.join("options.txt");
        let mut settings = if options.exists() {
            horizon_client::settings::Settings::load(&options)
        } else {
            std::fs::create_dir_all(&profile)?;
            self.host.settings.save(&options);
            self.host.settings.clone()
        };
        let pipe = self
            .host
            .integrated
            .as_ref()
            .and_then(|server| server.join_split(&name))
            .ok_or("P1's world is not on its server thread")?;
        self.guests[i].options = options;
        self.turn(i + 1);
        let guest = &mut self.guests[i];
        settings.extras.agent_control &= guest.game.options.control.is_some();
        guest.game.settings = settings;
        guest.game.options.player_name = Some(name.clone());
        guest.game.apply_settings();
        let joined = guest.game.connect_local(pipe, &name);
        self.turn(0);
        if let Err(e) = joined {
            // Let in for nothing: the world pauses again, the name is free.
            if let Some(server) = &self.host.integrated {
                server.leave_split(&name);
            }
            return Err(e);
        }
        self.guests[i].name = Some(name);
        Ok(())
    }

    /// Players picking a profile: into the world with it, or gone when they leave.
    fn pick_profiles(&mut self) -> bool {
        let mut changed = false;
        let mut i = 0;
        while i < self.guests.len() {
            let choice = match self.guests[i].name {
                None => self.guests[i]
                    .game
                    .title
                    .as_mut()
                    .and_then(|t| t.take_profile_choice()),
                Some(_) => None,
            };
            let leave = match choice {
                Some(Some(name)) => match self.enter(i, name) {
                    Ok(()) => {
                        changed = true;
                        false
                    }
                    Err(e) => {
                        log::warn!("Horizon: splitscreen: {e}");
                        true
                    }
                },
                Some(None) => true,
                None => false,
            };
            if leave {
                self.remove_guest(i);
                changed = true;
            } else {
                i += 1;
            }
        }
        changed
    }

    /// A new player's game in its seat, on its title screen until it has a profile.
    fn start_guest(&mut self, seat: usize, options_file: &Path) -> Result<Game<'static>> {
        // The players' games borrow P1's resources for the process's lifetime: one copy for all.
        if self.res.is_none() {
            self.res = Some(Box::leak(Box::new(self.host.res.clone())));
        }
        let res = self.res.ok_or("no resources")?;
        let host = &mut self.host;
        std::fs::create_dir_all(options_file.parent().ok_or("bad options path")?)?;
        let mut options = host.options.clone();
        options.world = None;
        options.server = None;
        options.split_guest = true;
        // P1's socket (`--control` or Agent Control) + the seat: every game in this process
        // would otherwise take the one default endpoint, P1's.
        options.control = host.control.as_ref().map(|c| {
            let mut p = c.path().as_os_str().to_owned();
            p.push(seat.to_string());
            p.into()
        });
        let mut settings = host.settings.clone();
        settings.extras.agent_control &= options.control.is_some();
        let gui = UiResources::load(&options.resources, &options.resources)?;
        // P1's output, heard from this player's camera too (`Audio::share`).
        let audio = host.audio.share(seat as u8);
        let window = host.window.clone().ok_or("no window")?;
        let mut guest = start_game(
            res,
            host.pack.clone(),
            gui,
            options,
            None,
            audio,
            host.options.resources.clone(),
            Instant::now(),
            None,
            Some(settings),
            None,
        )?
        .ok_or("the player's game did not start")?;
        guest.renderer = Some(build_renderer(
            window.clone(),
            true,
            &guest.pack,
            &guest.gui,
            host.renderer.as_ref(),
            None,
            host.settings.horizon.api,
        )?);
        guest.window = Some(window);
        guest.world_renderer_ready = true;
        guest.apply_settings();
        Ok(guest)
    }

    /// Players who disconnected drop out; all of them once P1 has left the world.
    fn drop_out(&mut self) -> bool {
        let p1_left = self.host.title.is_some() || self.host.integrated.is_none();
        let before = self.guests.len();
        for k in (0..self.guests.len()).rev() {
            let g = &self.guests[k];
            // A player picking a profile has no connection yet.
            let playing = g.name.is_none() || g.game.remote.is_some();
            if p1_left || !playing || g.game.error.is_some() {
                self.remove_guest(k);
            }
        }
        self.guests.len() != before
    }

    /// Guest `k` (player `k + 1`) leaves: keyboard and mouse stay with whoever had them (P1 if it
    /// was this one), the views keep their order for the layout's comparison, and the server
    /// forgets the name (the world may pause again).
    fn remove_guest(&mut self, k: usize) {
        // Its turn's buttons and options go back to their owners before the order shifts.
        self.turn(0);
        let guest = self.guests.remove(k);
        let i = k + 1;
        if self.focus == i {
            self.focus = 0;
        } else if self.focus > i {
            self.focus -= 1;
        }
        if i < self.applied.len() {
            self.applied.remove(i);
        }
        if let (Some(server), Some(name)) = (&self.host.integrated, &guest.name) {
            server.leave_split(name);
        }
    }

    /// A window other than Window 1 was closed: its players leave, P1 goes back to Window 1.
    fn close_window(&mut self, surface: usize) -> Result<()> {
        let views = self.views();
        for i in (0..self.players()).rev() {
            if views.get(i).is_none_or(|v| v.0 != surface) {
                continue;
            }
            if i == 0 {
                self.p1_window = 1;
                continue;
            }
            let guest = &mut self.guests[i - 1];
            if guest.game.remote.is_some() {
                guest.game.leave_server(None)?;
            } else if let Some(title) = guest.game.title.as_mut() {
                // Still picking a profile: as if it pressed Cancel.
                title.escape();
            }
        }
        Ok(())
    }
}

impl ApplicationHandler for Split<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.host.resumed(event_loop);
        if !self.enabled {
            return;
        }
        let (Some(window), Some(renderer)) = (&self.host.window, &self.host.renderer) else {
            return;
        };
        if self.surfaces.is_empty() {
            match renderer.compositor(window.clone()) {
                Ok(compositor) => self.surfaces.push(Surface {
                    key: 1,
                    placement: Placement::Free(None),
                    window: window.clone(),
                    compositor,
                    size: window.inner_size(),
                    focused: true,
                    title: String::new(),
                }),
                Err(e) => {
                    self.host.error = Some(e);
                    event_loop.exit();
                    return;
                }
            }
        }
        self.relayout(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if !self.enabled {
            return self.host.window_event(event_loop, id, event);
        }
        let Some(surface) = self.surfaces.iter().position(|s| s.window.id() == id) else {
            return;
        };
        match event {
            WindowEvent::Resized(size) => {
                let s = &mut self.surfaces[surface];
                s.size = size;
                s.compositor.resize(size.width, size.height);
                self.relayout(event_loop);
            }
            // The views draw themselves offscreen; the compositors present in about_to_wait.
            WindowEvent::RedrawRequested => {}
            // The game's window: P1's to handle (it saves the world, then quits); a player's game
            // would end the event loop before that. Another window: its players leave.
            WindowEvent::CloseRequested if surface == 0 => {
                if let Some((host, id)) = self.player(0) {
                    host.window_event(event_loop, id, WindowEvent::CloseRequested);
                }
            }
            WindowEvent::CloseRequested => {
                if let Err(e) = self.close_window(surface) {
                    log::warn!("Horizon: splitscreen: {e}");
                }
            }
            // Players' pads count while any of the windows has focus.
            WindowEvent::Focused(focused) => {
                self.surfaces[surface].focused = focused;
                let any = self.surfaces.iter().any(|s| s.focused);
                for i in 0..self.players() {
                    if let Some((player, id)) = self.player(i) {
                        player.window_event(event_loop, id, WindowEvent::Focused(any));
                    }
                }
            }
            WindowEvent::CursorMoved {
                device_id,
                position,
            } => {
                self.cursor = position;
                let position = self.cursor_in_focus();
                let focus = self.focus;
                if self.views().get(focus).is_some_and(|v| v.0 == surface)
                    && let Some((player, id)) = self.player(focus)
                {
                    player.window_event(
                        event_loop,
                        id,
                        WindowEvent::CursorMoved {
                            device_id,
                            position,
                        },
                    );
                }
            }
            WindowEvent::MouseInput {
                device_id,
                state: ElementState::Pressed,
                ..
            } => {
                // A click on another player's view gives it keyboard and mouse, unless the
                // focused player holds the mouse (playing): Esc frees it first, as in vanilla.
                let (x, y) = (self.cursor.x as u32, self.cursor.y as u32);
                let clicked = self.views().iter().position(|(s, [vx, vy], size)| {
                    *s == surface
                        && (*vx..vx + size.width).contains(&x)
                        && (*vy..vy + size.height).contains(&y)
                });
                if let Some(clicked) = clicked
                    && clicked != self.focus
                    && !self.captured(self.focus)
                {
                    self.focus = clicked;
                    self.tag(clicked, Some("Keyboard and mouse"));
                    let position = self.cursor_in_focus();
                    if let Some((player, id)) = self.player(clicked) {
                        player.window_event(
                            event_loop,
                            id,
                            WindowEvent::CursorMoved {
                                device_id,
                                position,
                            },
                        );
                    }
                }
                let focus = self.focus;
                if let Some((player, id)) = self.player(focus) {
                    player.window_event(event_loop, id, event);
                }
            }
            _ => {
                let focus = self.focus;
                if let Some((player, id)) = self.player(focus) {
                    player.window_event(event_loop, id, event);
                }
            }
        }
        self.turn(0);
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, id: DeviceId, event: DeviceEvent) {
        let focus = if self.enabled { self.focus } else { 0 };
        if let Some((player, _)) = self.player(focus) {
            player.device_event(event_loop, id, event);
        }
        self.turn(0);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.refresh_monitors();
        // The Splitscreen switch, turned on or off in P1's settings: applied at once.
        if self.host.settings.extras.splitscreen != self.enabled && self.host.window.is_some() {
            let switched = if self.enabled {
                self.disable(event_loop)
            } else {
                self.enable()
            };
            match switched {
                Ok(()) if self.enabled => self.relayout(event_loop),
                Ok(()) => {}
                Err(e) => {
                    log::warn!("Horizon: splitscreen could not switch: {e}");
                    self.host.settings.extras.splitscreen = self.enabled;
                }
            }
        }
        if !self.enabled {
            return self.host.about_to_wait(event_loop);
        }
        let pad_joins = self.route_pads();
        self.turn(0);
        self.host.about_to_wait(event_loop);
        let mut changed = self.drop_out();
        // Switching profile: choosing again, where it played.
        for (pad, window) in std::mem::take(&mut self.rejoin) {
            if self.guests.iter().any(|g| g.pad.is_some() && g.pad == pad) {
                // Not gone yet: next frame.
                self.rejoin.push((pad, window));
                continue;
            }
            match self.join(pad, None) {
                Ok(()) => {
                    // Its window again: still open, or opened again as it was.
                    let number = window.number;
                    if self.placement(number).is_none() {
                        self.windows.push(window);
                    }
                    if let Some(g) = self.guests.last_mut() {
                        g.window = number;
                    }
                    changed = true;
                }
                Err(e) => log::warn!("Horizon: splitscreen: a player could not come back: {e}"),
            }
        }
        for pad in pad_joins {
            match self.join(Some(pad), None) {
                Ok(()) => changed = true,
                Err(e) => log::warn!("Horizon: splitscreen: a player could not join: {e}"),
            }
        }
        changed |= self.pick_profiles();
        // Testing: players without controllers, one per frame, once.
        if self.in_world() && self.test_players > 0 {
            // HORIZON_SPLIT_PICK: they pick a profile like a controller's player, else P2...
            let name = std::env::var_os("HORIZON_SPLIT_PICK")
                .is_none()
                .then(|| format!("P{}", self.free_seat().unwrap_or(0)));
            match self.join(None, name) {
                Ok(()) => {
                    self.test_players -= 1;
                    changed = true;
                }
                Err(e) => {
                    self.host.error = Some(e);
                    event_loop.exit();
                    return;
                }
            }
        }
        self.sync_page();
        changed |= self.sync_setup();
        changed |= self.sync_surfaces(event_loop);
        // Also a new arrangement from the settings (a screen, Two on a screen).
        if changed || self.views() != self.applied {
            self.relayout(event_loop);
        }
        // Badges while others play too.
        let together = self.players() > 1;
        self.host.split_hud.badge = together.then(|| self.host.options.player_name());
        for g in &mut self.guests {
            g.game.split_hud.badge = Some(g.name.clone().unwrap_or_else(|| "New player".into()));
        }
        let master = self.host.audio.master;
        for i in 0..self.guests.len() {
            self.turn(i + 1);
            // One set of speakers: P1's volume.
            self.guests[i].game.audio.master = master;
            self.guests[i].game.about_to_wait(event_loop);
            // A player's game stops the event loop on an error, as P1's does: say why.
            if let Some(e) = &self.guests[i].game.error {
                log::error!("Horizon: splitscreen: seat {}: {e}", self.guests[i].seat);
            }
            // Choosing its profile, the bumpers move its view (its pad as its game read it: the
            // controller, or `horizon-ctl pad`).
            if self.guests[i].name.is_none() {
                let state = self.guests[i].game.pad.state;
                let held = [
                    state.held(Button::LeftBumper),
                    state.held(Button::RightBumper),
                ];
                let before = std::mem::replace(&mut self.guests[i].bumpers, held);
                if held[0] && !before[0] {
                    self.place_joining(i + 1, true);
                } else if held[1] && !before[1] {
                    self.place_joining(i + 1, false);
                }
            } else {
                self.leave_by_controller(i + 1);
            }
        }
        self.turn(0);
        self.p1_holds_select();
        // Each window shows its players' frames. Only the game's waits for the display (vsync):
        // windows each waiting would divide the rate.
        let views = self.views();
        let renderers: Vec<_> = std::iter::once(&self.host.renderer)
            .chain(self.guests.iter().map(|g| &g.game.renderer))
            .collect();
        let vsync = self.host.settings.vsync;
        for (k, surface) in self.surfaces.iter_mut().enumerate() {
            let frames: Vec<_> = renderers
                .iter()
                .zip(&views)
                .filter(|(_, v)| v.0 == k)
                .filter_map(|(r, v)| Some((r.as_ref()?.offscreen_frame()?, v.1)))
                .collect();
            surface.compositor.set_vsync(vsync && k == 0);
            surface.compositor.present(&frames);
        }
        // The players' GPU work waits here, once every player has submitted: one player's CPU
        // work overlapped the GPU drawing another's.
        let renderers = std::iter::once(&mut self.host.renderer)
            .chain(self.guests.iter_mut().map(|g| &mut g.game.renderer));
        for renderer in renderers.flatten() {
            renderer.paced_elsewhere = true;
            renderer.wait_in_flight();
        }
        self.print_stats();
        // One pace for all: the fastest any player wants, so an idle one (vanilla's AFK limit)
        // can't slow the others. With vsync the display paces anything at or above its refresh
        // rate; the limiter on top would miss refreshes.
        let limit = std::iter::once(self.host.framerate_limit())
            .chain(self.guests.iter().map(|g| g.game.framerate_limit()))
            .max()
            .unwrap_or(60);
        let refresh_hz = self
            .surfaces
            .first()
            .and_then(|s| s.window.current_monitor()?.refresh_rate_millihertz())
            .map_or(60, |mhz| mhz.div_ceil(1000) as i32);
        if !(vsync && limit >= refresh_hz) {
            horizon_client::window::end_frame(&mut self.limiter, limit, &mut self.clock);
        }
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        for i in (0..self.players()).rev() {
            if let Some((player, _)) = self.player(i) {
                player.exiting(event_loop);
            }
        }
        self.turn(0);
    }
}

impl Game<'_> {
    /// The Splitscreen Setup screen, from the controller: Done goes back to playing.
    fn open_split_setup(&mut self) {
        if self.screen == super::Screen::Playing && self.title.is_none() {
            self.capture(false);
        }
        self.open_options_at(false, horizon_client::options_ui::Page::SplitSetup);
    }

    /// An error stops the event loop, as ever, but not in another splitscreen player's game:
    /// that player drops out (`Split::drop_out`), the others play on.
    pub(super) fn exit_on_error(&self, event_loop: &ActiveEventLoop) {
        if !self.options.split_guest {
            event_loop.exit();
        }
    }

    /// Splitscreen over this player's view, above the other toasts (`ToastManager`'s slots).
    pub(super) fn draw_split_hud(&mut self, batch: &mut HudBatch, slot: usize) {
        let atlas = &self.gui.atlas;
        let (w, h) = (batch.width, batch.height);
        let width = |text: &str| HudBatch::text_width(atlas, text).ceil();
        // The badge, top left: who, and a moment where.
        if let Some(badge) = &self.split_hud.badge {
            let place = self.split_hud.place.as_ref();
            let text = match place.filter(|(_, until)| Instant::now() < *until) {
                Some((place, _)) => format!("{badge} · {place}"),
                None => badge.clone(),
            };
            batch.text(atlas, &text, 4.0, 4.0, rgba(255, 255, 255, 230), true);
        }
        // A question: centred over the view dimmed, or a strip at the top (a controller joining).
        if let Some(prompt) = &self.split_hud.prompt {
            let style = self.ui.options.pad_style;
            let dim = prompt.ask != Ask::Join;
            if dim {
                batch.fill(atlas, 0.0, 0.0, w, h, rgba(0, 0, 0, 120));
            }
            let glyphs: Vec<_> = prompt
                .choices
                .iter()
                .map(|&(b, what)| (b.label(style), b.color(style), what))
                .collect();
            let line: f32 = glyphs
                .iter()
                .map(|(g, _, what)| width(g) + 8.0 + width(what) + 12.0)
                .sum::<f32>()
                - 12.0;
            let pw = width(&prompt.title).max(line) + 16.0;
            let ph = 34.0;
            let (px, py) = (
                ((w - pw) / 2.0).floor(),
                if dim { ((h - ph) / 2.0).floor() } else { 18.0 },
            );
            batch.fill(atlas, px, py, pw, ph, rgba(0, 0, 0, 210));
            let edge = rgba(255, 255, 255, 90);
            for [x, y, fw, fh] in [
                [px, py, pw, 1.0],
                [px, py + ph - 1.0, pw, 1.0],
                [px, py, 1.0, ph],
                [px + pw - 1.0, py, 1.0, ph],
            ] {
                batch.fill(atlas, x, y, fw, fh, edge);
            }
            let tx = (px + (pw - width(&prompt.title)) / 2.0).floor();
            batch.text(
                atlas,
                &prompt.title,
                tx,
                py + 6.0,
                rgba(255, 255, 255, 255),
                true,
            );
            let mut x = (px + (pw - line) / 2.0).floor();
            let y = py + 20.0;
            for (glyph, [r, g, b], what) in glyphs {
                // The button as printed on the pad, in its colour, on a dark key.
                let gw = width(glyph) + 4.0;
                batch.fill(atlas, x - 1.0, y - 2.0, gw, 11.0, rgba(40, 40, 40, 255));
                batch.text(atlas, glyph, x + 1.0, y, rgba(r, g, b, 255), false);
                x += gw + 4.0;
                batch.text(atlas, what, x, y, rgba(220, 220, 220, 255), false);
                x += width(what) + 12.0;
            }
        }
        // Notices nobody answers: a toast.
        let multiplier = f64::from(self.settings.notification_display_time);
        let Some(notice) = &mut self.split_hud.notice else {
            return;
        };
        let (_, finished) = notice.update(multiplier);
        if finished {
            self.split_hud.notice = None;
            return;
        }
        notice.draw(&self.gui.atlas, batch, 32.0 * slot as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAD: [u8; 16] = [7; 16];
    const OTHER: [u8; 16] = [9; 16];

    fn lost(seat: usize, uuid: [u8; 16]) -> Lost<usize> {
        Lost {
            seat,
            id: seat,
            uuid,
        }
    }

    #[test]
    fn the_same_controller_back_takes_its_player_with_any_button() {
        let waiting = [lost(2, OTHER), lost(3, PAD)];
        assert_eq!(claim(&waiting, PAD, Button::DPadUp), Claim::Give(1));
        // P1's own comes back too.
        assert_eq!(claim(&[lost(1, PAD)], PAD, Button::West), Claim::Give(0));
    }

    #[test]
    fn another_controller_takes_over_or_lets_go_the_longest_waiting() {
        let waiting = [lost(1, OTHER), lost(3, OTHER), lost(2, OTHER)];
        assert_eq!(claim(&waiting, PAD, Button::South), Claim::Give(1));
        assert_eq!(claim(&waiting, PAD, Button::East), Claim::Leave(1));
        // Any other button: the others wait, nobody is offered to join meanwhile.
        assert_eq!(claim(&waiting, PAD, Button::North), Claim::Wait);
    }

    #[test]
    fn only_p1_waiting_leaves_the_offer_to_join() {
        assert_eq!(claim(&[lost(1, OTHER)], PAD, Button::South), Claim::Pass);
        assert_eq!(claim::<usize>(&[], PAD, Button::South), Claim::Pass);
    }

    #[test]
    fn a_controller_the_system_does_not_describe_is_not_known_back() {
        let unknown = [0; 16];
        assert_eq!(
            claim(&[lost(2, unknown)], unknown, Button::West),
            Claim::Wait
        );
    }
}
