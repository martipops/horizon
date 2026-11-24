//! Project a draft arrangement onto the Setup screen, without opening a window.
use crate::split_layout::{Draft, Monitor, Placement};

/// Map area: below the title and above the move row and footer.
pub const TOP: f32 = 30.0;
pub const BOTTOM: f32 = 80.0;
/// Between windows whose place nobody knows, in rows on the map (desktop pixels).
const SHELF_GAP: f64 = 16.0;
/// Another row of them only for a map this much larger: it holds still otherwise.
const ROW_GAIN: f64 = 1.05;
/// The least of a player's tile left to see and press, where the tile is as big itself.
const MIN_TILE: [f32; 2] = [24.0, 14.0];
pub type Rect = [f32; 4];
pub type Map = (Vec<Rect>, Vec<(u8, Rect)>, Vec<(u8, Rect)>);

fn overlap([ax, ay, aw, ah]: [f64; 4], [bx, by, bw, bh]: [f64; 4]) -> bool {
    ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
}

/// Windows of these sizes in `rows` rows from `start`, each past every monitor a window fills.
fn shelve(sizes: &[[f64; 2]], rows: usize, start: [f64; 2], filled: &[[f64; 4]]) -> Vec<[f64; 4]> {
    let mut y = start[1];
    let mut out = Vec::with_capacity(sizes.len());
    for row in sizes.chunks(sizes.len().div_ceil(rows).max(1)) {
        let mut x = start[0];
        let mut below = y;
        for &[w, h] in row {
            let mut rect = [x, y, w, h];
            // Each step passes a filled monitor for good, so this ends.
            while let Some(f) = filled.iter().find(|f| overlap(rect, **f)) {
                rect[0] = f[0] + f[2] + SHELF_GAP;
            }
            x = rect[0] + w + SHELF_GAP;
            below = below.max(y + h + SHELF_GAP);
            out.push(rect);
        }
        y = below;
    }
    out
}

/// The rectangle around all of these.
fn bounds(rects: impl Iterator<Item = [f64; 4]>) -> [f64; 4] {
    let mut out = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for [x, y, w, h] in rects {
        out = [
            out[0].min(x),
            out[1].min(y),
            out[2].max(x + w),
            out[3].max(y + h),
        ];
    }
    out
}

/// Where the map is drawn, in UI coordinates.
fn area(w: f32, h: f32) -> [f64; 4] {
    [
        20.0,
        f64::from(TOP) + 4.0,
        f64::from(w) - 40.0,
        // Room under the monitors for their names, and always for the move row: the map
        // doesn't jump when a player is picked (a drop back to where its tile was misses).
        (f64::from(h - TOP - BOTTOM) - 18.0).max(10.0),
    ]
}

/// Monitor, window and player rectangles in UI coordinates, in their drawing order. `unplaced`: the
/// system doesn't say where free windows are (Wayland), so they go in rows from the first monitor
/// no window fills.
pub fn project(
    draft: &Draft,
    monitors: &[Monitor],
    main: &str,
    unplaced: bool,
    w: f32,
    h: f32,
) -> Map {
    let desk: Vec<[f64; 4]> = monitors.iter().map(Monitor::desktop).collect();
    // A window's size on the desktop: its pixels at the scale of the monitor it is on.
    let rects = draft.rects(monitors, main);
    let mut on_desk: Vec<(u8, [f64; 4])> = draft
        .windows
        .iter()
        .zip(&rects)
        .filter_map(|(win, rect)| {
            let ([x, y], [pw, ph]) = (*rect)?;
            let (x, y) = (f64::from(x), f64::from(y));
            let scale = monitors
                .iter()
                .find(|m| {
                    let [mx, my, mw, mh] = m.desktop();
                    x >= mx && x < mx + mw && y >= my && y < my + mh
                })
                .or_else(|| monitors.iter().find(|m| m.name == main))
                .map_or(1.0, |m| m.desktop()[2] / f64::from(m.size[0]));
            Some((
                win.number,
                [x, y, f64::from(pw) * scale, f64::from(ph) * scale],
            ))
        })
        .collect();
    // A window whose place nobody knows is never shown under one that fills a monitor.
    let filled: Vec<[f64; 4]> = draft
        .windows
        .iter()
        .filter(|win| matches!(win.placement, Placement::Fill(_)))
        .filter_map(|win| on_desk.iter().find(|(n, _)| *n == win.number))
        .map(|&(_, rect)| rect)
        .collect();
    let free = desk
        .iter()
        .find(|m| !filled.iter().any(|f| overlap(**m, *f)));
    let start = free.or(desk.first()).map_or([0.0; 2], |m| [m[0], m[1]]);
    let unknown: Vec<usize> = (0..on_desk.len())
        .filter(|&i| {
            let (number, rect) = on_desk[i];
            let placement = draft.windows.iter().find(|w| w.number == number);
            unplaced
                && match placement.map(|w| &w.placement) {
                    Some(Placement::Free(Some(_))) => true,
                    // A new window opens in the middle of Window 1's monitor: shown there if it
                    // can be.
                    Some(Placement::Free(None)) => filled.iter().any(|f| overlap(rect, *f)),
                    Some(Placement::Fill(_)) | None => false,
                }
        })
        .collect();
    let [left, top, aw, ah] = area(w, h);
    let scale_of = |[x0, y0, x1, y1]: [f64; 4]| (aw / (x1 - x0)).min(ah / (y1 - y0));
    let sizes: Vec<[f64; 2]> = unknown
        .iter()
        .map(|&i| [on_desk[i].1[2], on_desk[i].1[3]])
        .collect();
    // As many rows as make the map the largest.
    let known = (0..on_desk.len()).filter(|i| !unknown.contains(i));
    let known: Vec<[f64; 4]> = known.map(|i| on_desk[i].1).collect();
    let mut best: Option<(f64, Vec<[f64; 4]>)> = None;
    for rows in 1..=sizes.len() {
        let shelf = shelve(&sizes, rows, start, &filled);
        let k = scale_of(bounds(desk.iter().chain(&known).chain(&shelf).copied()));
        if best.as_ref().is_none_or(|(b, _)| k > b * ROW_GAIN) {
            best = Some((k, shelf));
        }
    }
    for (i, rect) in unknown
        .iter()
        .zip(best.map_or(Vec::new(), |(_, shelf)| shelf))
    {
        on_desk[*i].1 = rect;
    }
    // The monitors, and windows past them (Wayland's, whose place is unknown) still shown.
    let around = bounds(desk.iter().copied().chain(on_desk.iter().map(|(_, d)| *d)));
    let [x0, y0, x1, y1] = around;
    if desk.is_empty() || x1 <= x0 || y1 <= y0 {
        return Default::default();
    }
    let k = scale_of(around);
    let (ox, oy) = (
        left + (aw - (x1 - x0) * k) / 2.0,
        top + (ah - (y1 - y0) * k) / 2.0,
    );
    let to_map = |[x, y, dw, dh]: [f64; 4]| {
        [
            (ox + (x - x0) * k) as f32,
            (oy + (y - y0) * k) as f32,
            (dw * k) as f32,
            (dh * k) as f32,
        ]
    };
    let mapped_monitors = desk.iter().copied().map(to_map).collect();
    let windows: Vec<(u8, [f32; 4])> = on_desk.iter().map(|&(n, d)| (n, to_map(d))).collect();
    // In their own order, emptied ones too: a window a player was just dragged out of stays
    // where it was until Apply closes it, so the player can be dragged back into it.
    let tiles = draft
        .preview(monitors, main)
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
    (mapped_monitors, windows, tiles)
}

/// The largest visible side of a tile after later windows cover it.
pub fn uncovered(mut r: [f32; 4], covers: impl Iterator<Item = [f32; 4]>) -> [f32; 4] {
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

/// The least of `tile` to press.
fn least([_, _, w, h]: Rect) -> [f32; 2] {
    [w.min(MIN_TILE[0]), h.min(MIN_TILE[1])]
}

/// Whether `part` of a tile is too little of it to press.
fn sliver(part: Rect, tile: Rect) -> bool {
    let [w, h] = least(tile);
    part != tile && (part[2] < w || part[3] < h)
}

/// A piece of `tile` to press, over none of the pieces `taken`: around what shows of it (`part`),
/// else in the tile, else the nearest place on the map.
fn piece(tile: Rect, part: Rect, taken: &[Rect], area: Rect) -> Rect {
    let [w, h] = least(tile);
    let inside = |[x, y, rw, rh]: Rect, [px, py]: [f32; 2]| {
        let (right, bottom) = (x.max(x + rw - w), y.max(y + rh - h));
        [px.clamp(x, right), py.clamp(y, bottom), w, h]
    };
    let cells = |[x, y, rw, rh]: Rect| {
        let (across, down) = ((rw / w).ceil() as usize, (rh / h).ceil() as usize);
        (0..down).flat_map(move |r| (0..across).map(move |c| [x + c as f32 * w, y + r as f32 * h]))
    };
    let clear = |r: &Rect| taken.iter().all(|t| uncovered(*r, [*t].into_iter()) == *r);
    let shows = inside(tile, [part[0], part[1]]);
    let from = |[x, y, ..]: Rect| (x - shows[0]).powi(2) + (y - shows[1]).powi(2);
    let nearest = |of: Rect| {
        cells(of)
            .map(|at| inside(of, at))
            .filter(clear)
            .min_by(|a, b| from(*a).total_cmp(&from(*b)))
    };
    Some(shows)
        .filter(clear)
        .or_else(|| nearest(tile))
        .or_else(|| nearest(area))
        .unwrap_or(shows)
}

/// What shows of every player's tile, in `tiles`' order, and whether it shows over the windows
/// that cover it: a tile with no more than a sliver left keeps a piece that can be pressed, and
/// the tiles that piece is over make room for it. No two are over one another. `w` and `h`: the
/// screen, as `project` had it.
pub fn visible(windows: &[(u8, Rect)], tiles: &[(u8, Rect)], w: f32, h: f32) -> Vec<(Rect, bool)> {
    let area = area(w, h).map(|v| v as f32);
    // The part of each no later window covers.
    let parts: Vec<Rect> = tiles
        .iter()
        .map(|&(number, tile)| {
            let at = windows.iter().position(|(n, _)| *n == number);
            let covers = at.map_or(&[][..], |k| &windows[k + 1..]);
            uncovered(tile, covers.iter().map(|(_, r)| *r))
        })
        .collect();
    let mut raised: Vec<Option<Rect>> = vec![None; tiles.len()];
    // A piece raised over another tile can leave a sliver of that one: it is raised too. Each
    // turn raises one, so this ends.
    loop {
        let taken: Vec<Rect> = raised.iter().flatten().copied().collect();
        let shown = |i: usize| uncovered(parts[i], taken.iter().copied());
        let Some(i) =
            (0..tiles.len()).find(|&i| raised[i].is_none() && sliver(shown(i), tiles[i].1))
        else {
            return (0..tiles.len())
                .map(|i| raised[i].map_or_else(|| (shown(i), false), |r| (r, true)))
                .collect();
        };
        raised[i] = Some(piece(tiles[i].1, parts[i], &taken, area));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split_layout::{Spot, Window};

    fn monitor(name: &str, at: [i32; 2], size: [u32; 2]) -> Monitor {
        Monitor {
            name: name.into(),
            at,
            size,
            scale: 1.0,
        }
    }

    /// Wayland: nobody says where Window 1 and Window 3 are. Whichever monitor another window
    /// fills, neither is under it, nor under the other.
    #[test]
    fn a_window_of_unknown_place_never_sits_under_one_that_fills_a_monitor() {
        let monitors = [
            monitor("A", [0, 0], [1920, 1080]),
            monitor("B", [1920, 0], [1920, 1080]),
            monitor("C", [0, 1080], [1920, 1080]),
        ];
        let free = |size| Placement::Free(Some(([0, 0], size)));
        let fill = |name: &str| Placement::Fill(name.into());
        let every = [
            vec!["A"],
            vec!["B"],
            vec!["C"],
            vec!["A", "B"],
            vec!["A", "B", "C"],
        ];
        for names in every {
            let mut draft = Draft::everyone_here(2, free([1280, 720]), 0, 0);
            draft.windows.push(Window {
                number: 3,
                placement: free([1280, 720]),
            });
            // Not open yet: its place is the middle of the first monitor.
            draft.windows.push(Window {
                number: 4,
                placement: Placement::Free(None),
            });
            for (i, name) in names.iter().enumerate() {
                draft.windows.push(Window {
                    number: 5 + i as u8,
                    placement: fill(name),
                });
            }
            let (_, windows, _) = project(&draft, &monitors, "", true, 427.0, 300.0);
            let of = |n: u8| windows.iter().find(|w| w.0 == n).unwrap().1.map(f64::from);
            for n in [1, 3, 4] {
                for (k, name) in names.iter().enumerate() {
                    assert!(
                        !overlap(of(n), of(5 + k as u8)),
                        "{n} under {name}, {names:?}"
                    );
                }
            }
            assert!(!overlap(of(1), of(3)), "{names:?}");
        }
    }

    /// A window the system does place is shown where it is, monitor-filling windows or not.
    #[test]
    fn a_window_whose_place_is_known_stays_there() {
        let monitors = [
            monitor("A", [0, 0], [1920, 1080]),
            monitor("B", [1920, 0], [1920, 1080]),
        ];
        let mut draft =
            Draft::everyone_here(2, Placement::Free(Some(([100, 100], [1280, 720]))), 0, 0);
        let before = project(&draft, &monitors, "A", false, 427.0, 300.0).1[0];
        draft.place_player(1, Spot::Fill("B".into()));
        assert_eq!(
            project(&draft, &monitors, "A", false, 427.0, 300.0).1[0],
            before
        );
    }

    /// Sixteen players, one filling the only monitor and every other in a window of its own whose
    /// place nobody knows, and three monitors besides: each tile is big enough to press.
    #[test]
    fn fifteen_windows_of_unknown_place_stay_big_enough_to_press() {
        for (monitors, w, h) in [(1, 427.0, 301.0), (3, 427.0, 301.0), (1, 320.0, 240.0)] {
            let monitors: Vec<Monitor> = (0..monitors)
                .map(|i| monitor(&format!("M{i}"), [1920 * i, 0], [1920, 1080]))
                .collect();
            let window = Placement::Free(Some(([0, 0], [854, 480])));
            let mut draft = Draft::everyone_here(16, window.clone(), 0, 0);
            draft.windows = (1..=15)
                .map(|number| Window {
                    number,
                    placement: window.clone(),
                })
                .collect();
            draft.windows.push(Window {
                number: 16,
                placement: Placement::Fill("M0".into()),
            });
            draft.players = (1..=16).collect();
            let (_, windows, tiles) = project(&draft, &monitors, "", true, w, h);
            let shown = visible(&windows, &tiles, w, h);
            assert_eq!(shown.len(), 16);
            for (rect, raised) in &shown {
                assert!(
                    !raised && rect[2] >= MIN_TILE[0] && rect[3] >= MIN_TILE[1],
                    "{rect:?}"
                );
            }
            apart(&shown);
        }
    }

    /// A system that says where windows are: a new one is shown in the middle of Window 1's
    /// monitor, a filled monitor next to it or not.
    #[test]
    fn a_new_window_moves_aside_only_where_places_are_unknown() {
        let monitors = [
            monitor("A", [0, 0], [800, 600]),
            monitor("B", [800, 0], [1920, 1080]),
        ];
        let mut draft = Draft::everyone_here(3, Placement::Free(Some(([0, 0], [400, 300]))), 0, 0);
        draft.place_player(1, Spot::Fill("B".into()));
        draft.place_player(2, Spot::New);
        let new = |unplaced| {
            let (screens, windows, _) = project(&draft, &monitors, "A", unplaced, 427.0, 300.0);
            (windows[2].1[0] - screens[0][0]) / screens[0][2] * 800.0
        };
        assert!(new(false) < 800.0, "{}", new(false));
        assert!(new(true) >= 800.0 + 1920.0, "{}", new(true));
    }

    /// No two of these are over one another, and each is on the screen.
    fn apart(shown: &[(Rect, bool)]) {
        for (i, (a, _)) in shown.iter().enumerate() {
            assert!(a[2] > 0.0 && a[3] > 0.0, "{i}: {a:?}");
            for (b, _) in &shown[i + 1..] {
                assert_eq!(uncovered(*a, [*b].into_iter()), *a, "{a:?} and {b:?}");
            }
        }
    }

    #[test]
    fn windows_on_the_very_same_place_leave_every_player_a_piece_of_its_own() {
        let every = [
            [100.0, 100.0, 80.0, 40.0],
            // Too small for a piece each: the others go beside it.
            [100.0, 100.0, 26.0, 16.0],
            [20.0, 34.0, 387.0, 172.0],
        ];
        for at in every {
            for n in [2, 4, 16] {
                let windows: Vec<(u8, Rect)> = (1..=n).map(|i| (i, at)).collect();
                let shown = visible(&windows, &windows, 427.0, 300.0);
                apart(&shown);
                let [w, h] = least(at);
                for (rect, _) in &shown {
                    assert!(rect[2] >= w && rect[3] >= h, "{n} at {at:?}: {rect:?}");
                }
            }
        }
    }

    #[test]
    fn a_covered_tile_keeps_a_piece_to_press_over_the_window_that_covers_it() {
        // The 4 px strip of issue #20928, and a tile with nothing left at all.
        let strip = [21.0, 85.0, 58.0, 76.0];
        let cover = [21.0, 85.0, 113.0, 72.0];
        for (tile, strip) in [(strip, true), ([30.0, 90.0, 60.0, 40.0], false)] {
            let windows = [(1, tile), (2, cover)];
            let shown = visible(&windows, &windows, 427.0, 300.0);
            let ([x, y, w, h], raised) = shown[0];
            assert!(raised && w >= MIN_TILE[0] && h >= MIN_TILE[1], "{shown:?}");
            assert!(x >= tile[0] && x + w <= tile[0] + tile[2] + 0.01);
            assert!(y >= tile[1] && y + h <= tile[1] + tile[3] + 0.01);
            if strip {
                assert!((y + h - 161.0).abs() < 0.01, "around the strip: {shown:?}");
            }
            // The other tile makes room: a press on its middle is its own.
            let (other, raised) = shown[1];
            assert!(!raised);
            let middle = (other[0] + other[2] / 2.0, other[1] + other[3] / 2.0);
            assert!(!crate::ui::hovered(middle, x, y, w, h));
        }
        // Enough left, or nothing over it: only what shows, under the other.
        let half = [(1, [0.0, 0.0, 100.0, 60.0]), (2, [0.0, 0.0, 60.0, 60.0])];
        let expected = [([60.0, 0.0, 40.0, 60.0], false), (half[1].1, false)];
        assert_eq!(visible(&half, &half, 427.0, 300.0), expected);
        // A tile that is small itself is left as it is.
        let small = [(1, [0.0, 0.0, 10.0, 6.0])];
        assert_eq!(visible(&small, &small, 427.0, 300.0), [(small[0].1, false)]);
    }

    #[test]
    fn preview_tiles_share_a_window_and_fallback_when_a_monitor_disappears() {
        let monitors = vec![
            Monitor {
                name: "Desk".into(),
                at: [0, 0],
                size: [2000, 1200],
                scale: 1.0,
            },
            Monitor {
                name: "TV".into(),
                at: [-1920, 0],
                size: [1920, 1080],
                scale: 1.0,
            },
        ];
        let mut draft =
            Draft::everyone_here(2, Placement::Free(Some(([0, 0], [2000, 1200]))), 0, 0);
        let (_, windows, tiles) = project(&draft, &monitors, "Desk", false, 427.0, 300.0);
        assert_eq!(windows.len(), 1);
        let (a, b) = (tiles[0].1, tiles[1].1);
        assert!((a[2] - b[2]).abs() < 0.01);
        assert!((a[0] + a[2] - b[0]).abs() < 0.01);
        draft.place_player(1, Spot::Fill("TV".into()));
        let (screens, windows, tiles) = project(&draft, &monitors, "Desk", false, 427.0, 300.0);
        assert!(screens[1][0] < screens[0][0]);
        assert_eq!(windows.len(), 2);
        assert_eq!((tiles[0].0, tiles[1].0), (1, 2));
        let (_, windows, tiles) = project(&draft, &monitors[..1], "Desk", false, 427.0, 300.0);
        assert_eq!(windows.len(), 1);
        assert_eq!((tiles[0].0, tiles[1].0), (1, 1));
        assert!(
            project(&draft, &[], "Desk", false, 427.0, 300.0)
                .0
                .is_empty()
        );
    }

    #[test]
    fn covered_tiles_only_expose_a_visible_side_for_clicks() {
        let tile = [0.0, 0.0, 100.0, 100.0];
        assert_eq!(
            uncovered(tile, [[0.0, 0.0, 60.0, 100.0]].into_iter()),
            [60.0, 0.0, 40.0, 100.0]
        );
        let hidden = uncovered(tile, [tile].into_iter());
        assert_eq!(hidden[2] * hidden[3], 0.0);
        assert_eq!(
            uncovered(tile, [[110.0, 0.0, 50.0, 50.0]].into_iter()),
            tile
        );
    }
}
