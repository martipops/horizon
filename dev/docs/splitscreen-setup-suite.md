# Splitscreen setup suite — plan

2026-10-27 · branch `splitscreen` (github.com/martipops/horizon)

A Splitscreen Setup screen will show your monitors to scale with every window and player on them, let
you move players and windows as a draft, and change nothing until you press **Apply**. It is built in new
files, so Horizon's own code gains about a dozen lines.

## Where splitscreen stands

Up to 8 players in one process: drop-in with controllers (press A, pick or make a profile, LB/RB to
choose where your view goes), numbered windows players can share, monitor windows, shared sound from the
nearest player, questions as in-view panels with the pad's own buttons, a corner badge per view, and a
live test suite (`tools/splitscreen_e2e.py`). Arranging players afterwards is still awkward: "Play on"
buttons on a settings page, no picture of where windows go, and "Monitor 2" versus "Window 2" confuses.

## The model: everything is a window

A window has a **placement**:

- **Free**: a normal window you can drag, resize and put anywhere.
- **Fills Monitor N**: borderless over that monitor (what "Monitor N" means today).

Window 1 is the game's own window. Players belong to windows. The whole setup is: which windows exist,
where each is, who is in each. "Monitor N" stops being a player choice and becomes "a window filling
Monitor N".

## The screen

Horizon Settings → **Splitscreen…**, and from each player's pause menu.

```
                         Splitscreen setup
 ┌──────────── Monitor 1 (3024x1964) ───────────┐ ┌──── Monitor 2 (3840x2160) ────┐
 │  ┌─ Window 1 ───────────────┐                │ │ ┌─ Window 2 (fills it) ─────┐ │
 │  │ [P1 Martin] [P2 Jo]      │                │ │ │ [P3 Alex]                 │ │
 │  └──────────────────────────┘                │ │ │ [P4 Sam]                  │ │
 │                                              │ │ └───────────────────────────┘ │
 └──────────────────────────────────────────────┘ └───────────────────────────────┘
   Presets:  [Everyone here]  [Desk + TV]  [Everyone separate]      [Identify screens]
   + Add window      Two on a screen: Auto     Three on a screen: One wide on top
   ⚠ Window 3 has nobody in it: it won't open            [Revert]   [Apply]
```

- **Map to scale**, monitors at their real positions; windows drawn where they will be; player tiles split
  exactly as their views will be (the game's own `place()`, so the preview cannot disagree with reality).
- **Move players**: mouse drag-and-drop; controller D-pad to select, A to pick up, D-pad or bumpers to
  move, A to drop.
- **Windows**: `+ Add window`, drag it onto a monitor and choose *Fill monitor* or *Free*; remove it with a
  button on the window (its players go back to Window 1).
- **Presets**: *Everyone here* (all in Window 1), *Desk + TV* (P1 in Window 1, everyone else filling the
  other monitor), *Everyone separate* (a window each, spread over the monitors).
- **Identify screens**: a big number on every real monitor, with the window names, for 3 s.
- **Footer**: Splitscreen on/off, Two on a screen, Three on a screen (from today's page).

## Draft, Apply, warnings

- Nothing moves until **Apply**. Meanwhile faint outlines on the real screens show where the draft's
  windows would go. **Revert** drops the draft; closing the screen without Apply is Revert.
- Warnings before Apply, in plain words:
  - an empty window: it won't open;
  - a monitor that is unplugged: its players play in Window 1;
  - more than 8 players.

## Who can change what

- **P1** moves anyone and edits windows.
- **Other players** open the same screen but move only their own tile (others are locked), and may add a
  window for themselves.

## How it is built

| Piece | File | Upstream impact |
|---|---|---|
| Draft model: monitors, windows + placements, players → windows, presets, validation, preview geometry via `place()` | new `crates/horizon-client/src/split_layout.rs` (pure data, unit-tested) | none |
| The screen: draw, mouse, controller, draft state | new `crates/horizon-client/src/split_ui.rs`, shaped like `graphics_ui.rs` (`draw` / `click` / `release` / `scroll`) | `options_ui.rs`: one field and ~6 delegation lines, as `pub graphics: GraphicsUi` has |
| Apply / Revert / outlines / Identify; live state ⇄ draft | `crates/horizon-client/src/game/split.rs` (+ `game/split/layout.rs`) | none |

Reuse: `game/split/layout.rs::place` (tiling); `Split::choices` / `sync_surfaces` / `relayout` (applying);
`split::Hud` (badge and panels, for the on-screen numbers); `Button::label` / `color` in `gamepad.rs`
(button glyphs).

## Steps, each tested before the next

1. **Draft model** + unit tests: every preset and warning, unplugged monitors, 8 players, live ⇄ draft
   round trip.
2. **`split.rs` switches to the window model** (Monitor N → a window filling it); the live suite must pass
   unchanged.
3. **Static screen**: map from a draft, click to select, Apply / Revert; a live-suite scenario opens it,
   moves a player, applies, checks the views.
4. **Full interaction**: drag-and-drop, controller navigation, add / remove window, presets, Identify,
   outlines. The biggest step.
5. **Pause-menu entry and permissions**; a scenario checks P2 cannot move P3.
6. **Foolproof pass**: wording, a first-open hint, the warnings; try it with someone who has never seen it.

Verification each step: `cargo test -p horizon-client --lib split_layout`,
`cargo test -p horizon-client --profile fast --bin horizon-client split::`,
`python3 tools/splitscreen_e2e.py` (exit code checked), clippy with warnings denied, screenshots at 1, 2
and 8 players, and a try with real controllers.

## Status (2026-10-27)

Built on `splitscreen`: steps 1–5 (commits 6c549122b, 4c68f5c51, bd1d7c034, abf63501a). Different from
the plan:

- **Entry**: Horizon Settings → Splitscreen → *Setup…*, or hold Select 2 s then **Y** on a controller. No
  pause-menu button: the pause menu is vanilla's grid and its free row holds the icons.
- **Windows**: no separate *+ Add window* / remove; *New window* in a player's moves adds one, and a window
  nobody plays in closes on Apply (with a warning first).
- **Controller**: the tiles and buttons are ordinary widgets, so the controller's cursor moves between
  them; no D-pad pick-up/drop yet.
- **Not built**: outlines of the draft on the real screens; a "more than 8 players" warning (the screen
  can't add players).
- Tried with a TV over AirPlay: monitor windows are placed in each monitor's points on macOS. The TV's menu
  bar still covers the top ~30 px of a window filling it.

## Decisions (open to veto)

- "Fills Monitor N" is a borderless window covering the monitor, not macOS native fullscreen (that hid the
  other windows).
- The draft lives only while the screen is open; closing without Apply reverts.
- Free windows remember position and size for the session only.
- Screens are not remembered across sessions (removed 2026-10-27: "as last time" confused players).
