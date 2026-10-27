//! Splitscreen spike (`HORIZON_SPLIT=N`, 2 to 4 players in one window). P1 is the game as
//! launched; once its world is open it goes to LAN and the others, more `Game`s in this process,
//! join it. Every player renders offscreen on one device and the compositor tiles their frames:
//! halves for two, quadrants for three or four. Each controller joins as the next player when one
//! of its buttons is pressed; keyboard and mouse drive P1. Player N also takes `horizon-ctl` on
//! P1's socket path + "N".
use super::{Game, UiResources, build_renderer, pad::PadRouter, start_game};
use horizon_client::{Result, gfx::compositor::Compositor};
use std::sync::Arc;
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{DeviceEvent, DeviceId, WindowEvent},
    event_loop::ActiveEventLoop,
    window::WindowId,
};

const LAN_PORT: &str = "25579";

pub(super) struct Split<'a> {
    pub(super) host: Game<'a>,
    guests: Vec<Game<'static>>,
    players: usize,
    pads: PadRouter,
    compositor: Option<Compositor>,
    size: PhysicalSize<u32>,
}

impl<'a> Split<'a> {
    pub(super) fn new(mut host: Game<'a>, players: usize) -> Self {
        host.options.offscreen = true;
        Self {
            host,
            guests: Vec::new(),
            players: players.clamp(2, 4),
            pads: PadRouter::default(),
            compositor: None,
            size: PhysicalSize::new(1, 1),
        }
    }

    /// Each player's view: its top-left corner and size. Two players split the window down the
    /// middle; three or four take its quarters.
    fn views(&self) -> Vec<([u32; 2], PhysicalSize<u32>)> {
        let (w, h) = (self.size.width, self.size.height);
        let (columns, rows) = if self.players == 2 { (2, 1) } else { (2, 2) };
        (0..self.players as u32)
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

    fn resize_views(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        let views = self.views();
        self.host
            .window_event(event_loop, id, WindowEvent::Resized(views[0].1));
        for (guest, view) in self.guests.iter_mut().zip(&views[1..]) {
            guest.window_event(event_loop, id, WindowEvent::Resized(view.1));
        }
    }

    /// Player `n` (from 2) joins P1's world, opened to LAN for the first.
    fn start_guest(&mut self, n: usize) -> Result<Game<'static>> {
        let host = &mut self.host;
        if n == 2 {
            host.lan_control(&["lan", "open", LAN_PORT])?;
        }
        let mut options = host.options.clone();
        options.world = None;
        options.server = Some(format!("127.0.0.1:{LAN_PORT}"));
        options.player_name = Some(format!("P{n}"));
        options.control = options.control.map(|p| {
            let mut p = p.into_os_string();
            p.push(n.to_string());
            p.into()
        });
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
            std::time::Instant::now(),
            None,
            Some(host.settings.clone()),
            None,
        )?
        .ok_or("a player did not start")?;
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
}

impl ApplicationHandler for Split<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.host.resumed(event_loop);
        let (Some(window), Some(renderer)) = (&self.host.window, &self.host.renderer) else {
            return;
        };
        if self.compositor.is_none() {
            self.size = window.inner_size();
            match renderer.compositor(window.clone()) {
                Ok(c) => self.compositor = Some(c),
                Err(e) => {
                    self.host.error = Some(e);
                    event_loop.exit();
                    return;
                }
            }
            let id = window.id();
            self.resize_views(event_loop, id);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::Resized(size) => {
                self.size = size;
                if let Some(c) = &mut self.compositor {
                    c.resize(size.width, size.height);
                }
                self.resize_views(event_loop, id);
            }
            // The views draw themselves offscreen; the compositor presents in about_to_wait.
            WindowEvent::RedrawRequested => {}
            WindowEvent::Focused(_) | WindowEvent::CloseRequested => {
                for guest in &mut self.guests {
                    guest.window_event(event_loop, id, event.clone());
                }
                self.host.window_event(event_loop, id, event);
            }
            _ => self.host.window_event(event_loop, id, event),
        }
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, id: DeviceId, event: DeviceEvent) {
        self.host.device_event(event_loop, id, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let mut pads = self.pads.read(self.players).into_iter();
        self.host.pad.feed = pads.next();
        for (guest, pad) in self.guests.iter_mut().zip(pads) {
            guest.pad.feed = Some(pad);
        }
        self.host.about_to_wait(event_loop);
        // One player joins per frame once P1 is playing in its world.
        if self.guests.len() + 1 < self.players
            && self.compositor.is_some()
            && self.host.player.is_some()
            && self.host.title.is_none()
        {
            match self.start_guest(self.guests.len() + 2) {
                Ok(guest) => {
                    self.guests.push(guest);
                    if let Some(id) = self.host.window.as_ref().map(|w| w.id()) {
                        self.resize_views(event_loop, id);
                    }
                }
                Err(e) => {
                    self.host.error = Some(e);
                    event_loop.exit();
                    return;
                }
            }
        }
        for guest in &mut self.guests {
            guest.about_to_wait(event_loop);
        }
        let views = self.views();
        let frames: Vec<_> = std::iter::once(&self.host.renderer)
            .chain(self.guests.iter().map(|g| &g.renderer))
            .zip(&views)
            .filter_map(|(r, view)| Some((r.as_ref()?.offscreen_frame()?, view.0)))
            .collect();
        if let Some(c) = &mut self.compositor {
            c.present(&frames);
        }
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        for guest in &mut self.guests {
            guest.exiting(event_loop);
        }
        self.host.exiting(event_loop);
    }
}
