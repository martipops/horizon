//! Splitscreen (Horizon's Splitscreen setting, off by default): players drop in and out of P1's
//! world on one screen, as on a console. P1 is the game as launched, with the keyboard, the mouse
//! and the first controller used in a menu. In a world, another controller pressing a button is
//! offered to join; pressing A adds a player, another `Game` in this process that joins P1's
//! server thread through memory. A player who disconnects (pause menu) drops out; when P1 leaves
//! the world, all do.
//!
//! Every player renders offscreen on one device; each window's compositor copies the frames of
//! the players placed in it. Splitscreen windows (the setting beside it) place them: all in one
//! window (halves for two, side by side or top and bottom; quarters for three or four), spread
//! across the monitors (a borderless fullscreen window on each one after P1's), or each in its own
//! window. With the setting on, P1 renders this way from the start, so nothing is rebuilt when
//! someone joins. Keyboard and mouse drive the player whose view was clicked last (Esc frees the
//! mouse, then click another view); closing a player's own window drops that player. The players'
//! frames are paced once, at the fastest any wants.
//!
//! A joining player first picks a profile in its view, or makes one (`title/profiles.rs`): a name
//! with its own options in `splitscreen/profiles/<name>/options.txt` (first a copy of P1's) and,
//! on the server, its own inventory.
//!
//! `HORIZON_SPLIT=N` (testing): the setting on, and N-1 players without controllers join once P1
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
    window::{Fullscreen, Window, WindowId},
};

const MAX_PLAYERS: usize = 4;
/// How long a controller's offer to join stays up.
const OFFER: Duration = Duration::from_secs(10);

/// Splitscreen windows: `extras.splitscreen_windows`.
const ONE_WINDOW: u8 = 0;
const PER_MONITOR: u8 = 1;
const PER_PLAYER: u8 = 2;

thread_local! {
    /// The options file of the player whose turn it is (none for P1, whose is the game's own):
    /// every player runs on this thread, one after another.
    static OPTIONS: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// The `options.txt` of the player whose turn it is; `None` for P1.
pub(super) fn options_path() -> Option<PathBuf> {
    OPTIONS.with_borrow(Clone::clone)
}

/// A player's view: its window, its corner in the window and its size.
type View = (usize, [u32; 2], PhysicalSize<u32>);

/// Where `players` views go in windows of `sizes` (window 0 is P1's): all in window 0, spread
/// over the windows (P1 and P3 in the first, P2 and P4 in the second...), or one window each.
/// Within a window two take halves (`two`: 0 side by side when wider than tall, 1 side by side,
/// 2 top and bottom), three or four its quarters.
fn place(windows: u8, two: u8, players: usize, sizes: &[PhysicalSize<u32>]) -> Vec<View> {
    let window_of = |i: usize| match windows {
        ONE_WINDOW => 0,
        PER_MONITOR => i % sizes.len().max(1),
        _ => i.min(sizes.len().saturating_sub(1)),
    };
    let mut views = Vec::with_capacity(players);
    for i in 0..players {
        let window = window_of(i);
        let here: Vec<usize> = (0..players).filter(|&p| window_of(p) == window).collect();
        let slot = here.iter().position(|&p| p == i).unwrap_or(0) as u32;
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
        let (columns, rows) = match here.len() {
            1 => (1, 1),
            2 if side_by_side => (2, 1),
            2 => (1, 2),
            _ => (2, 2),
        };
        let (column, row) = (slot % columns, slot / columns);
        let (x0, x1) = (w * column / columns, w * (column + 1) / columns);
        let (y0, y1) = (h * row / rows, h * (row + 1) / rows);
        views.push((
            window,
            [x0, y0],
            PhysicalSize::new((x1 - x0).max(1), (y1 - y0).max(1)),
        ));
    }
    views
}

/// A window showing players' views, and the compositor that copies their frames into it.
struct Surface {
    window: Arc<Window>,
    compositor: Compositor,
    size: PhysicalSize<u32>,
    focused: bool,
}

struct Guest {
    game: Game<'static>,
    /// The profile it plays as; `None` while it picks one.
    name: Option<String>,
    /// Its place among the players (2 to 4), for its control socket.
    seat: usize,
    pad: Option<PadId>,
    options: PathBuf,
    /// Its buttons and focus targets while another player's game runs.
    widgets: widgets::Saved,
}

pub(super) struct Split<'a> {
    pub(super) host: Game<'a>,
    guests: Vec<Guest>,
    /// The setting is on (else every event goes straight to P1).
    enabled: bool,
    /// `HORIZON_SPLIT=N`: players without controllers still to add once P1 is in a world.
    test_players: usize,
    pads: PadRouter,
    p1_pad: Option<PadId>,
    /// A controller offered to join, and since when.
    offer: Option<(PadId, Instant)>,
    /// Pads whose buttons still count as the press that joined them: released first.
    fresh: Vec<PadId>,
    /// The windows (P1's first) and the layout they were made for (`splitscreen_windows`).
    surfaces: Vec<Surface>,
    windows_mode: u8,
    /// The views the players were last fitted to.
    applied: Vec<View>,
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
        let enabled = host.settings.extras.splitscreen || test_players > 0;
        if enabled {
            host.options.offscreen = true;
            host.options.paced_by_split = true;
        }
        // Made after the window on an ordinary launch (no `resumed` follows).
        let surfaces = match (compositor, &host.window) {
            (Some(compositor), Some(window)) => vec![Surface {
                window: window.clone(),
                compositor,
                size: window.inner_size(),
                focused: true,
            }],
            _ => Vec::new(),
        };
        let windows_mode = host.settings.extras.splitscreen_windows;
        Self {
            host,
            guests: Vec::new(),
            enabled,
            test_players,
            pads: PadRouter::default(),
            p1_pad: None,
            offer: None,
            fresh: Vec::new(),
            surfaces,
            windows_mode,
            applied: Vec::new(),
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

    /// Every player's view, as the settings place them now.
    fn views(&self) -> Vec<View> {
        let sizes: Vec<_> = self.surfaces.iter().map(|s| s.size).collect();
        let e = &self.host.settings.extras;
        place(
            e.splitscreen_windows,
            e.splitscreen_two,
            self.players(),
            &sizes,
        )
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

    /// Every player into its view: its window, then its size.
    fn relayout(&mut self, event_loop: &ActiveEventLoop) {
        self.applied = self.views();
        for (i, &(surface, _, size)) in self.applied.clone().iter().enumerate() {
            let Some(window) = self.surfaces.get(surface).map(|s| s.window.clone()) else {
                continue;
            };
            match i {
                0 => self.host.window = Some(window),
                _ => self.guests[i - 1].game.window = Some(window),
            }
            if let Some((player, id)) = self.player(i) {
                player.window_event(event_loop, id, WindowEvent::Resized(size));
            }
        }
        self.turn(0);
    }

    /// The windows the layout needs: one, one per monitor the players use (P1's first), or one
    /// per player. A new layout makes the others again. True when they changed.
    fn sync_surfaces(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(host_window) = self.surfaces.first().map(|s| s.window.clone()) else {
            return false;
        };
        let mode = self.host.settings.extras.splitscreen_windows;
        let mut changed = false;
        if mode != self.windows_mode {
            self.windows_mode = mode;
            self.surfaces.truncate(1);
            changed = true;
        }
        // The monitors, the one P1's window is on first.
        let current = host_window.current_monitor();
        let mut monitors: Vec<_> = host_window
            .available_monitors()
            .filter(|m| Some(m) != current.as_ref())
            .collect();
        monitors.splice(0..0, current);
        let want = match mode {
            PER_MONITOR => self.players().min(monitors.len()).max(1),
            PER_PLAYER => self.players(),
            _ => 1,
        };
        if self.surfaces.len() > want {
            self.surfaces.truncate(want);
            changed = true;
        }
        while self.surfaces.len() < want {
            let k = self.surfaces.len();
            let attributes = Window::default_attributes()
                .with_title(format!("Horizon - P{}", k + 1))
                .with_window_icon(horizon_client::window::icon());
            let attributes = match mode {
                PER_MONITOR => attributes
                    .with_fullscreen(Some(Fullscreen::Borderless(monitors.get(k).cloned()))),
                _ => attributes.with_inner_size(LogicalSize::new(854.0, 480.0)),
            };
            let made = event_loop
                .create_window(attributes)
                .map_err(Into::into)
                .and_then(|w| {
                    let w = Arc::new(w);
                    let renderer = self.host.renderer.as_ref().ok_or("no renderer")?;
                    Ok::<_, Box<dyn std::error::Error>>((w.clone(), renderer.compositor(w)?))
                });
            match made {
                Ok((window, compositor)) => self.surfaces.push(Surface {
                    size: window.inner_size(),
                    window,
                    compositor,
                    focused: false,
                }),
                Err(e) => {
                    log::warn!("Horizon: splitscreen: no window for player {}: {e}", k + 1);
                    break;
                }
            }
            changed = true;
        }
        changed
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

    /// The splitscreen folder in the game folder: `profiles/<name>/` holds each profile.
    fn folder() -> Result<PathBuf> {
        Ok(std::env::current_dir()?.join("splitscreen"))
    }

    /// Add a player driven by `pad`: it picks a profile in its view first, unless `name` is
    /// given (tests).
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
        if let Some(title) = game.title.as_mut() {
            title.profile_picker(names, self.taken());
        }
        self.guests.push(Guest {
            game,
            name: None,
            seat,
            pad,
            options,
            widgets: Default::default(),
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

    /// Guest `i` plays as profile `name`: its options, then into P1's world.
    fn enter(&mut self, i: usize, name: String) -> Result<()> {
        if self.taken().contains(&name) {
            return Err(format!("{name} is already playing").into());
        }
        let options = Self::folder()?.join(format!("profiles/{name}/options.txt"));
        let mut settings = if options.exists() {
            horizon_client::settings::Settings::load(&options)
        } else {
            std::fs::create_dir_all(options.parent().ok_or("bad options path")?)?;
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
                Some(Some(name)) => match self.enter(i, name) {
                    Ok(()) => false,
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
        let audio = horizon_client::audio::Audio::silent(&options.resources);
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

    /// A player's own window was closed: that player leaves (P1's closes the game).
    fn close_window(&mut self, surface: usize) -> Result<()> {
        let views = self.views();
        for i in (1..self.players()).rev() {
            if views.get(i).is_some_and(|v| v.0 == surface) {
                let guest = &mut self.guests[i - 1];
                if guest.game.remote.is_some() {
                    guest.game.leave_server(None)?;
                } else if let Some(title) = guest.game.title.as_mut() {
                    // Still picking a profile: as if it pressed Cancel.
                    title.escape();
                }
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
                    window: window.clone(),
                    compositor,
                    size: window.inner_size(),
                    focused: true,
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
            // P1's window: P1's to handle (it saves the world, then quits); a player's game would
            // end the event loop before that. Another window: its players leave.
            WindowEvent::CloseRequested if surface == 0 => {
                self.host.window_event(event_loop, id, event)
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
        changed |= self.sync_surfaces(event_loop);
        // Also a new arrangement from the settings (Two players).
        if changed || self.views() != self.applied {
            self.relayout(event_loop);
        }
        for i in 0..self.guests.len() {
            self.turn(i + 1);
            self.guests[i].game.about_to_wait(event_loop);
            // A player's game stops the event loop on an error, as P1's does: say why.
            if let Some(e) = &self.guests[i].game.error {
                log::error!("Horizon: splitscreen: seat {}: {e}", self.guests[i].seat);
            }
        }
        self.turn(0);
        // Each window shows its players' frames. Only P1's waits for the display (vsync): two
        // windows each waiting would halve the rate.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn size(w: u32, h: u32) -> PhysicalSize<u32> {
        PhysicalSize::new(w, h)
    }

    #[test]
    fn one_window_tiles_halves_then_quarters() {
        let wide = [size(1920, 1080)];
        let two = place(ONE_WINDOW, 0, 2, &wide);
        assert_eq!(two[1], (0, [960, 0], size(960, 1080)));
        let stacked = place(ONE_WINDOW, 2, 2, &wide);
        assert_eq!(stacked[1], (0, [0, 540], size(1920, 540)));
        // Auto on a tall window: top and bottom.
        assert_eq!(place(ONE_WINDOW, 0, 2, &[size(1080, 1920)])[1].1, [0, 960]);
        let four = place(ONE_WINDOW, 0, 4, &wide);
        assert_eq!(four[3], (0, [960, 540], size(960, 540)));
    }

    #[test]
    fn per_monitor_spreads_players_first() {
        let monitors = [size(1920, 1080), size(2560, 1440)];
        let three = place(PER_MONITOR, 0, 3, &monitors);
        // P1 and P3 share the first monitor; P2 has the second to itself.
        assert_eq!(three[0], (0, [0, 0], size(960, 1080)));
        assert_eq!(three[1], (1, [0, 0], size(2560, 1440)));
        assert_eq!(three[2], (0, [960, 0], size(960, 1080)));
    }

    #[test]
    fn per_player_fills_each_window() {
        let windows = [size(800, 600), size(854, 480)];
        assert_eq!(
            place(PER_PLAYER, 0, 2, &windows),
            vec![(0, [0, 0], size(800, 600)), (1, [0, 0], size(854, 480))]
        );
    }
}
