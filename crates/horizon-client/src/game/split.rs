//! Splitscreen (Horizon's Splitscreen page, off by default): players drop in and out of P1's world
//! on this computer, as on a console. P1 is the game as launched, with the keyboard, the mouse and
//! the first controller used in a menu. In a world, another controller pressing a button is
//! offered to join; pressing A adds a player, another `Game` in this process that joins P1's
//! server thread through memory, after it picks a profile and where to play in its view
//! (`title/profiles.rs`). A player who disconnects (pause menu) drops out; when P1 leaves the
//! world, all do.
//!
//! Each player plays in a window: Window 1 (the game's), Window 2, 3... (opened when someone picks
//! New window, closed with its last player), or a monitor's (borderless over it). Players in one
//! window share it: halves for two (side by side or top and bottom), three one wide and two (or
//! in columns), quarters for four.
//! A profile remembers its screen (`profiles/<name>/screen.txt`, P1's `p1-screen.txt`); the
//! Splitscreen page changes anyone's. Every player renders offscreen on one device and each
//! window's compositor copies its players' frames. Turning splitscreen on rebuilds P1's renderer
//! that way at once (the world is meshed again: a moment, in the menu); off, back.
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
    pad::{PadId, PadRouter},
    start_game,
};
use horizon_client::{
    Result,
    gamepad::{Button, PadState},
    gfx::compositor::Compositor,
    text::Text,
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
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{DeviceEvent, DeviceId, ElementState, WindowEvent},
    event_loop::ActiveEventLoop,
    monitor::MonitorHandle,
    window::{Window, WindowId},
};

const MAX_PLAYERS: usize = 4;
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

/// Where a player plays.
#[derive(Clone, Debug, PartialEq)]
enum Screen {
    /// The game's window.
    Main,
    /// A monitor, by name: a borderless fullscreen window there.
    Monitor(String),
    /// A window others can share, by number from 2 ("Window 2"); 0 is a new one, given the
    /// first free number when it opens. A profile keeps only "a window": numbers last a session.
    Window(u8),
}
impl Screen {
    /// As `screen.txt` keeps it.
    fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path).unwrap_or_default().trim() {
            "window" | "own" => Screen::Window(0),
            text => text
                .strip_prefix("monitor:")
                .map_or(Screen::Main, |name| Screen::Monitor(name.to_owned())),
        }
    }
    fn save(&self, path: &Path) {
        let text = match self {
            Screen::Main => "main".to_owned(),
            Screen::Monitor(name) => format!("monitor:{name}"),
            Screen::Window(_) => "window".to_owned(),
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(path, text + "\n") {
            log::warn!("Horizon: splitscreen: {}: {e}", path.display());
        }
    }
}

/// Which window a view is in: the game's, a monitor's, or a numbered one.
#[derive(Clone, Debug, PartialEq)]
enum Key {
    Main,
    Monitor(String),
    Window(u8),
}

/// A player's view: its window (an index into the windows), its corner there and its size.
type View = (usize, [u32; 2], PhysicalSize<u32>);

/// Each player's view, given the window each is in and the windows' sizes. Players in one window
/// share it in the order they joined: two take halves (`two`: 0 side by side when wider than
/// tall, 1 side by side, 2 top and bottom); three (`three`: 0 the first wide on top and two below,
/// 1 the first tall on the left and two on the right, 2 three columns); four its quarters.
fn place(windows: &[usize], sizes: &[PhysicalSize<u32>], two: u8, three: u8) -> Vec<View> {
    windows
        .iter()
        .enumerate()
        .map(|(i, &window)| {
            let here = windows.iter().filter(|&&w| w == window).count();
            let slot = windows[..i].iter().filter(|&&w| w == window).count() as u32;
            let size = sizes
                .get(window)
                .copied()
                .unwrap_or(PhysicalSize::new(1, 1));
            let (w, h) = (size.width, size.height);
            let side_by_side = match two {
                1 => true,
                2 => false,
                _ => w >= h,
            };
            // The view's cell as fractions of the window: (x0, x1, y0, y1) in halves or thirds.
            let cell = |x0: u32, x1: u32, y0: u32, y1: u32, of: u32| {
                [w * x0 / of, w * x1 / of, h * y0 / of, h * y1 / of]
            };
            let [x0, x1, y0, y1] = match (here, slot) {
                (1, _) => cell(0, 1, 0, 1, 1),
                (2, s) if side_by_side => cell(s, s + 1, 0, 2, 2),
                (2, s) => cell(0, 2, s, s + 1, 2),
                (3, s) => match (three, s) {
                    (2, s) => cell(s, s + 1, 0, 3, 3),
                    (1, 0) => cell(0, 1, 0, 2, 2),
                    (1, s) => cell(1, 2, s - 1, s, 2),
                    (_, 0) => cell(0, 2, 0, 1, 2),
                    (_, s) => cell(s - 1, s, 1, 2, 2),
                },
                (_, s) => cell(s % 2, s % 2 + 1, s / 2, s / 2 + 1, 2),
            };
            (
                window,
                [x0, y0],
                PhysicalSize::new((x1 - x0).max(1), (y1 - y0).max(1)),
            )
        })
        .collect()
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
    key: Key,
    window: Arc<Window>,
    compositor: Compositor,
    size: PhysicalSize<u32>,
    focused: bool,
    /// The title it was last given.
    title: String,
}

struct Guest {
    game: Game<'static>,
    /// The profile it plays as; `None` while it picks one.
    name: Option<String>,
    /// Its place among the players (2 to 4), for its control socket and own window.
    seat: usize,
    screen: Screen,
    pad: Option<PadId>,
    options: PathBuf,
    /// Its buttons and focus targets while another player's game runs.
    widgets: widgets::Saved,
}

pub(super) struct Split<'a> {
    pub(super) host: Game<'a>,
    guests: Vec<Guest>,
    /// Splitscreen is on: every player renders through the compositors (else P1 draws on its
    /// window and every event goes straight to it).
    enabled: bool,
    /// `HORIZON_SPLIT=N`: players without controllers still to add once P1 is in a world.
    test_players: usize,
    pads: PadRouter,
    p1_pad: Option<PadId>,
    p1_screen: Screen,
    /// A controller offered to join, and since when.
    offer: Option<(PadId, Instant)>,
    /// Pads whose buttons still count as the press that joined them: released first.
    fresh: Vec<PadId>,
    /// The game's window and its compositor first, then monitors' and players' own.
    surfaces: Vec<Surface>,
    /// The views the players were last fitted to.
    applied: Vec<View>,
    /// Where each numbered window was when it closed, to open there again (this session).
    window_rects: std::collections::HashMap<u8, (PhysicalPosition<i32>, PhysicalSize<u32>)>,
    /// The player keyboard and mouse drive (0 = P1), and the pointer: its window and place.
    focus: usize,
    cursor: (usize, PhysicalPosition<f64>),
    limiter: horizon_client::window::FramerateLimiter,
    clock: horizon_client::window::SystemClock,
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
                key: Key::Main,
                window: window.clone(),
                compositor,
                size: window.inner_size(),
                focused: true,
                title: String::new(),
            }],
            _ => Vec::new(),
        };
        let p1_screen =
            Self::folder().map_or(Screen::Main, |f| Screen::load(&f.join("p1-screen.txt")));
        Self {
            host,
            guests: Vec::new(),
            enabled,
            test_players,
            pads: PadRouter::default(),
            p1_pad: None,
            p1_screen,
            offer: None,
            fresh: Vec::new(),
            surfaces,
            applied: Vec::new(),
            window_rects: Default::default(),
            focus: 0,
            cursor: (0, PhysicalPosition::new(0.0, 0.0)),
            limiter: horizon_client::window::FramerateLimiter::new(0),
            clock: Default::default(),
            turn: 0,
            host_widgets: Default::default(),
        }
    }

    fn players(&self) -> usize {
        1 + self.guests.len()
    }

    /// The splitscreen folder in the game folder: `profiles/<name>/` holds each profile.
    fn folder() -> Result<PathBuf> {
        Ok(std::env::current_dir()?.join("splitscreen"))
    }

    /// Where each player's screen choice is kept.
    fn screen_file(&self, i: usize) -> Option<PathBuf> {
        let folder = Self::folder().ok()?;
        match i {
            0 => Some(folder.join("p1-screen.txt")),
            _ => Some(folder.join(format!(
                "profiles/{}/screen.txt",
                self.guests.get(i - 1)?.name.as_ref()?
            ))),
        }
    }

    fn screen(&self, i: usize) -> &Screen {
        match i {
            0 => &self.p1_screen,
            _ => &self.guests[i - 1].screen,
        }
    }

    /// The monitors, by name.
    fn monitors(&self) -> Vec<MonitorHandle> {
        self.surfaces
            .first()
            .map(|s| {
                s.window
                    .available_monitors()
                    .filter(|m| m.name().is_some())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The monitor the game's window is on: Window 1 already shows there.
    fn main_monitor(&self) -> Option<String> {
        self.surfaces.first()?.window.current_monitor()?.name()
    }

    /// Where player `i` can play, as the screens say it: Window 1 (the game's), the windows open,
    /// a new window (unless it is alone in one), and each monitor but Window 1's (numbered left
    /// to right, with its size: the system's names are ids on some). `i` past the players: one
    /// joining. The page cycles through them forward (buttons take the left click only).
    fn choices(&self, i: usize) -> Vec<(String, Screen)> {
        let main = self.main_monitor();
        let mut monitors = self.monitors();
        monitors.sort_by_key(|m| (m.position().x, m.position().y));
        let monitors = monitors.iter().enumerate().filter_map(|(n, m)| {
            let name = m.name().filter(|name| Some(name) != main.as_ref())?;
            let size = m.size();
            let label = format!("Monitor {} ({}x{})", n + 1, size.width, size.height);
            Some((label, Screen::Monitor(name)))
        });
        let window_of = |j: usize| match self.screen(j) {
            Screen::Window(n) if *n > 0 => Some(*n),
            _ => None,
        };
        let mut open: Vec<u8> = (0..self.players()).filter_map(window_of).collect();
        open.sort_unstable();
        open.dedup();
        // Alone in its window, a new one would only renumber it.
        let alone = i < self.players()
            && window_of(i)
                .is_some_and(|n| (0..self.players()).all(|j| j == i || window_of(j) != Some(n)));
        std::iter::once((WINDOW_1.to_owned(), Screen::Main))
            .chain(
                open.into_iter()
                    .map(|n| (format!("Window {n}"), Screen::Window(n))),
            )
            .chain((!alone).then(|| (NEW_WINDOW.to_owned(), Screen::Window(0))))
            .chain(monitors)
            .collect()
    }

    /// How a screen is shown: a monitor that's gone or Window 1's own, as Window 1.
    fn label(&self, screen: &Screen) -> String {
        match screen {
            Screen::Window(0) => NEW_WINDOW.to_owned(),
            Screen::Window(n) => format!("Window {n}"),
            _ => self
                .choices(usize::MAX)
                .into_iter()
                .find(|(_, s)| s == screen)
                .map_or_else(|| WINDOW_1.to_owned(), |(label, _)| label),
        }
    }

    /// A new window's number: the first not in use.
    fn free_window(&self) -> u8 {
        (2..)
            .find(|n| (0..self.players()).all(|j| self.screen(j) != &Screen::Window(*n)))
            .unwrap_or(2)
    }

    /// The window player `i` is in: a monitor that's gone, or the one Window 1 is on, means
    /// Window 1.
    fn key(&self, i: usize, monitors: &[String]) -> Key {
        match self.screen(i) {
            Screen::Monitor(name)
                if monitors.contains(name) && Some(name) != self.main_monitor().as_ref() =>
            {
                Key::Monitor(name.clone())
            }
            Screen::Window(n) if *n > 0 => Key::Window(*n),
            _ => Key::Main,
        }
    }

    /// Every player's view, in the windows there are now.
    fn views(&self) -> Vec<View> {
        let monitors: Vec<_> = self
            .monitors()
            .iter()
            .filter_map(MonitorHandle::name)
            .collect();
        let windows: Vec<usize> = (0..self.players())
            .map(|i| {
                let key = self.key(i, &monitors);
                self.surfaces.iter().position(|s| s.key == key).unwrap_or(0)
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

    /// Player `i`'s game (0 = P1), in its turn, and its window's id (a game only takes its own
    /// window's events).
    fn player(&mut self, i: usize) -> Option<(&mut dyn ApplicationHandler, WindowId)> {
        self.turn(i);
        let id = match i {
            0 => self.host.window.as_ref()?.id(),
            _ => self.guests.get(i - 1)?.game.window.as_ref()?.id(),
        };
        match i {
            0 => Some((&mut self.host as &mut dyn ApplicationHandler, id)),
            _ => self
                .guests
                .get_mut(i - 1)
                .map(|g| (&mut g.game as &mut dyn ApplicationHandler, id)),
        }
    }

    fn captured(&self, i: usize) -> bool {
        match i {
            0 => self.host.captured,
            _ => self.guests.get(i - 1).is_some_and(|g| g.game.captured),
        }
    }

    /// The pointer as the focused player sees it: from its view's corner.
    fn cursor_in_focus(&self) -> PhysicalPosition<f64> {
        let [x, y] = self.views().get(self.focus).map_or([0, 0], |v| v.1);
        let (_, at) = self.cursor;
        PhysicalPosition::new(at.x - f64::from(x), at.y - f64::from(y))
    }

    /// Every player into its view: its window, then its size. Keyboard and mouse follow their
    /// player into another window; a player whose view moved or grew shows its name tag.
    fn relayout(&mut self, event_loop: &ActiveEventLoop) {
        let views = self.views();
        let before = std::mem::replace(&mut self.applied, views);
        for (i, &(surface, _, size)) in self.applied.clone().iter().enumerate() {
            let Some(window) = self.surfaces.get(surface).map(|s| s.window.clone()) else {
                continue;
            };
            let moved = before.get(i).is_none_or(|b| b.0 != surface);
            match i {
                0 => into_window(&mut self.host, window.clone(), moved),
                _ => into_window(&mut self.guests[i - 1].game, window.clone(), moved),
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

    /// Player `i`'s name tag: "P2 · Alex" and where it plays (or `note`).
    fn tag(&mut self, i: usize, note: Option<&str>) {
        let who = match i {
            0 => self.host.options.player_name(),
            _ => match &self.guests[i - 1].name {
                Some(name) => name.clone(),
                None => return,
            },
        };
        let note = note.map_or_else(|| self.label(self.screen(i)), str::to_owned);
        let toast =
            horizon_client::toasts::SystemToast::new(format!("P{} · {who}", i + 1), Some(note));
        match i {
            0 => self.host.split_tag = Some(toast),
            _ => self.guests[i - 1].game.split_tag = Some(toast),
        }
    }

    /// What a window's title says: its name and who plays in it.
    fn title(&self, k: usize) -> String {
        let views = &self.applied;
        let who: Vec<String> = (0..self.players())
            .filter(|&i| views.get(i).is_some_and(|v| v.0 == k))
            .map(|i| match i {
                0 => self.host.options.player_name(),
                _ => self.guests[i - 1]
                    .name
                    .clone()
                    .unwrap_or_else(|| "...".into()),
            })
            .collect();
        let name = match &self.surfaces[k].key {
            Key::Window(n) => format!(" - Window {n}"),
            Key::Main if self.surfaces.len() > 1 => format!(" - {WINDOW_1}"),
            _ => String::new(),
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
        let monitors = self.monitors();
        let names: Vec<_> = monitors.iter().filter_map(MonitorHandle::name).collect();
        let wanted: Vec<Key> = (0..self.players()).map(|i| self.key(i, &names)).collect();
        let before = self.surfaces.len();
        for s in &self.surfaces {
            if let Key::Window(n) = s.key
                && !wanted.contains(&s.key)
                && let Ok(at) = s.window.outer_position()
            {
                self.window_rects.insert(n, (at, s.window.inner_size()));
            }
        }
        self.surfaces
            .retain(|s| s.key == Key::Main || wanted.contains(&s.key));
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
            let cover = match key {
                Key::Monitor(name) => monitors
                    .iter()
                    .find(|m| m.name().as_ref() == Some(name))
                    .map(|m| (m.position(), m.size())),
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
                    let last = match key {
                        Key::Window(n) => self.window_rects.get(n).copied(),
                        _ => None,
                    };
                    match last {
                        Some((at, size)) => attributes.with_position(at).with_inner_size(size),
                        None => attributes.with_inner_size(LogicalSize::new(854.0, 480.0)),
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
                    key: key.clone(),
                    size: window.inner_size(),
                    window,
                    compositor,
                    focused: false,
                    title: String::new(),
                }),
                Err(e) => log::warn!("Horizon: splitscreen: no window for {key:?}: {e}"),
            }
            changed = true;
        }
        // The game's window only while someone plays on it.
        let main_used = wanted.contains(&Key::Main);
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
            if i >= self.players() {
                continue;
            }
            let choices = self.choices(i);
            let at = choices
                .iter()
                .position(|(_, s)| s == self.screen(i))
                .unwrap_or(0);
            let n = choices.len().max(1);
            let mut next = choices[if back { (at + n - 1) % n } else { (at + 1) % n }]
                .1
                .clone();
            if next == Screen::Window(0) {
                next = Screen::Window(self.free_window());
            }
            if let Some(file) = self.screen_file(i) {
                next.save(&file);
            }
            match i {
                0 => self.p1_screen = next,
                _ => self.guests[i - 1].screen = next,
            }
        }
        let rows: Vec<(String, String)> = (0..self.players())
            .map(|i| {
                let who = match i {
                    0 => self.host.options.player_name(),
                    _ => self.guests[i - 1]
                        .name
                        .clone()
                        .unwrap_or_else(|| "(choosing)".into()),
                };
                (format!("P{} {who}", i + 1), self.label(self.screen(i)))
            })
            .collect();
        self.host.ui.options.split_players = rows.clone();
        for g in &mut self.guests {
            g.game.ui.options.split_players = rows.clone();
        }
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
        let compositor = old.compositor(window.clone())?;
        self.host.renderer = Some(renderer);
        self.host.options.offscreen = true;
        self.host.options.paced_by_split = true;
        // The new renderer takes the old one's present mode, settings and all: P1's again.
        self.host.apply_settings();
        self.host.clear_lod();
        self.host.all_sections_changed();
        self.surfaces = vec![Surface {
            key: Key::Main,
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
        self.turn(0);
        if let Some(server) = &self.host.integrated {
            for g in &self.guests {
                if let Some(name) = &g.name {
                    server.leave_split(name);
                }
            }
        }
        self.guests.clear();
        self.focus = 0;
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
        let renderer = build_renderer(
            main.window.clone(),
            false,
            &self.host.pack,
            &self.host.gui,
            Some(&old),
            None,
            api,
        )?;
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
    fn route_pads(&mut self) -> Option<PadId> {
        let now = Instant::now();
        let mut join = None;
        for (id, button) in self.pads.presses() {
            if Some(id) == self.p1_pad || self.guests.iter().any(|g| g.pad == Some(id)) {
                continue;
            }
            if !self.in_world() {
                if self.p1_pad.is_none() {
                    self.p1_pad = Some(id);
                    self.fresh.push(id);
                }
                continue;
            }
            match self.offer {
                Some((offered, _)) if offered == id => match button {
                    Button::South => {
                        self.offer = None;
                        join = Some(id);
                    }
                    Button::North if self.p1_pad.is_none() => {
                        self.offer = None;
                        self.p1_pad = Some(id);
                        self.fresh.push(id);
                    }
                    Button::East => self.offer = None,
                    _ => {}
                },
                _ if self.players() < MAX_PLAYERS => self.offer = Some((id, now)),
                _ => {}
            }
        }
        if let Some((id, since)) = self.offer {
            if now - since > OFFER || !self.in_world() {
                self.offer = None;
            } else {
                let ask = if self.p1_pad.is_none() {
                    "press A to join, Y to play as P1"
                } else {
                    "press A to join"
                };
                let text = Text::literal(format!("{}: {ask}", self.pads.name(id)));
                self.host.titles.set_overlay(text, false);
            }
        }
        let p1 = self.pad_state(self.p1_pad);
        self.host.pad.feed = Some(p1);
        for i in 0..self.guests.len() {
            let state = self.pad_state(self.guests[i].pad);
            self.guests[i].game.pad.feed = Some(state);
        }
        join
    }

    /// Add a player driven by `pad`: it picks a profile and where to play in its view first,
    /// unless `name` is given (tests).
    fn join(&mut self, pad: Option<PadId>, name: Option<String>) -> Result<()> {
        let seat = (2..=MAX_PLAYERS)
            .find(|n| self.guests.iter().all(|g| g.seat != *n))
            .ok_or("every seat is taken")?;
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
        let labels = self
            .choices(usize::MAX)
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        if let Some(title) = game.title.as_mut() {
            title.profile_picker(names, taken, labels);
        }
        self.guests.push(Guest {
            game,
            name: None,
            seat,
            screen: Screen::Main,
            pad,
            options,
            widgets: Default::default(),
        });
        if let Some(name) = name {
            self.enter(self.guests.len() - 1, name, None)?;
        }
        Ok(())
    }

    /// The names playing now: P1's and the joined players'.
    fn taken(&self) -> Vec<String> {
        std::iter::once(self.host.options.player_name())
            .chain(self.guests.iter().filter_map(|g| g.name.clone()))
            .collect()
    }

    /// Guest `i` plays as profile `name` on `screen` (`None`: where the profile played last):
    /// its options, then into P1's world.
    fn enter(&mut self, i: usize, name: String, screen: Option<String>) -> Result<()> {
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
        let picked = screen.and_then(|label| {
            self.choices(usize::MAX)
                .into_iter()
                .find(|(l, _)| *l == label)
                .map(|(_, s)| s)
        });
        let screen = match picked {
            Some(screen) => {
                screen.save(&profile.join("screen.txt"));
                screen
            }
            None => Screen::load(&profile.join("screen.txt")),
        };
        // "A window": a new one, numbered now.
        let screen = match screen {
            Screen::Window(0) => Screen::Window(self.free_window()),
            other => other,
        };
        let pipe = self
            .host
            .integrated
            .as_ref()
            .and_then(|server| server.join_split(&name))
            .ok_or("P1's world is not on its server thread")?;
        self.guests[i].options = options;
        self.guests[i].screen = screen;
        self.turn(i + 1);
        let guest = &mut self.guests[i];
        settings.extras.agent_control &= guest.game.options.control.is_some();
        guest.game.settings = settings;
        guest.game.options.player_name = Some(name.clone());
        guest.game.apply_settings();
        let joined = guest.game.connect_local(pipe, &name);
        self.turn(0);
        joined?;
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
                Some(Some((name, screen))) => match self.enter(i, name, screen) {
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
                self.guests.remove(i);
                changed = true;
            } else {
                i += 1;
            }
        }
        changed
    }

    /// A new player's game in its seat, on its title screen until it has a profile.
    fn start_guest(&mut self, seat: usize, options_file: &Path) -> Result<Game<'static>> {
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
        let res: &'static Arc<_> = Box::leak(Box::new(host.res.clone()));
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
        let server = self.host.integrated.as_ref();
        self.guests.retain(|g| {
            // A player picking a profile has no connection yet.
            let playing = g.name.is_none() || g.game.remote.is_some();
            let stays = !p1_left && playing && g.game.error.is_none();
            if !stays && let (Some(server), Some(name)) = (server, &g.name) {
                server.leave_split(name);
            }
            stays
        });
        if self.focus >= self.players() {
            self.focus = 0;
        }
        self.guests.len() != before
    }

    /// A window other than Window 1 was closed: its players leave, P1 goes back to Window 1.
    fn close_window(&mut self, surface: usize) -> Result<()> {
        let views = self.views();
        for i in (0..self.players()).rev() {
            if views.get(i).is_none_or(|v| v.0 != surface) {
                continue;
            }
            if i == 0 {
                self.p1_screen = Screen::Main;
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
                    key: Key::Main,
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
                self.cursor = (surface, position);
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
                let (x, y) = (self.cursor.1.x as u32, self.cursor.1.y as u32);
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
        if let Some(pad) = pad_joins {
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
                .then(|| format!("P{}", self.players() + 1));
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
        changed |= self.sync_surfaces(event_loop);
        // Also a new arrangement from the settings (a screen, Two on a screen).
        if changed || self.views() != self.applied {
            self.relayout(event_loop);
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
        }
        self.turn(0);
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
    /// The splitscreen name tag, in the toast slot after the others (`ToastManager`).
    pub(super) fn draw_split_tag(
        &mut self,
        batch: &mut horizon_client::hud::HudBatch,
        slot: usize,
    ) {
        let multiplier = f64::from(self.settings.notification_display_time);
        let Some(tag) = &mut self.split_tag else {
            return;
        };
        let (_, finished) = tag.update(multiplier);
        if finished {
            self.split_tag = None;
            return;
        }
        tag.draw(&self.gui.atlas, batch, 32.0 * slot as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(w: u32, h: u32) -> PhysicalSize<u32> {
        PhysicalSize::new(w, h)
    }

    #[test]
    fn players_on_one_screen_share_it() {
        let wide = [size(1920, 1080)];
        assert_eq!(
            place(&[0, 0], &wide, 0, 0)[1],
            (0, [960, 0], size(960, 1080))
        );
        assert_eq!(
            place(&[0, 0], &wide, 2, 0)[1],
            (0, [0, 540], size(1920, 540))
        );
        // Auto on a tall screen: top and bottom.
        assert_eq!(place(&[0, 0], &[size(1080, 1920)], 0, 0)[1].1, [0, 960]);
        assert_eq!(
            place(&[0; 4], &wide, 0, 0)[3],
            (0, [960, 540], size(960, 540))
        );
    }

    #[test]
    fn desk_and_couch() {
        // P1 alone at the desk; P2 and P3 share the TV.
        let screens = [size(3024, 1964), size(3840, 2160)];
        let views = place(&[0, 1, 1], &screens, 0, 0);
        assert_eq!(views[0], (0, [0, 0], size(3024, 1964)));
        assert_eq!(views[1], (1, [0, 0], size(1920, 2160)));
        assert_eq!(views[2], (1, [1920, 0], size(1920, 2160)));
    }

    #[test]
    fn three_on_a_screen() {
        let tv = [size(1920, 1080)];
        // One wide on top, two below.
        let top = place(&[0; 3], &tv, 0, 0);
        assert_eq!(top[0], (0, [0, 0], size(1920, 540)));
        assert_eq!(top[2], (0, [960, 540], size(960, 540)));
        // One tall on the left, two stacked on the right.
        let left = place(&[0; 3], &tv, 0, 1);
        assert_eq!(left[0], (0, [0, 0], size(960, 1080)));
        assert_eq!(left[2], (0, [960, 540], size(960, 540)));
        // Three columns.
        assert_eq!(
            place(&[0; 3], &tv, 0, 2)[2],
            (0, [1280, 0], size(640, 1080))
        );
    }

    #[test]
    fn two_windows_two_players_each() {
        // P1 and P2 at the desk, P3 and P4 on the TV: halves in each window.
        let screens = [size(3024, 1964), size(3840, 2160)];
        let views = place(&[0, 0, 1, 1], &screens, 0, 0);
        assert_eq!(views[1], (0, [1512, 0], size(1512, 1964)));
        assert_eq!(views[2], (1, [0, 0], size(1920, 2160)));
        assert_eq!(views[3], (1, [1920, 0], size(1920, 2160)));
    }

    #[test]
    fn screen_choices_round_trip() {
        let dir = std::env::temp_dir().join(format!("horizon-split-screen-{}", std::process::id()));
        let file = dir.join("screen.txt");
        for screen in [
            Screen::Main,
            Screen::Window(0),
            Screen::Monitor("LG TV".into()),
        ] {
            screen.save(&file);
            assert_eq!(Screen::load(&file), screen);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
