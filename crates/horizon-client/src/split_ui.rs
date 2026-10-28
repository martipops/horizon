//! The Splitscreen Setup screen: the monitors to scale with every window and player on them, a
//! draft to move players in, and nothing changed until Apply (`split_layout::Draft`). Horizon
//! Settings → Splitscreen... opens it, with the switch that turns splitscreen on (`on`, from the
//! settings). The game fills `live`, `monitors`, `main`, `names`, `me` and `host` while
//! splitscreen is on and it is open, and takes `apply` and `identify`.
use crate::{
    extras::{SPLIT_THREE, SPLIT_TWO},
    gamepad::Button as PadButton,
    graphics_style::STYLE,
    hud::{HudAtlas, HudBatch, rgba},
    split_layout::{Draft, Monitor, Placement, Spot, Warning},
    text::Text,
    ui,
};

#[derive(Default)]
pub struct SplitUi {
    /// Splitscreen is on (the options screen sets it from the settings).
    pub on: bool,
    /// The arrangement now.
    pub live: Option<Draft>,
    /// The monitors, left to right, and the name of the one Window 1 is on.
    pub monitors: Vec<Monitor>,
    pub main: String,
    /// Each player as shown on its tile: "P2 Ann".
    pub names: Vec<String>,
    /// Whose screen this is (0 = P1), and whether it arranges everyone (P1) or only itself.
    pub me: usize,
    pub host: bool,
    /// Apply pressed: the arrangement for the game to make (P1's screen), or where this player
    /// goes (another's: its draft may be older than P1's changes, so only its move counts).
    pub apply: Option<Draft>,
    pub apply_move: Option<(Spot, Option<usize>)>,
    /// Identify pressed: every view says which window it is in.
    pub identify: bool,
    /// The changes not applied yet.
    edit: Option<Draft>,
    selected: Option<usize>,
    /// This player's last move in the draft (another player's screen).
    moved: Option<(Spot, Option<usize>)>,
    /// A tile held by the mouse: whose, and where the pointer is (`None` until it moves).
    drag: Option<(usize, Option<(f32, f32)>)>,
    /// A player picked up with the controller (A), moved with the bumpers.
    held: Option<Held>,
    /// Dropped or put back: the player whose tile the controller's focus goes to.
    refocus: Option<usize>,
    /// The screen's size: at the last draw (the options screen sets it) or press.
    pub(crate) size: (f32, f32),
}

/// A player picked up with the controller: where it could go, the one picked so far, and the
/// draft with it there (the map shows it).
struct Held {
    player: usize,
    spots: Vec<(String, Drop)>,
    at: Option<usize>,
    preview: Option<Draft>,
}

/// Where a held player can go: a place (`Spot`), or another player's (they swap).
#[derive(Clone, Debug, PartialEq)]
enum Drop {
    At(Spot),
    Swap(usize),
    /// Place `k` among the players in a window (another player's screen: the bumpers' stops).
    Place(u8, usize),
}

/// What a press lands on.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    Player(usize),
    Move(Spot),
    Two,
    Three,
    Identify,
    Revert,
    Apply,
    Done,
    /// Splitscreen on or off (P1's).
    Switch,
}

/// What a press did, for the options screen.
pub enum Click {
    Nothing,
    Pressed,
    Done,
    /// Splitscreen on or off: the settings change.
    Switch,
}

/// A button: where, what it says, what it does, whether it can be pressed.
type Button = ([f32; 4], String, Target, bool);

/// The map's area: below the title, above the move row, the notes and the row of buttons.
const TOP: f32 = 30.0;
const BOTTOM: f32 = 80.0;
const ROW: f32 = 24.0;

/// How a layout's square button is drawn.
enum Glyph {
    Auto,
    /// The panes in a 14 x 10 window, 1 px apart.
    Panes(&'static [[f32; 4]]),
}

/// The glyph for a layout button's label ("2 sharing: Side by side"...).
fn layout_glyph(label: &str) -> Option<Glyph> {
    let (_, layout) = label.split_once(": ")?;
    Some(match (label.starts_with('2'), layout) {
        (true, "Auto") => Glyph::Auto,
        (true, "Side by side") => Glyph::Panes(&[[0.0, 0.0, 6.5, 10.0], [7.5, 0.0, 6.5, 10.0]]),
        (true, _) => Glyph::Panes(&[[0.0, 0.0, 14.0, 4.5], [0.0, 5.5, 14.0, 4.5]]),
        (false, "One wide on top") => Glyph::Panes(&[
            [0.0, 0.0, 14.0, 4.5],
            [0.0, 5.5, 6.5, 4.5],
            [7.5, 5.5, 6.5, 4.5],
        ]),
        (false, "One tall on the left") => Glyph::Panes(&[
            [0.0, 0.0, 6.5, 10.0],
            [7.5, 0.0, 6.5, 4.5],
            [7.5, 5.5, 6.5, 4.5],
        ]),
        (false, _) => Glyph::Panes(&[
            [0.0, 0.0, 4.0, 10.0],
            [5.0, 0.0, 4.0, 10.0],
            [10.0, 0.0, 4.0, 10.0],
        ]),
    })
}

/// What of `r` no rectangle of `covers` hides, as one rectangle: the largest side left by each
/// cover in turn (all of it hidden: no size).
fn uncovered(mut r: [f32; 4], covers: impl Iterator<Item = [f32; 4]>) -> [f32; 4] {
    for [cx, cy, cw, ch] in covers {
        let [x, y, w, h] = r;
        if cx >= x + w || cx + cw <= x || cy >= y + h || cy + ch <= y {
            continue;
        }
        let sides = [
            [x, y, cx - x, h],
            [cx + cw, y, x + w - (cx + cw), h],
            [x, y, w, cy - y],
            [x, cy + ch, w, y + h - (cy + ch)],
        ];
        r = sides
            .into_iter()
            .map(|[sx, sy, sw, sh]| [sx, sy, sw.max(0.0), sh.max(0.0)])
            .max_by(|a, b| (a[2] * a[3]).total_cmp(&(b[2] * b[3])))
            .unwrap_or([x, y, 0.0, 0.0]);
    }
    r
}

impl SplitUi {
    /// Opened: the arrangement as it is.
    pub fn reset(&mut self) {
        self.edit = None;
        self.moved = None;
        self.apply_move = None;
        self.selected = None;
        self.apply = None;
        self.drag = None;
        self.held = None;
    }

    /// The draft shown: with a held player where the D-pad put it, else the changes so far,
    /// else the arrangement now.
    fn draft(&self) -> Option<&Draft> {
        let preview = self.held.as_ref().and_then(|h| h.preview.as_ref());
        preview.or(self.edit.as_ref()).or(self.live.as_ref())
    }

    /// The controller (`pad_menu`), A at `at`: on a player's tile it may move, picks it up. True
    /// when it did (the press is taken).
    pub fn pad_pick_up(&mut self, at: (f32, f32)) -> bool {
        let (w, h) = self.size;
        let hit = self
            .buttons(w, h)
            .into_iter()
            .rev()
            .find(|([x, y, bw, bh], _, _, active)| *active && ui::hovered(at, *x, *y, *bw, *bh));
        let Some((_, _, Target::Player(i), _)) = hit else {
            return false;
        };
        self.selected = Some(i);
        let mut spots: Vec<(String, Drop)> = match self.host {
            true => self
                .spots()
                .into_iter()
                .map(|(label, spot)| (label, Drop::At(spot)))
                .collect(),
            false => self.stops(i),
        };
        // P1 also swaps it with another player (a swap moves that one too).
        if self.host {
            let players = self.draft().map_or(0, |d| d.players.len());
            for j in (0..players).filter(|&j| j != i) {
                let name = self
                    .names
                    .get(j)
                    .cloned()
                    .unwrap_or_else(|| format!("Player {}", j + 1));
                spots.push((format!("Swap with {name}"), Drop::Swap(j)));
            }
        }
        self.held = Some(Held {
            player: i,
            spots,
            at: None,
            preview: None,
        });
        true
    }

    /// The controller while a player is held: the bumpers or left and right cycle where it goes,
    /// as when joining (the map shows it there), A drops it there (into the draft), B puts it
    /// back. True when the button was taken: every button, while one is held.
    pub fn pad_button(&mut self, button: PadButton) -> bool {
        let Some(held) = &self.held else {
            return false;
        };
        let player = held.player;
        match button {
            PadButton::LeftBumper
            | PadButton::RightBumper
            | PadButton::DPadLeft
            | PadButton::DPadRight => {
                let n = held.spots.len();
                if n == 0 {
                    return true;
                }
                let back = matches!(button, PadButton::LeftBumper | PadButton::DPadLeft);
                let next = match held.at {
                    None if back => n - 1,
                    None => 0,
                    Some(k) if back => (k + n - 1) % n,
                    Some(k) => (k + 1) % n,
                };
                let (player, drop) = (held.player, held.spots[next].1.clone());
                let preview = self.edit.clone().or(self.live.clone()).map(|mut draft| {
                    match drop {
                        Drop::At(spot) => draft.place_player(player, spot),
                        Drop::Swap(j) => draft.swap(player, j),
                        Drop::Place(n, k) => {
                            draft.place_player(player, Spot::Window(n));
                            draft.put_at(player, k);
                        }
                    }
                    draft
                });
                if let Some(held) = &mut self.held {
                    held.at = Some(next);
                    held.preview = preview;
                }
            }
            PadButton::South => {
                let held = self.held.take().expect("held");
                // One highlight after: the focus on the player where it landed, not selected too.
                (self.selected, self.refocus) = (None, Some(held.player));
                match held.at.map(|k| held.spots[k].1.clone()) {
                    Some(Drop::At(spot)) => self.place(held.player, spot),
                    Some(Drop::Swap(j)) => self.swap(held.player, j),
                    Some(Drop::Place(n, k)) => self.place_at(held.player, n, k),
                    None => {}
                }
            }
            PadButton::East => {
                (self.selected, self.refocus) = (None, Some(player));
                self.held = None;
            }
            // While one is held, the focus stays put: A drops, it presses nothing else.
            _ => {}
        }
        true
    }

    /// The tile of the player just dropped or put back, for the controller's focus. Taken once.
    pub fn take_refocus(&mut self) -> Option<[f32; 4]> {
        let i = self.refocus.take()?;
        let (w, h) = self.size;
        self.buttons(w, h)
            .into_iter()
            .find(|b| b.2 == Target::Player(i))
            .map(|b| b.0)
    }

    /// Where another player's screen can put it, as its bumpers step: each place in each window
    /// ("Window 1 (2 of 3)"), a new window, a monitor no window fills.
    fn stops(&self, i: usize) -> Vec<(String, Drop)> {
        let Some(draft) = self.draft() else {
            return Vec::new();
        };
        let mut stops = Vec::new();
        for w in &draft.windows {
            let others = draft.sharing(i, w.number).len();
            if others == 0 {
                stops.push((
                    format!("Window {}", w.number),
                    Drop::At(Spot::Window(w.number)),
                ));
            }
            for k in (0..=others).filter(|_| others > 0) {
                let label = format!("Window {} ({} of {})", w.number, k + 1, others + 1);
                stops.push((label, Drop::Place(w.number, k)));
            }
        }
        stops.push(("New window".to_owned(), Drop::At(Spot::New)));
        for m in &self.monitors {
            let filled = draft
                .windows
                .iter()
                .any(|w| w.placement == Placement::Fill(m.name.clone()));
            if m.name != self.main && !filled {
                let label = format!("Fill {}", self.monitor_name(&m.name));
                stops.push((label, Drop::At(Spot::Fill(m.name.clone()))));
            }
        }
        stops
    }

    /// The draft to change: the changes so far, else the arrangement now.
    fn editing(&mut self) -> Option<&mut Draft> {
        if self.edit.is_none() {
            self.edit = self.live.clone();
        }
        self.edit.as_mut()
    }

    fn monitor_name(&self, name: &str) -> String {
        match self.monitors.iter().position(|m| m.name == name) {
            Some(n) => format!("Monitor {}", n + 1),
            None => "a monitor".to_owned(),
        }
    }

    /// Whether this screen may move player `i`: P1 moves anyone, the others themselves.
    fn may_move(&self, i: usize) -> bool {
        self.host || i == self.me
    }

    /// Every monitor's, window's and player's rectangle on a screen `w` x `h`: monitors to scale
    /// in the map area, each window where it would be, each player's tile as its view (with its
    /// window's number).
    #[allow(clippy::type_complexity)]
    fn map(&self, w: f32, h: f32) -> (Vec<[f32; 4]>, Vec<(u8, [f32; 4])>, Vec<(u8, [f32; 4])>) {
        let Some(draft) = self.draft() else {
            return Default::default();
        };
        let desk: Vec<[f64; 4]> = self.monitors.iter().map(Monitor::desktop).collect();
        let x0 = desk.iter().map(|d| d[0]).fold(f64::MAX, f64::min);
        let y0 = desk.iter().map(|d| d[1]).fold(f64::MAX, f64::min);
        let x1 = desk.iter().map(|d| d[0] + d[2]).fold(f64::MIN, f64::max);
        let y1 = desk.iter().map(|d| d[1] + d[3]).fold(f64::MIN, f64::max);
        if desk.is_empty() || x1 <= x0 || y1 <= y0 {
            return Default::default();
        }
        let (aw, ah) = (
            f64::from(w) - 40.0,
            // Room under the monitors for their names; the move row's too while nobody is picked.
            (f64::from(h - TOP - BOTTOM + if self.spots().is_empty() { ROW } else { 0.0 }) - 18.0)
                .max(10.0),
        );
        let k = (aw / (x1 - x0)).min(ah / (y1 - y0));
        let (ox, oy) = (
            (f64::from(w) - (x1 - x0) * k) / 2.0,
            f64::from(TOP) + 4.0 + (ah - (y1 - y0) * k) / 2.0,
        );
        let to_map = |[x, y, dw, dh]: [f64; 4]| {
            [
                (ox + (x - x0) * k) as f32,
                (oy + (y - y0) * k) as f32,
                (dw * k) as f32,
                (dh * k) as f32,
            ]
        };
        let monitors = desk.iter().copied().map(to_map).collect();
        // A window's size on the desktop: its pixels at the scale of the monitor it is on.
        let rects = draft.rects(&self.monitors, &self.main);
        let mut windows: Vec<(u8, [f32; 4])> = draft
            .windows
            .iter()
            .zip(&rects)
            .filter_map(|(win, rect)| {
                let ([x, y], [pw, ph]) = (*rect)?;
                let (x, y) = (f64::from(x), f64::from(y));
                let scale = self
                    .monitors
                    .iter()
                    .find(|m| {
                        let [mx, my, mw, mh] = m.desktop();
                        x >= mx && x < mx + mw && y >= my && y < my + mh
                    })
                    .or_else(|| self.monitors.iter().find(|m| m.name == self.main))
                    .map_or(1.0, |m| m.desktop()[2] / f64::from(m.size[0]));
                let size = [f64::from(pw) * scale, f64::from(ph) * scale];
                Some((win.number, to_map([x, y, size[0], size[1]])))
            })
            .collect();
        // Windows nobody plays in (they won't open) under the others.
        windows.sort_by_key(|(n, _)| draft.players.contains(n));
        let tiles = draft
            .preview(&self.monitors, &self.main)
            .iter()
            .map(|&(i, [vx, vy], size)| {
                let number = draft.windows.get(i).map_or(1, |w| w.number);
                let Some((_, [wx, wy, ww, wh])) = windows.iter().find(|(n, _)| *n == number) else {
                    return (number, [0.0; 4]);
                };
                let [pw, ph] = rects[i].map_or([1, 1], |r| r.1).map(|v| v.max(1) as f32);
                (
                    number,
                    [
                        wx + vx as f32 / pw * ww,
                        wy + vy as f32 / ph * wh,
                        size.width as f32 / pw * ww,
                        size.height as f32 / ph * wh,
                    ],
                )
            })
            .collect();
        (monitors, windows, tiles)
    }

    /// Where the selected player can go: the other windows, a new one, a monitor no window fills.
    fn spots(&self) -> Vec<(String, Spot)> {
        let (Some(draft), Some(i)) = (self.draft(), self.selected) else {
            return Vec::new();
        };
        let mine = draft.players.get(i).copied();
        let mut spots: Vec<(String, Spot)> = draft
            .windows
            .iter()
            .filter(|w| Some(w.number) != mine)
            .map(|w| (format!("Window {}", w.number), Spot::Window(w.number)))
            .collect();
        spots.push(("New window".to_owned(), Spot::New));
        for m in &self.monitors {
            let filled = draft
                .windows
                .iter()
                .any(|w| w.placement == Placement::Fill(m.name.clone()));
            if m.name != self.main && !filled {
                spots.push((
                    format!("Fill {}", self.monitor_name(&m.name)),
                    Spot::Fill(m.name.clone()),
                ));
            }
        }
        spots
    }

    /// Where a tile dropped at `(x, y)` goes: the window on top there, else the monitor's (a new
    /// window on Window 1's monitor, the one filling another or a new one filling it).
    fn drop_spot(&self, (x, y): (f32, f32), w: f32, h: f32) -> Option<Spot> {
        let draft = self.draft()?;
        let (monitors, windows, _) = self.map(w, h);
        let inside = |[rx, ry, rw, rh]: [f32; 4]| x >= rx && x < rx + rw && y >= ry && y < ry + rh;
        if let Some((n, _)) = windows.iter().rev().find(|(_, r)| inside(*r)) {
            return Some(Spot::Window(*n));
        }
        let m = &self.monitors[monitors.iter().position(|r| inside(*r))?];
        if m.name == self.main {
            return Some(Spot::New);
        }
        Some(
            match draft
                .windows
                .iter()
                .find(|w| w.placement == Placement::Fill(m.name.clone()))
            {
                Some(win) => Spot::Window(win.number),
                None => Spot::Fill(m.name.clone()),
            },
        )
    }

    /// Every button: the players' tiles (window by window, as they stack), the move row, the
    /// switch and the layouts (P1's), and Identify, Revert, Apply and Done. Splitscreen off: the
    /// switch and Done.
    fn buttons(&self, w: f32, h: f32) -> Vec<Button> {
        let row = |out: &mut Vec<Button>, y: f32, items: Vec<(f32, String, Target, bool)>| {
            let total: f32 = items.iter().map(|i| i.0 + 4.0).sum::<f32>() - 4.0;
            let mut x = ((w - total) / 2.0).floor();
            for (bw, label, target, active) in items {
                out.push(([x, y, bw, 20.0], label, target, active));
                x += bw + 4.0;
            }
        };
        let done = (64.0, "Done".into(), Target::Done, true);
        // The switch, top right (P1's).
        let switch = (
            [w - 110.0, 6.0, 104.0, 20.0],
            format!("Splitscreen: {}", if self.on { "On" } else { "Off" }),
            Target::Switch,
            true,
        );
        if !self.on {
            let mut out = vec![switch];
            row(&mut out, h - 27.0, vec![done]);
            return out;
        }
        let (_, windows, tiles) = self.map(w, h);
        let mut order: Vec<usize> = (0..tiles.len()).collect();
        order.sort_by_key(|&i| windows.iter().position(|(n, _)| *n == tiles[i].0));
        let mut out: Vec<Button> = order
            .into_iter()
            .map(|i| {
                let name = self
                    .names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("Player {}", i + 1));
                // The part no later window covers: what can be seen and pressed.
                let at = windows.iter().position(|(n, _)| *n == tiles[i].0);
                let covers = at.map_or(&[][..], |k| &windows[k + 1..]);
                let rect = uncovered(tiles[i].1, covers.iter().map(|(_, r)| *r));
                (rect, name, Target::Player(i), self.may_move(i))
            })
            .collect();
        let spots = self.spots();
        if !spots.is_empty() {
            let items = spots
                .into_iter()
                .map(|(label, spot)| (86.0, label, Target::Move(spot), true))
                .collect();
            row(&mut out, h - BOTTOM + 4.0, items);
        }
        // P1's: the switch, and a square for how 2 (or 3) players share a window, while a window
        // has that many (drawn as the layout; the label is for the control socket and the tests).
        let mut footer = Vec::new();
        if self.host
            && let Some(draft) = self.draft()
        {
            out.push(switch);
            let sharing = |k: usize| {
                draft
                    .windows
                    .iter()
                    .any(|win| draft.players.iter().filter(|&&n| n == win.number).count() == k)
            };
            if sharing(2) {
                let two = SPLIT_TWO[usize::from(draft.two.min(2))];
                footer.push((20.0, format!("2 sharing: {two}"), Target::Two, true));
            }
            if sharing(3) {
                let three = SPLIT_THREE[usize::from(draft.three.min(2))];
                footer.push((20.0, format!("3 sharing: {three}"), Target::Three, true));
            }
        }
        // Narrow enough for two squares in the narrowest view (320 GUI pixels).
        let editing = self.edit.is_some();
        footer.extend([
            (64.0, "Identify".into(), Target::Identify, true),
            (64.0, "Revert".into(), Target::Revert, editing),
            (64.0, "Apply".into(), Target::Apply, editing),
            done,
        ]);
        row(&mut out, h - 27.0, footer);
        out
    }

    /// What a button does, for the line under the map while the pointer (or the controller's
    /// focus) is on it; for one that can't be pressed, why not.
    fn help(&self, (_, label, target, active): &Button) -> Option<String> {
        let me = self.names.get(self.me).map_or("you", String::as_str);
        Some(match target {
            Target::Player(_) if *active => format!("Click or drag {label} to move them."),
            Target::Player(_) => format!("Only {me}'s own place can be changed here."),
            Target::Move(_) => return None,
            Target::Two if label.ends_with("Auto") => {
                format!("{label} (side by side when the window is wide). Press to change.")
            }
            Target::Two | Target::Three => format!("{label}. Press to change."),
            Target::Switch => "Splitscreen for this computer, on or off.".to_owned(),
            Target::Identify => "Each player's view shows which window it is in.".to_owned(),
            Target::Revert if *active => "Undo the changes not applied yet.".to_owned(),
            Target::Apply if *active => "Play the way the map shows.".to_owned(),
            Target::Revert | Target::Apply => "Nothing changed yet.".to_owned(),
            Target::Done => return None,
        })
    }

    fn warnings(&self) -> Vec<String> {
        let Some(draft) = self.draft() else {
            return Vec::new();
        };
        draft
            .warnings(&self.monitors, &self.main)
            .into_iter()
            .map(|w| match w {
                Warning::Empty(1) => "Nobody plays in Window 1: it hides".to_owned(),
                Warning::Empty(n) => format!("Nobody plays in Window {n}: it won't open"),
                Warning::OnWindow1(n) => {
                    format!("Window {n} fills Window 1's monitor: its players play in Window 1")
                }
                Warning::Unplugged(n, _) => {
                    format!("Window {n}'s monitor is not connected: its players play in Window 1")
                }
            })
            .collect()
    }

    pub fn draw(&self, batch: &mut HudBatch, atlas: &HudAtlas, cursor: (f32, f32), in_world: bool) {
        let (w, h) = (batch.width, batch.height);
        ui::menu_background(batch, atlas, 0.0, 0.0, w, h, in_world);
        STYLE.title(batch, atlas, &Text::literal("Splitscreen Setup"));
        if !self.on {
            let grey = rgba(170, 170, 170, 255);
            for (k, line) in [
                "Splitscreen is off.",
                "Turn it on, then in a world press a button on another",
                "controller to add a player.",
            ]
            .into_iter()
            .enumerate()
            {
                let y = (h / 2.0 - 30.0 + 12.0 * k as f32).floor();
                ui::centered_text(batch, atlas, &Text::literal(line), 0.0, y, w, grey);
            }
            for b in &self.buttons(w, h) {
                self.draw_button(batch, atlas, cursor, b);
            }
            return;
        }
        let (monitors, windows, tiles) = self.map(w, h);
        let grey = rgba(160, 160, 160, 255);
        for &[x, y, mw, mh] in &monitors {
            batch.fill(atlas, x, y, mw, mh, rgba(255, 255, 255, 60));
            batch.fill(
                atlas,
                x + 1.0,
                y + 1.0,
                mw - 2.0,
                mh - 2.0,
                rgba(0, 0, 0, 170),
            );
        }
        let buttons = self.buttons(w, h);
        let yellow = rgba(255, 255, 85, 255);
        // Each window, then its players: a window over another covers it.
        for &(number, [x, y, ww, wh]) in &windows {
            batch.fill(atlas, x, y, ww, wh, rgba(255, 255, 85, 200));
            batch.fill(
                atlas,
                x + 1.0,
                y + 1.0,
                ww - 2.0,
                wh - 2.0,
                rgba(40, 40, 40, 230),
            );
            for b in buttons
                .iter()
                .filter(|b| matches!(b.2, Target::Player(i) if tiles[i].0 == number))
            {
                self.draw_button(batch, atlas, cursor, b);
            }
        }
        // The windows' names over everything, so another window's tiles never hide them.
        for (number, [x, y, _, _]) in windows {
            let label = Text::literal(format!("Window {number}"));
            batch.text_styled(atlas, &label, x + 3.0, y - 10.0, yellow, true);
        }
        for b in buttons.iter().filter(|b| !matches!(b.2, Target::Player(_))) {
            self.draw_button(batch, atlas, cursor, b);
        }
        // The tile being dragged, under the pointer.
        if let Some((i, Some((x, y)))) = self.drag
            && let Some(name) = self.names.get(i)
        {
            let label = Text::literal(name.as_str());
            ui::button(
                batch,
                atlas,
                x - 30.0,
                y - 10.0,
                60.0,
                20.0,
                &label,
                true,
                true,
            );
        }
        // The monitors' names under them, where no window covers them.
        for (n, [x, y, _, mh]) in monitors.into_iter().enumerate() {
            let label = Text::literal(format!("Monitor {}", n + 1));
            batch.text_styled(atlas, &label, x + 1.0, y + mh + 2.0, grey, true);
        }
        self.draw_notes(batch, atlas, cursor, &buttons);
    }

    fn draw_button(
        &self,
        batch: &mut HudBatch,
        atlas: &HudAtlas,
        cursor: (f32, f32),
        ([x, y, bw, bh], label, target, active): &Button,
    ) {
        let (mut x, mut y, mut bw, mut bh) = (*x, *y, *bw, *bh);
        // While a player is held, only its tile is lit: the controller's pointer stays where
        // the pick-up was, over whichever tile is there now.
        let hover = *active && self.held.is_none() && ui::hovered(cursor, x, y, bw, bh);
        let selected = matches!(target, Target::Player(i) if Some(*i) == self.selected);
        if matches!(target, Target::Player(_)) {
            // A tile: inset so neighbours stay apart.
            (x, y, bw, bh) = (x + 1.0, y + 1.0, (bw - 2.0).max(4.0), (bh - 2.0).max(4.0));
        }
        let glyph = match target {
            Target::Two | Target::Three => layout_glyph(label),
            _ => None,
        };
        let text = Text::literal(if glyph.is_some() { "" } else { label.as_str() });
        ui::button(
            batch,
            atlas,
            x,
            y,
            bw,
            bh,
            &text,
            *active,
            hover || selected,
        );
        // A layout's square: its panes, in a 14 x 10 window; Auto a letter. Its name for the
        // control socket, which lists widgets by what they say.
        if glyph.is_some() {
            crate::widgets::record(label, [x, y, bw, bh], *active);
        }
        let (gx, gy) = (
            (x + (bw - 14.0) / 2.0).floor(),
            (y + (bh - 10.0) / 2.0).floor(),
        );
        match glyph {
            Some(Glyph::Auto) => {
                let a = Text::literal("A");
                ui::centered_text(batch, atlas, &a, x, y + 6.0, bw, rgba(255, 255, 255, 255));
            }
            Some(Glyph::Panes(panes)) => {
                for &[px, py, pw, ph] in panes {
                    batch.fill(atlas, gx + px, gy + py, pw, ph, rgba(230, 230, 230, 255));
                }
            }
            None => {}
        }
    }

    /// What to do next (or what the button under the pointer does), and the warnings.
    fn draw_notes(
        &self,
        batch: &mut HudBatch,
        atlas: &HudAtlas,
        cursor: (f32, f32),
        buttons: &[Button],
    ) {
        let (w, h) = (batch.width, batch.height);
        let held = self.held.as_ref().map(|h| {
            let who = self
                .names
                .get(h.player)
                .map_or("this player", String::as_str);
            match h.at {
                Some(k) => format!(
                    "{who} to {}: A drops it here, B puts it back.",
                    h.spots[k].0
                ),
                None => {
                    format!("{who} picked up: left/right picks where, A drops, B puts it back.")
                }
            }
        });
        // The pointer's button; disabled ones too, to say why. Not the selected player's tile:
        // the pointer is still on it after the click, and the question about it matters more.
        let help = buttons
            .iter()
            .filter(|b| ui::hovered(cursor, b.0[0], b.0[1], b.0[2], b.0[3]))
            .filter(|b| !matches!(b.2, Target::Player(i) if Some(i) == self.selected))
            .find_map(|b| self.help(b));
        let note = match self.selected {
            _ if held.is_some() => held.unwrap_or_default(),
            _ if help.is_some() => help.unwrap_or_default(),
            None if self.host => "Click a player, or drag it to a window or a monitor.".to_owned(),
            None => "Drag yourself onto a player to take their place, or to a window or a monitor."
                .to_owned(),
            Some(_) if self.edit.is_some() => "Apply to play this way, Revert to undo.".to_owned(),
            Some(i) => format!(
                "Where should {} play?",
                self.names.get(i).map_or("this player", String::as_str)
            ),
        };
        let warnings = self.warnings();
        let lines = std::iter::once((note, rgba(170, 170, 170, 255)))
            .chain(warnings.into_iter().map(|w| (w, rgba(255, 170, 85, 255))))
            .take(2);
        let mut y = h - BOTTOM + 28.0;
        for (line, color) in lines {
            ui::centered_text(batch, atlas, &Text::literal(line), 0.0, y, w, color);
            y += 11.0;
        }
    }

    /// A press at `(x, y)` on a screen `w` x `h`, or the mouse moving with it held (`drag`).
    pub fn click(&mut self, (x, y): (f32, f32), (w, h): (f32, f32), drag: bool) -> Click {
        self.size = (w, h);
        // The mouse takes over from a player held with the controller.
        if !drag {
            self.held = None;
        }
        if drag {
            if let Some((_, at)) = &mut self.drag {
                *at = Some((x, y));
            }
            return Click::Nothing;
        }
        // The topmost: the last drawn.
        let hit = self
            .buttons(w, h)
            .into_iter()
            .rev()
            .find(|([bx, by, bw, bh], _, _, active)| {
                *active && ui::hovered((x, y), *bx, *by, *bw, *bh)
            });
        let Some((_, _, target, _)) = hit else {
            return Click::Nothing;
        };
        match target {
            Target::Player(i) => {
                self.selected = Some(i);
                self.drag = Some((i, None));
            }
            Target::Move(spot) => {
                if let Some(i) = self.selected.filter(|&i| self.may_move(i)) {
                    self.place(i, spot);
                }
            }
            Target::Two => {
                if let Some(draft) = self.editing() {
                    draft.two = (draft.two + 1) % 3;
                }
            }
            Target::Three => {
                if let Some(draft) = self.editing() {
                    draft.three = (draft.three + 1) % 3;
                }
            }
            Target::Identify => self.identify = true,
            Target::Revert => {
                self.edit = None;
                self.moved = None;
            }
            Target::Apply => {
                let draft = self.edit.take();
                match self.host {
                    true => self.apply = draft,
                    false => self.apply_move = self.moved.take(),
                }
            }
            Target::Done => {
                self.reset();
                return Click::Done;
            }
            Target::Switch => {
                self.reset();
                return Click::Switch;
            }
        }
        Click::Pressed
    }

    /// The mouse let go: a tile dragged somewhere goes there (a draft). True when it moved.
    pub fn release(&mut self) -> bool {
        let (w, h) = self.size;
        let Some((i, Some(at))) = self.drag.take() else {
            return false;
        };
        if !self.may_move(i) {
            return false;
        }
        // On another player's tile: they swap (P1's screen), or this player takes that place and
        // the others move along (another's).
        let on = self
            .buttons(w, h)
            .into_iter()
            .rev()
            .find(|([x, y, bw, bh], _, _, _)| ui::hovered(at, *x, *y, *bw, *bh));
        if let Some((_, _, Target::Player(j), _)) = on
            && j != i
        {
            if self.host {
                self.swap(i, j);
            } else if let Some(draft) = self.draft() {
                let n = draft.players[j];
                let k = draft
                    .sharing(i, n)
                    .iter()
                    .position(|&p| p == j)
                    .unwrap_or(0);
                self.place_at(i, n, k);
            }
            return true;
        }
        let Some(spot) = self.drop_spot(at, w, h) else {
            return false;
        };
        if self.draft().and_then(|d| d.players.get(i).copied())
            == Some(match &spot {
                Spot::Window(n) => *n,
                _ => 0,
            })
        {
            return false;
        }
        self.place(i, spot);
        true
    }

    /// Player `i` to `spot` in the draft.
    fn place(&mut self, i: usize, spot: Spot) {
        if !self.host {
            self.moved = Some((spot.clone(), None));
        }
        if let Some(draft) = self.editing() {
            draft.place_player(i, spot);
        }
    }

    /// Player `i` to place `k` among the players of window `n` in the draft.
    fn place_at(&mut self, i: usize, n: u8, k: usize) {
        self.place(i, Spot::Window(n));
        if !self.host {
            self.moved = Some((Spot::Window(n), Some(k)));
        }
        if let Some(draft) = self.editing() {
            draft.put_at(i, k);
        }
    }

    /// Players `i` and `j` trade places in the draft (P1's screen: it moves another player).
    fn swap(&mut self, i: usize, j: usize) {
        if !self.host {
            return;
        }
        if let Some(draft) = self.editing() {
            draft.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (f32, f32) = (427.0, 300.0);

    fn screen(players: usize) -> SplitUi {
        SplitUi {
            live: Some(Draft::everyone_here(
                players,
                Placement::Free(Some(([0, 0], [2000, 1200]))),
                0,
                0,
            )),
            monitors: vec![
                Monitor {
                    name: "Built-in".into(),
                    at: [0, 0],
                    size: [3024, 1964],
                    scale: 2.0,
                },
                Monitor {
                    name: "TV".into(),
                    at: [3024, 0],
                    size: [1920, 1080],
                    scale: 1.0,
                },
            ],
            main: "Built-in".into(),
            names: (1..=players).map(|n| format!("P{n}")).collect(),
            host: true,
            on: true,
            ..Default::default()
        }
    }

    fn labels(ui: &SplitUi) -> Vec<String> {
        ui.buttons(SIZE.0, SIZE.1)
            .into_iter()
            .map(|b| b.1)
            .collect()
    }

    #[test]
    fn off_only_the_switch_and_done() {
        let mut ui = screen(2);
        ui.on = false;
        assert_eq!(labels(&ui), ["Splitscreen: Off", "Done"]);
        let r = rect(&ui, "Splitscreen: Off");
        assert!(matches!(
            ui.click((r[0] + 1.0, r[1] + 1.0), SIZE, false),
            Click::Switch
        ));
    }

    #[test]
    fn the_switch_is_p1s() {
        let mut ui = screen(2);
        assert!(labels(&ui).contains(&"Splitscreen: On".to_owned()));
        ui.host = false;
        assert!(!labels(&ui).iter().any(|l| l.starts_with("Splitscreen")));
    }

    fn rect(ui: &SplitUi, label: &str) -> [f32; 4] {
        ui.buttons(SIZE.0, SIZE.1)
            .into_iter()
            .find(|(_, l, _, _)| l == label)
            .unwrap_or_else(|| panic!("no {label}"))
            .0
    }

    fn press(ui: &mut SplitUi, label: &str) {
        let r = rect(ui, label);
        ui.click((r[0] + 1.0, r[1] + 1.0), SIZE, false);
    }

    #[test]
    fn move_a_player_then_apply() {
        let mut ui = screen(2);
        press(&mut ui, "P2");
        press(&mut ui, "Fill Monitor 2");
        // Nothing applied yet: the draft only.
        assert_eq!(ui.apply, None);
        assert_eq!(ui.live.as_ref().unwrap().players, [1, 1]);
        press(&mut ui, "Apply");
        let applied = ui.apply.take().unwrap();
        assert_eq!(applied.players, [1, 2]);
        assert_eq!(applied.windows[1].placement, Placement::Fill("TV".into()));
    }

    #[test]
    fn revert_drops_the_draft() {
        let mut ui = screen(2);
        press(&mut ui, "P2");
        press(&mut ui, "New window");
        assert!(ui.edit.is_some());
        press(&mut ui, "Revert");
        assert!(ui.edit.is_none());
        // Done closes and forgets the selection.
        press(&mut ui, "Done");
        assert_eq!(ui.selected, None);
    }

    #[test]
    fn tiles_split_the_window_on_the_map() {
        let ui = screen(2);
        let (monitors, windows, tiles) = ui.map(SIZE.0, SIZE.1);
        assert_eq!(monitors.len(), 2);
        assert_eq!(windows.len(), 1);
        // Two halves of Window 1, side by side.
        let (a, b) = (tiles[0].1, tiles[1].1);
        assert!((a[2] - b[2]).abs() < 0.01);
        assert!((a[0] + a[2] - b[0]).abs() < 0.01);
    }

    #[test]
    fn a_window_over_another_takes_the_click() {
        let mut ui = screen(2);
        // P2 in a new window, opened over Window 1's corner.
        press(&mut ui, "P2");
        press(&mut ui, "New window");
        press(&mut ui, "Apply");
        ui.live = ui.apply.take();
        press(&mut ui, "P2");
        assert_eq!(ui.selected, Some(1));
    }

    fn centre(ui: &SplitUi, label: &str) -> (f32, f32) {
        let [x, y, w, h] = rect(ui, label);
        (x + w / 2.0, y + h / 2.0)
    }

    #[test]
    fn a_controller_picks_a_player_up_moves_it_and_drops_it() {
        let mut ui = screen(2);
        ui.size = SIZE;
        assert!(ui.pad_pick_up(centre(&ui, "P2")));
        // Nothing moves until the D-pad does; then the map shows it there (New window first).
        assert_eq!(ui.draft().unwrap().players, [1, 1]);
        assert!(ui.pad_button(PadButton::RightBumper));
        assert_eq!(ui.draft().unwrap().players, [1, 2]);
        assert_eq!(ui.edit, None);
        // Right again: filling the TV; A drops it there, into the draft.
        assert!(ui.pad_button(PadButton::RightBumper));
        assert!(ui.pad_button(PadButton::South));
        let draft = ui.edit.as_ref().unwrap();
        assert_eq!(draft.players, [1, 2]);
        assert_eq!(draft.windows[1].placement, Placement::Fill("TV".into()));
        // Not held any more: the buttons go back to the menu.
        assert!(!ui.pad_button(PadButton::RightBumper));
    }

    #[test]
    fn a_controller_swaps_two_players() {
        let mut ui = screen(2);
        ui.size = SIZE;
        assert!(ui.pad_pick_up(centre(&ui, "P2")));
        // Step to "Swap with P1" (after the windows and monitors).
        while !ui
            .held
            .as_ref()
            .and_then(|h| h.at.map(|k| h.spots[k].0.clone()))
            .is_some_and(|label| label == "Swap with P1")
        {
            assert!(ui.pad_button(PadButton::RightBumper));
        }
        assert!(ui.pad_button(PadButton::South));
        assert_eq!(ui.edit.as_ref().unwrap().order, [1, 0]);
        // Another player's screen offers no swap: it would move someone else.
        let mut other = screen(2);
        other.size = SIZE;
        (other.host, other.me) = (false, 1);
        assert!(other.pad_pick_up(centre(&other, "P2")));
        let held = other.held.as_ref().unwrap();
        assert!(!held.spots.iter().any(|(l, _)| l.starts_with("Swap")));
    }

    #[test]
    fn b_puts_a_held_player_back() {
        let mut ui = screen(2);
        ui.size = SIZE;
        assert!(ui.pad_pick_up(centre(&ui, "P2")));
        ui.pad_button(PadButton::RightBumper);
        assert!(ui.pad_button(PadButton::East));
        assert_eq!(ui.edit, None);
        assert_eq!(ui.draft().unwrap().players, [1, 1]);
    }

    #[test]
    fn another_player_picks_up_only_itself() {
        let mut ui = screen(3);
        ui.size = SIZE;
        (ui.host, ui.me) = (false, 1);
        assert!(!ui.pad_pick_up(centre(&ui, "P3")));
        assert!(ui.pad_pick_up(centre(&ui, "P2")));
    }

    #[test]
    fn another_player_steps_through_the_places_with_the_bumpers() {
        let mut ui = screen(3);
        ui.size = SIZE;
        (ui.host, ui.me) = (false, 1);
        assert!(ui.pad_pick_up(centre(&ui, "P2")));
        let stops: Vec<String> = ui
            .held
            .as_ref()
            .unwrap()
            .spots
            .iter()
            .map(|s| s.0.clone())
            .collect();
        assert_eq!(
            stops,
            [
                "Window 1 (1 of 3)",
                "Window 1 (2 of 3)",
                "Window 1 (3 of 3)",
                "New window",
                "Fill Monitor 2"
            ]
        );
        // Left and right cycle, as the bumpers do; up and down are taken (the focus stays).
        assert!(ui.pad_button(PadButton::DPadDown));
        assert!(ui.pad_button(PadButton::DPadRight));
        assert!(ui.pad_button(PadButton::DPadLeft));
        // RB to the first place: before P1, shown on the map, dropped with A.
        assert!(ui.pad_button(PadButton::RightBumper));
        assert_eq!(ui.draft().unwrap().order[..2], [1, 0]);
        assert!(ui.pad_button(PadButton::South));
        press(&mut ui, "Apply");
        assert_eq!(ui.apply_move, Some((Spot::Window(1), Some(0))));
    }

    #[test]
    fn dropped_or_put_back_one_highlight_and_the_focus_follows() {
        let mut ui = screen(2);
        ui.size = SIZE;
        assert!(ui.pad_pick_up(centre(&ui, "P2")));
        assert!(ui.pad_button(PadButton::RightBumper)); // to a new window
        assert!(ui.pad_button(PadButton::South));
        // Not selected (the focus is its highlight), and the focus goes to its new tile.
        assert_eq!(ui.selected, None);
        assert_eq!(ui.take_refocus(), Some(rect(&ui, "P2")));
        assert_eq!(ui.take_refocus(), None);
        // Put back: the same.
        assert!(ui.pad_pick_up(centre(&ui, "P2")));
        assert!(ui.pad_button(PadButton::East));
        assert_eq!(
            (ui.selected, ui.take_refocus()),
            (None, Some(rect(&ui, "P2")))
        );
    }

    #[test]
    fn another_player_dropped_on_a_tile_takes_that_place() {
        let mut ui = screen(3);
        (ui.host, ui.me) = (false, 2);
        let (p3, p1) = (rect(&ui, "P3"), rect(&ui, "P1"));
        ui.click((p3[0] + 2.0, p3[1] + 2.0), SIZE, false);
        ui.click((p1[0] + 2.0, p1[1] + 2.0), SIZE, true);
        assert!(ui.release());
        // P3 first, the others along; nobody else changed window.
        let draft = ui.edit.as_ref().unwrap();
        assert_eq!(
            (draft.players.clone(), draft.order.clone()),
            (vec![1, 1, 1], vec![2, 0, 1])
        );
    }

    #[test]
    fn a_tile_under_a_window_is_pressed_where_it_shows() {
        // The left half of a tile hidden: its right half is left.
        let r = uncovered([0.0, 0.0, 10.0, 4.0], [[-5.0, -1.0, 10.0, 9.0]].into_iter());
        assert_eq!(r, [5.0, 0.0, 5.0, 4.0]);
        // All of it hidden: nothing to press.
        let r = uncovered([0.0, 0.0, 10.0, 4.0], [[-1.0, -1.0, 20.0, 9.0]].into_iter());
        assert_eq!(r[2] * r[3], 0.0);
        // P3 in a new window over the middle of Window 1: P2's tile, partly under it, still
        // answers at the middle of what shows of it.
        let mut ui = screen(3);
        press(&mut ui, "P3");
        press(&mut ui, "New window");
        press(&mut ui, "Apply");
        ui.live = ui.apply.take();
        let [x, y, w, h] = rect(&ui, "P2");
        ui.click((x + w / 2.0, y + h / 2.0), SIZE, false);
        assert_eq!(ui.selected, Some(1));
    }

    #[test]
    fn drag_a_player_onto_a_monitor() {
        let mut ui = screen(3);
        let (monitors, _, _) = ui.map(SIZE.0, SIZE.1);
        let tv = monitors[1];
        let p3 = rect(&ui, "P3");
        ui.click((p3[0] + 2.0, p3[1] + 2.0), SIZE, false);
        ui.click((tv[0] + 5.0, tv[1] + 5.0), SIZE, true);
        assert!(ui.release());
        assert_eq!(ui.edit.as_ref().unwrap().players, [1, 1, 2]);
        // P2 dropped on P3 (its tile fills that window): they swap.
        let p2 = rect(&ui, "P2");
        ui.click((p2[0] + 2.0, p2[1] + 2.0), SIZE, false);
        ui.click((tv[0] + 5.0, tv[1] + 5.0), SIZE, true);
        assert!(ui.release());
        assert_eq!(ui.edit.as_ref().unwrap().players, [1, 2, 1]);
        // A click without moving only selects.
        let p1 = rect(&ui, "P1");
        ui.click((p1[0] + 2.0, p1[1] + 2.0), SIZE, false);
        assert!(!ui.release());
    }

    #[test]
    fn layout_squares_show_while_a_window_has_that_many() {
        let labels = |ui: &SplitUi| labels(ui).into_iter().filter(|l| l.contains("sharing"));
        // Three in Window 1: only the 3's square, drawn as its panes.
        let mut ui = screen(3);
        assert_eq!(
            labels(&ui).collect::<Vec<_>>(),
            ["3 sharing: One wide on top"]
        );
        press(&mut ui, "3 sharing: One wide on top");
        assert_eq!(ui.edit.as_ref().unwrap().three, 1);
        assert!(matches!(
            layout_glyph("3 sharing: One tall on the left"),
            Some(Glyph::Panes(p)) if p.len() == 3
        ));
        // Two: only the 2's, Auto a letter, explained under the pointer.
        let ui = screen(2);
        assert_eq!(labels(&ui).collect::<Vec<_>>(), ["2 sharing: Auto"]);
        assert!(matches!(layout_glyph("2 sharing: Auto"), Some(Glyph::Auto)));
        let buttons = ui.buttons(SIZE.0, SIZE.1);
        let two = buttons.iter().find(|b| b.2 == Target::Two).unwrap();
        assert_eq!(
            ui.help(two).as_deref(),
            Some("2 sharing: Auto (side by side when the window is wide). Press to change.")
        );
        // Alone: neither.
        assert_eq!(labels(&screen(1)).count(), 0);
    }

    #[test]
    fn another_player_moves_only_itself() {
        let mut ui = screen(3);
        ui.host = false;
        ui.me = 1;
        let buttons = ui.buttons(SIZE.0, SIZE.1);
        let active = |label: &str| buttons.iter().any(|b| b.1 == label && b.3);
        assert!(active("P2") && !active("P3") && !active("P1"));
        assert!(!buttons.iter().any(|b| b.1 == "Everyone here"));
        press(&mut ui, "P3");
        assert_eq!(ui.selected, None);
        // A selection left from before someone left: no move for another player.
        ui.selected = Some(2);
        press(&mut ui, "New window");
        assert_eq!(ui.edit, None);
        // Its Apply says only where it goes.
        press(&mut ui, "P2");
        press(&mut ui, "New window");
        press(&mut ui, "Apply");
        assert_eq!(
            (ui.apply.as_ref(), ui.apply_move.as_ref()),
            (None, Some(&(Spot::New, None)))
        );
    }
}
