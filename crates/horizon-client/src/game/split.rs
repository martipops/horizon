//! Splitscreen (Horizon's Splitscreen setting, off by default): players drop in and out of P1's
//! world on one screen, as on a console. P1 is the game as launched, with the keyboard, the mouse
//! and the first controller used in a menu. In a world, another controller pressing a button is
//! offered to join; pressing A adds a player, another `Game` in this process that joins P1's
//! server thread through memory, and the window re-tiles: halves for two, quarters for three or
//! four. A player who disconnects (pause menu) drops out; when P1 leaves the world, all do.
//!
//! Every player renders offscreen on one device and the compositor copies their frames into the
//! window; with the setting on, P1 does too from the start (one view filling the window), so
//! nothing is rebuilt when someone joins. Keyboard and mouse drive the player whose view was
//! clicked last (Esc frees the mouse, then click another view). The window paces every player's
//! frames once, at the fastest any wants.
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
    dpi::{PhysicalPosition, PhysicalSize},
    event::{DeviceEvent, DeviceId, ElementState, WindowEvent},
    event_loop::ActiveEventLoop,
    window::WindowId,
};

const MAX_PLAYERS: usize = 4;
/// How long a controller's offer to join stays up.
const OFFER: Duration = Duration::from_secs(10);

thread_local! {
    /// The options file of the player whose turn it is (none for P1, whose is the game's own):
    /// every player runs on this thread, one after another.
    static OPTIONS: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// The `options.txt` of the player whose turn it is; `None` for P1.
pub(super) fn options_path() -> Option<PathBuf> {
    OPTIONS.with_borrow(Clone::clone)
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
    compositor: Option<Compositor>,
    size: PhysicalSize<u32>,
    /// The player keyboard and mouse drive (0 = P1), and the pointer on the window.
    focus: usize,
    cursor: PhysicalPosition<f64>,
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
        // Made after the window on an ordinary launch (no `resumed` follows): its size now.
        let size = host
            .window
            .as_ref()
            .map_or(PhysicalSize::new(1, 1), |w| w.inner_size());
        Self {
            host,
            guests: Vec::new(),
            enabled,
            test_players,
            pads: PadRouter::default(),
            p1_pad: None,
            offer: None,
            fresh: Vec::new(),
            compositor,
            size,
            focus: 0,
            cursor: PhysicalPosition::new(0.0, 0.0),
            limiter: horizon_client::window::FramerateLimiter::new(0),
            clock: Default::default(),
            turn: 0,
            host_widgets: Default::default(),
        }
    }

    /// Each player's view: its top-left corner and size. One player fills the window, two split
    /// it down the middle, three or four take its quarters.
    fn views(&self) -> Vec<([u32; 2], PhysicalSize<u32>)> {
        let players = 1 + self.guests.len() as u32;
        let (w, h) = (self.size.width, self.size.height);
        let (columns, rows) = match players {
            1 => (1, 1),
            2 => (2, 1),
            _ => (2, 2),
        };
        (0..players)
            .map(|i| {
                let (column, row) = (i % columns, i / columns);
                let (x0, x1) = (w * column / columns, w * (column + 1) / columns);
                let (y0, y1) = (h * row / rows, h * (row + 1) / rows);
                (
                    [x0, y0],
                    PhysicalSize::new((x1 - x0).max(1), (y1 - y0).max(1)),
                )
            })
            .collect()
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

    /// Player `i`'s game (0 = P1), in its turn.
    fn player(&mut self, i: usize) -> Option<&mut dyn ApplicationHandler> {
        self.turn(i);
        match i {
            0 => Some(&mut self.host),
            _ => self.guests.get_mut(i - 1).map(|g| &mut g.game as _),
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
        let [x, y] = self.views().get(self.focus).map_or([0, 0], |v| v.0);
        PhysicalPosition::new(self.cursor.x - f64::from(x), self.cursor.y - f64::from(y))
    }

    fn resize_views(&mut self, event_loop: &ActiveEventLoop) {
        let Some(id) = self.host.window.as_ref().map(|w| w.id()) else {
            return;
        };
        for (i, view) in self.views().into_iter().enumerate() {
            if let Some(player) = self.player(i) {
                player.window_event(event_loop, id, WindowEvent::Resized(view.1));
            }
        }
        self.turn(0);
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
                _ if self.guests.len() + 1 < MAX_PLAYERS => self.offer = Some((id, now)),
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
        if self.focus > self.guests.len() {
            self.focus = 0;
        }
        self.guests.len() != before
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
        self.size = window.inner_size();
        if self.compositor.is_none() {
            match renderer.compositor(window.clone()) {
                Ok(c) => self.compositor = Some(c),
                Err(e) => {
                    self.host.error = Some(e);
                    event_loop.exit();
                    return;
                }
            }
        }
        self.resize_views(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if !self.enabled {
            return self.host.window_event(event_loop, id, event);
        }
        match event {
            WindowEvent::Resized(size) => {
                self.size = size;
                if let Some(c) = &mut self.compositor {
                    c.resize(size.width, size.height);
                }
                self.resize_views(event_loop);
            }
            // The views draw themselves offscreen; the compositor presents in about_to_wait.
            WindowEvent::RedrawRequested => {}
            // Closing the window is P1's to handle (it saves the world, then quits); a player's
            // game would end the event loop before that.
            WindowEvent::CloseRequested => self.host.window_event(event_loop, id, event),
            WindowEvent::Focused(_) => {
                for i in 0..=self.guests.len() {
                    if let Some(player) = self.player(i) {
                        player.window_event(event_loop, id, event.clone());
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
                if let Some(player) = self.player(focus) {
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
                let clicked = self.views().iter().position(|([vx, vy], size)| {
                    (*vx..vx + size.width).contains(&x) && (*vy..vy + size.height).contains(&y)
                });
                if let Some(clicked) = clicked
                    && clicked != self.focus
                    && !self.captured(self.focus)
                {
                    self.focus = clicked;
                    let position = self.cursor_in_focus();
                    if let Some(player) = self.player(clicked) {
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
                if let Some(player) = self.player(focus) {
                    player.window_event(event_loop, id, event);
                }
            }
            _ => {
                let focus = self.focus;
                if let Some(player) = self.player(focus) {
                    player.window_event(event_loop, id, event);
                }
            }
        }
        self.turn(0);
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, id: DeviceId, event: DeviceEvent) {
        let focus = if self.enabled { self.focus } else { 0 };
        if let Some(player) = self.player(focus) {
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
        // Testing: players without controllers, one per frame, once.
        changed |= self.pick_profiles();
        if self.in_world() && self.test_players > 0 {
            let seat = MAX_PLAYERS + 1 - self.test_players;
            // HORIZON_SPLIT_PICK: they pick a profile like a controller's player, else P2...
            let name = std::env::var_os("HORIZON_SPLIT_PICK")
                .is_none()
                .then(|| format!("P{seat}"));
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
        if changed {
            self.resize_views(event_loop);
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
        let views = self.views();
        let frames: Vec<_> = std::iter::once(&self.host.renderer)
            .chain(self.guests.iter().map(|g| &g.game.renderer))
            .zip(&views)
            .filter_map(|(r, view)| Some((r.as_ref()?.offscreen_frame()?, view.0)))
            .collect();
        if let Some(c) = &mut self.compositor {
            c.set_vsync(self.host.settings.vsync);
            c.present(&frames);
        }
        // One pace for all: the fastest any player wants, so an idle one (vanilla's AFK limit)
        // can't slow the others. With vsync the display paces anything at or above its refresh
        // rate; the limiter on top would miss refreshes.
        let limit = std::iter::once(self.host.framerate_limit())
            .chain(self.guests.iter().map(|g| g.game.framerate_limit()))
            .max()
            .unwrap_or(60);
        let refresh_hz = self
            .host
            .window
            .as_ref()
            .and_then(|w| w.current_monitor()?.refresh_rate_millihertz())
            .map_or(60, |mhz| mhz.div_ceil(1000) as i32);
        if !(self.host.settings.vsync && limit >= refresh_hz) {
            horizon_client::window::end_frame(&mut self.limiter, limit, &mut self.clock);
        }
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        for i in (0..=self.guests.len()).rev() {
            if let Some(player) = self.player(i) {
                player.exiting(event_loop);
            }
        }
        self.turn(0);
    }
}
