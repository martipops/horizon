//! Splitscreen spike (`HORIZON_SPLIT=1`): two players in one window. P1 is the game as launched;
//! once its world is open it goes to LAN and P2, a second `Game` in this process, joins it. Both
//! render offscreen on one device and the compositor puts P1 on the left half, P2 on the right.
//! Keyboard and mouse drive P1; P2 takes `horizon-ctl` on P1's socket path + "2".
use super::{Game, UiResources, build_renderer, start_game};
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
    guest: Option<Box<Game<'static>>>,
    compositor: Option<Compositor>,
    size: PhysicalSize<u32>,
}

impl<'a> Split<'a> {
    pub(super) fn new(mut host: Game<'a>) -> Self {
        host.options.offscreen = true;
        Self {
            host,
            guest: None,
            compositor: None,
            size: PhysicalSize::new(1, 1),
        }
    }

    /// P1 on the left half, P2 on the right.
    fn halves(&self) -> [PhysicalSize<u32>; 2] {
        let left = (self.size.width / 2).max(1);
        [
            PhysicalSize::new(left, self.size.height),
            PhysicalSize::new((self.size.width - left).max(1), self.size.height),
        ]
    }

    fn resize_views(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        let [left, right] = self.halves();
        self.host
            .window_event(event_loop, id, WindowEvent::Resized(left));
        if let Some(guest) = &mut self.guest {
            guest.window_event(event_loop, id, WindowEvent::Resized(right));
        }
    }

    /// P2 joins once P1 is playing in a world it can open to LAN.
    fn start_guest(&mut self) -> Result<Game<'static>> {
        let host = &mut self.host;
        host.lan_control(&["lan", "open", LAN_PORT])?;
        let mut options = host.options.clone();
        options.world = None;
        options.server = Some(format!("127.0.0.1:{LAN_PORT}"));
        options.player_name = Some("P2".into());
        options.control = options.control.map(|p| {
            let mut p = p.into_os_string();
            p.push("2");
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
        .ok_or("P2 did not start")?;
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
                if let Some(guest) = &mut self.guest {
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
        self.host.about_to_wait(event_loop);
        if self.guest.is_none()
            && self.compositor.is_some()
            && self.host.player.is_some()
            && self.host.title.is_none()
        {
            match self.start_guest() {
                Ok(guest) => {
                    self.guest = Some(Box::new(guest));
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
        if let Some(guest) = &mut self.guest {
            guest.about_to_wait(event_loop);
        }
        let left = self.halves()[0].width;
        let host_frame = self
            .host
            .renderer
            .as_ref()
            .and_then(|r| r.offscreen_frame());
        let guest_frame = self
            .guest
            .as_ref()
            .and_then(|g| g.renderer.as_ref()?.offscreen_frame());
        let views: Vec<_> = host_frame
            .map(|t| (t, [0, 0]))
            .into_iter()
            .chain(guest_frame.map(|t| (t, [left, 0])))
            .collect();
        if let Some(c) = &mut self.compositor {
            c.present(&views);
        }
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(guest) = &mut self.guest {
            guest.exiting(event_loop);
        }
        self.host.exiting(event_loop);
    }
}
