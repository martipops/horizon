//! Project a draft arrangement onto the Setup screen, without opening a window.
use crate::split_layout::{Draft, Monitor, Placement};

/// Map area: below the title and above the move row and footer.
pub const TOP: f32 = 30.0;
pub const BOTTOM: f32 = 80.0;
/// Between windows whose place nobody knows, side by side on the map (desktop pixels).
const SHELF_GAP: f64 = 16.0;
/// The least of a player's tile left to see and press, where the tile is as big itself.
const MIN_TILE: [f32; 2] = [24.0, 14.0];
pub type Rect = [f32; 4];
pub type Map = (Vec<Rect>, Vec<(u8, Rect)>, Vec<(u8, Rect)>);

fn overlap([ax, ay, aw, ah]: [f64; 4], [bx, by, bw, bh]: [f64; 4]) -> bool {
    ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
}

/// The next place on the shelf for a window of `size`: from `at` rightwards, past every monitor a
/// window fills.
fn on_shelf(at: [f64; 2], [w, h]: [f64; 2], filled: &[[f64; 4]]) -> [f64; 4] {
    let mut rect = [at[0], at[1], w, h];
    // Each step passes a filled monitor for good, so this ends.
    while let Some(f) = filled.iter().find(|f| overlap(rect, **f)) {
        rect[0] = f[0] + f[2] + SHELF_GAP;
    }
    rect
}

/// Monitor, window and player rectangles in UI coordinates, in their drawing order. `unplaced`:
/// the free windows whose place the system doesn't say (Wayland's), and where they aren't known
/// they go side by side, from the first monitor no window fills.
pub fn project(
    draft: &Draft,
    monitors: &[Monitor],
    main: &str,
    unplaced: &[u8],
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
    let mut shelf = free.or(desk.first()).map_or([0.0; 2], |m| [m[0], m[1]]);
    for (number, rect) in &mut on_desk {
        let Some(win) = draft.windows.iter().find(|w| w.number == *number) else {
            continue;
        };
        let unknown = match win.placement {
            Placement::Free(Some(_)) => unplaced.contains(&win.number),
            // A new window opens in the middle of Window 1's monitor: shown there if it can be.
            Placement::Free(None) => filled.iter().any(|f| overlap(*rect, *f)),
            Placement::Fill(_) => false,
        };
        if unknown {
            *rect = on_shelf(shelf, [rect[2], rect[3]], &filled);
            shelf[0] = rect[0] + rect[2] + SHELF_GAP;
        }
    }
    // The monitors, and windows past them (Wayland's, whose place is unknown) still shown.
    let all = || desk.iter().chain(on_desk.iter().map(|(_, d)| d));
    let x0 = all().map(|d| d[0]).fold(f64::MAX, f64::min);
    let y0 = all().map(|d| d[1]).fold(f64::MAX, f64::min);
    let x1 = all().map(|d| d[0] + d[2]).fold(f64::MIN, f64::max);
    let y1 = all().map(|d| d[1] + d[3]).fold(f64::MIN, f64::max);
    if desk.is_empty() || x1 <= x0 || y1 <= y0 {
        return Default::default();
    }
    let (aw, ah) = (
        f64::from(w) - 40.0,
        // Room under the monitors for their names, and always for the move row: the map
        // doesn't jump when a player is picked (a drop back to where its tile was misses).
        (f64::from(h - TOP - BOTTOM) - 18.0).max(10.0),
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

/// What shows of every player's tile, in `tiles`' order, and whether it shows over the windows
/// that cover it: a tile with no more than a sliver left keeps a piece that can be pressed, and
/// the tiles that piece is over make room for it.
pub fn visible(windows: &[(u8, Rect)], tiles: &[(u8, Rect)]) -> Vec<(Rect, bool)> {
    let least = |[_, _, w, h]: Rect| [w.min(MIN_TILE[0]), h.min(MIN_TILE[1])];
    let mut out: Vec<(Rect, bool)> = Vec::with_capacity(tiles.len());
    let mut raised: Vec<Rect> = Vec::new();
    for &(number, tile) in tiles {
        // The part no later window covers.
        let at = windows.iter().position(|(n, _)| *n == number);
        let covers = at.map_or(&[][..], |k| &windows[k + 1..]);
        let part = uncovered(tile, covers.iter().map(|(_, r)| *r));
        let [least_w, least_h] = least(tile);
        if part == tile || (part[2] >= least_w && part[3] >= least_h) {
            out.push((part, false));
            continue;
        }
        let [x, y, w, h] = tile;
        let size = [part[2].max(least_w), part[3].max(least_h)];
        // Around the sliver, or in a corner of the tile where none is raised yet.
        let (left, right) = (x, x + w - size[0]);
        let (top, bottom) = (y, y + h - size[1]);
        let places = [
            [
                part[0].clamp(left, right.max(left)),
                part[1].clamp(top, bottom.max(top)),
            ],
            [left, top],
            [right, top],
            [left, bottom],
            [right, bottom],
        ];
        let piece = |[px, py]: [f32; 2]| [px, py, size[0], size[1]];
        let clear = |r: &Rect| raised.iter().all(|o| uncovered(*r, [*o].into_iter()) == *r);
        let rect = places
            .into_iter()
            .map(piece)
            .find(clear)
            .unwrap_or(piece(places[0]));
        raised.push(rect);
        out.push((rect, true));
    }
    for (rect, _) in out.iter_mut().filter(|(_, raised)| !raised) {
        let part = uncovered(*rect, raised.iter().copied());
        let [least_w, least_h] = least(*rect);
        if part[2] >= least_w && part[3] >= least_h {
            *rect = part;
        }
    }
    out
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
            let (_, windows, _) = project(&draft, &monitors, "", &[1, 3], 427.0, 300.0);
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
        let before = project(&draft, &monitors, "A", &[], 427.0, 300.0).1[0];
        draft.place_player(1, Spot::Fill("B".into()));
        assert_eq!(
            project(&draft, &monitors, "A", &[], 427.0, 300.0).1[0],
            before
        );
    }

    #[test]
    fn a_covered_tile_keeps_a_piece_to_press_over_the_window_that_covers_it() {
        // The 4 px strip of issue #20928, and a tile with nothing left at all.
        let strip = [21.0, 85.0, 58.0, 76.0];
        let cover = [21.0, 85.0, 113.0, 72.0];
        for (tile, sliver) in [(strip, true), ([30.0, 90.0, 60.0, 40.0], false)] {
            let windows = [(1, tile), (2, cover)];
            let shown = visible(&windows, &windows);
            let ([x, y, w, h], raised) = shown[0];
            assert!(raised && w >= MIN_TILE[0] && h >= MIN_TILE[1], "{shown:?}");
            assert!(x >= tile[0] && x + w <= tile[0] + tile[2] + 0.01);
            assert!(y >= tile[1] && y + h <= tile[1] + tile[3] + 0.01);
            if sliver {
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
        assert_eq!(visible(&half, &half), expected);
        // A tile that is small itself is left as it is.
        let small = [(1, [0.0, 0.0, 10.0, 6.0])];
        assert_eq!(visible(&small, &small), [(small[0].1, false)]);
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
        let (_, windows, tiles) = project(&draft, &monitors, "Desk", &[], 427.0, 300.0);
        assert_eq!(windows.len(), 1);
        let (a, b) = (tiles[0].1, tiles[1].1);
        assert!((a[2] - b[2]).abs() < 0.01);
        assert!((a[0] + a[2] - b[0]).abs() < 0.01);
        draft.place_player(1, Spot::Fill("TV".into()));
        let (screens, windows, tiles) = project(&draft, &monitors, "Desk", &[], 427.0, 300.0);
        assert!(screens[1][0] < screens[0][0]);
        assert_eq!(windows.len(), 2);
        assert_eq!((tiles[0].0, tiles[1].0), (1, 2));
        let (_, windows, tiles) = project(&draft, &monitors[..1], "Desk", &[], 427.0, 300.0);
        assert_eq!(windows.len(), 1);
        assert_eq!((tiles[0].0, tiles[1].0), (1, 1));
        assert!(project(&draft, &[], "Desk", &[], 427.0, 300.0).0.is_empty());
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
