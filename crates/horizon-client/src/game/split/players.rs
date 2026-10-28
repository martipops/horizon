//! Players coming and going: joining (P1's world through memory, or P1's server), picking a
//! profile, dropping out.
use super::*;

/// Why a player's game left its server, when the server said (its Disconnected or error screen).
pub(super) fn turned_away(game: &Game<'_>) -> Option<String> {
    use horizon_client::{multiplayer::Screen, title::Mode};
    let why = match &game.title.as_ref()?.mode {
        Mode::Multiplayer(Screen::Disconnected { reason, .. }) => reason.clone(),
        Mode::Error { message, .. } => message.clone(),
        _ => return None,
    };
    // An online-mode server: the other players have no Minecraft account of their own here.
    Some(if why.contains("verify username") {
        "This server needs each player's own Minecraft account".to_owned()
    } else {
        why
    })
}

/// The `Game` `start` makes (~67 KB, by value) into a box, in a frame of its own that is gone
/// once it is: the copies stay out of the caller's frame, deep in the event loop.
#[inline(never)]
fn boxed(start: impl FnOnce() -> Result<Option<Game<'static>>>) -> Result<Box<Game<'static>>> {
    match start() {
        Ok(Some(game)) => Ok(Box::new(game)),
        Ok(None) => Err("the player's game did not start".into()),
        Err(e) => Err(e),
    }
}

impl<'a> Split<'a> {
    /// A server's resource packs: the others answer as P1 did (asked once, by P1). A player
    /// joining after P1 answered is answered at once; one asked before waits for P1's answer.
    pub(super) fn follow_pack_answer(&mut self) {
        let Some(yes) = self.host.pack_answer() else {
            return;
        };
        for i in 0..self.guests.len() {
            if !self.guests[i].game.pack_prompt_open() {
                continue;
            }
            self.turn(i + 1);
            if let Err(e) = self.guests[i].game.answer_pack_prompt(yes) {
                log::warn!(
                    "Horizon: splitscreen: P{}'s server packs: {e}",
                    self.guests[i].seat
                );
            }
        }
        self.turn(0);
    }

    /// Add a player driven by `pad`: it picks a profile and where to play in its view first,
    /// unless `name` is given (tests).
    pub(super) fn join(&mut self, pad: Option<PadId>, name: Option<String>) -> Result<()> {
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
        // The launcher's offline names, and profiles made before (or without) it.
        let profiles = Self::folder()?.join("profiles");
        let mut names: Vec<String> = std::fs::read_dir(&profiles)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| horizon_client::title::valid_profile_name(n))
            .collect();
        for p in self.launcher.iter().flatten() {
            if !names.iter().any(|n| n.eq_ignore_ascii_case(&p.name)) {
                names.push(p.name.clone());
            }
        }
        // The other Microsoft accounts: a player signs in with one as itself.
        let accounts: Vec<String> = self.accounts.iter().map(|a| a.name.clone()).collect();
        names.retain(|n| !accounts.iter().any(|a| a.eq_ignore_ascii_case(n)));
        names.sort_by_key(|n| n.to_lowercase());
        names.extend(accounts.iter().cloned());
        let taken = self.taken();
        if let Some(title) = game.title.as_mut() {
            title.profile_picker(names, taken, accounts);
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
    pub(super) fn taken(&self) -> Vec<String> {
        std::iter::once(self.host.options.player_name())
            .chain(self.guests.iter().filter_map(|g| g.name.clone()))
            .collect()
    }

    /// Guest `i` plays as profile `name` where its view is now (moved with the bumpers): its
    /// options, then into P1's world.
    pub(super) fn enter(&mut self, i: usize, name: String) -> Result<()> {
        if self.taken().contains(&name) {
            return Err(format!("{name} is already playing").into());
        }
        // A Microsoft account signs in as itself (its own connections and skin).
        let account = self.accounts.iter().find(|a| a.name == name).cloned();
        self.guests[i].game.split_auth = account.clone();
        // One list: a name new to the launcher is kept there too, and plays with its skin.
        if let (None, Some(list)) = (&account, &mut self.launcher) {
            if !list.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
                launcher::run(&["add-offline", &name]);
                list.push(launcher::Profile {
                    name: name.clone(),
                    skin: None,
                    slim: false,
                });
            }
            launcher::wear(list, &name);
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
        // P1's own world through memory (`Ok`), else P1's server over the network (`Err`).
        let via = match &self.host.integrated {
            Some(server) => Ok(server
                .join_split(&name)
                .ok_or("P1's world does not let players in")?),
            None => Err(self.p1_server().ok_or("P1 is not in a world")?),
        };
        self.guests[i].options = options;
        self.turn(i + 1);
        let guest = &mut self.guests[i];
        settings.extras.agent_control &= guest.game.options.control.is_some();
        guest.game.settings = settings;
        guest.game.options.player_name = Some(name.clone());
        guest.game.apply_settings();
        let joined = match via {
            Ok(pipe) => guest.game.connect_local(pipe, &name),
            // A server that refuses it (online mode, the name taken) disconnects it: it drops
            // out and P1 hears why (`drop_out`).
            Err(address) => guest.game.connect_server(&address, &name),
        };
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
    pub(super) fn pick_profiles(&mut self) -> bool {
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

    /// A new player's game in its seat, on its title screen until it has a profile. Boxed as it
    /// comes out of `start_game`, and never inlined: a `Game` is ~67 KB, and copies of it in the
    /// event loop's frames overflowed Windows' 1 MB main stack.
    #[inline(never)]
    pub(super) fn start_guest(
        &mut self,
        seat: usize,
        options_file: &Path,
    ) -> Result<Box<Game<'static>>> {
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
        // P1's frame-limited test runs end the process; a player's game doesn't.
        options.frames = None;
        options.world_frames = None;
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
        let pack = host.pack.clone();
        let resources = host.options.resources.clone();
        let mut guest = boxed(|| {
            start_game(
                res,
                pack,
                gui,
                options,
                None,
                audio,
                resources,
                Instant::now(),
                None,
                Some(settings),
                None,
            )
        })?;
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
        // Its pad comes from the split from its first frame: it never opens the pads itself.
        guest.pad.feed = Some(PadState::default());
        Ok(guest)
    }

    /// Players who disconnected drop out; all of them once P1 has left the world.
    pub(super) fn drop_out(&mut self) -> bool {
        // Out of its world: on the title screen, or neither on its server thread nor a server's.
        let p1_left = self.host.title.is_some()
            || (self.host.integrated.is_none() && self.host.remote.is_none());
        let before = self.guests.len();
        // Out of the world: the others drop with it, which is no news (their connections may
        // have gone a frame before P1's: a notice raised then is wrong).
        if p1_left {
            self.host.split_hud.notice = None;
        }
        for k in (0..self.guests.len()).rev() {
            let g = &self.guests[k];
            // A player picking a profile has no connection yet.
            let playing = g.name.is_none() || g.game.remote.is_some();
            if p1_left || !playing || g.game.error.is_some() {
                // A server that turned it away (online mode, its name taken, full): P1 hears why.
                if let (false, Some(name), Some(why)) = (p1_left, &g.name, turned_away(&g.game)) {
                    self.host.split_hud.notice =
                        Some(Notice::new(format!("{name} could not stay"), why));
                }
                self.remove_guest(k);
            }
        }
        self.guests.len() != before
    }

    /// Guest `k` (player `k + 1`) leaves: keyboard and mouse stay with whoever had them (P1 if it
    /// was this one), the views keep their order for the layout's comparison, and the server
    /// forgets the name (the world may pause again).
    pub(super) fn remove_guest(&mut self, k: usize) {
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
}
