# CtoP - Computer to Pen

A browser UI plus a Rust controller for a Marlin pen plotter. Type text, drop in an image, or rule and fill a lined
notebook page, preview exactly where every stroke will land on a fixed A4 sheet, then press **Write**.

> **Status: simulation-verified only.** Every feature has been exercised against a simulated controller and a fake
> camera (unit tests, end-to-end scripts, headless UI runs). **Nothing has been verified on a real plotter yet**: not
> the calibration with real marks, not pen contact height, not print quality. Treat the first real runs as experiments:
> stay next to the machine, keep a hand on the power switch, and expect to fix things.

## Features

Three modes, all producing single-stroke pen paths on an A4 frame:

- **Text** - single-line (Hershey) fonts and installed outline fonts (.ttf/.otf, drawn as contours); size, spacing,
  alignment, portrait or landscape, automatic wrapping and multiple sheets.
- **Image** - PNG, JPG, WEBP or SVG converted to outline, line-art or hatch strokes, with background cleanup, size and
  position controls and pen-travel ordering.
- **Lined paper** - draws a notebook page (border, margin line, writing lines solid or dashed) and optionally writes
  text on the lines.

Also: fixed-frame calibration saved to disk (the camera is not needed to draw), a live preview of the exact strokes
that will be sent, an optional camera for positioning help, a "Technical" view for diagnostics, redacted diagnostic logs.

## Requirements

- Node.js 20+ and npm (or pnpm), Rust (stable) with the usual Tauri 2 Linux build dependencies
  (webkit2gtk etc., see the Tauri docs). Linux only at the moment (unix socket, `/run/user/...` defaults).
- **A plotter controller process listening on a unix socket.** CtoP never opens the printer's USB/serial port; it
  talks to an external controller ("daemon4", default socket `/run/user/1000/pend.sock`, default folder
  `~/ctop-controller`). That controller is **not part of this repository**. Without it the UI loads but cannot move
  anything. Override with `CTOP_CONTROLLER_SOCK` / `CTOP_CONTROLLER_DIR` or in Setup.
- Optional: a camera hub (HTTP, default `http://127.0.0.1:7792`) for positioning help. CtoP works without it.
- A pencil or pen plotter with a spring-loaded or otherwise safe pen holder, and an A4 sheet fixed to the bed.

## Build and run

```sh
npm install
npx vite build                         # builds the frontend into dist/
cd src-tauri && cargo build --release  # or: npx tauri build --no-bundle
./target/release/ctop --serve          # browser mode
```

Open <http://localhost:7795>. On localhost no token needs to be typed: the service embeds it in the page it serves.
The service binds to 127.0.0.1 only, checks Host/Origin, and requires the token on every API call (stored in the
data dir, mode 0600). Do not expose it to a network; `CTOP_WEB_BIND=0.0.0.0` exists but whoever reaches the page
can move your plotter.

Without `--serve` the binary opens a desktop window; `CTOP_WEB=1` does both.

Environment: `CTOP_WEB_PORT` (7795), `CTOP_WEB_BIND`, `CTOP_DATA` (data dir, default `~/.local/share/dev.ctop.app`),
`CTOP_DIST` (frontend dir for `--serve`), `CTOP_CONTROLLER_SOCK`, `CTOP_CONTROLLER_DIR`.

### Autostart (systemd user unit)

Save as `~/.config/systemd/user/ctop.service` (adjust the path), then
`systemctl --user daemon-reload && systemctl --user enable --now ctop`:

```ini
[Unit]
Description=CtoP pen plotter UI
After=default.target

[Service]
ExecStart=%h/path/to/ctop/src-tauri/target/release/ctop --serve
Environment=CTOP_DIST=%h/path/to/ctop/dist
Restart=on-failure

[Install]
WantedBy=default.target
```

## One-time calibration

Open **Set up your A4 frame** (Calibrate). The result is saved as a named profile and drawing never needs the camera
again. Every step that moves the machine needs an explicit click.

1. **Plotter and homing** - confirm the controller is ready, then *Home X/Y now*.
2. **Where the paper sits** - fix the A4 sheet to the bed. Enter its position (or use *Suggest position* when a camera
   is connected) and check it against the machine's reach.
3. **Pen contact height** - move the pen over the middle of the sheet, lower in 1 / 0.5 / 0.2 / 0.1 mm steps until the
   tip just touches, then record.
4. **Test pattern** - draw the test crosses, measure them on the paper with a ruler and enter the measurements; CtoP
   corrects offset, rotation and skew, or confirm they are already right.
5. **Save and use** - give the profile a name.

Recalibrate only when the frame, the pen, or the machine changes (sheet moved, new pen length, bed re-leveled).

## Using the modes

- **Text** - type, pick a font and size, choose portrait/landscape. Text that does not fit continues on further sheets;
  the preview shows each sheet, and you replace the paper between sheets.
- **Image** - load a file, pick the stroke style (outline, line-art, hatch), clean the background, set size and position.
- **Lined paper** - options: line spacing, margins, border on/off, margin line and its offset, line style
  (solid/dashed/hidden), number of lines (0 = as many as fit), write on every n-th line, optional text on the lines.
  Landscape turns the page a quarter turn clockwise on the sheet (the page's left edge lands on the sheet's top edge).
  Overflowing text continues on more sheets.

**Reach limit.** The sheet can be taller than the plotter's travel. CtoP computes the machine's reach on the sheet
and automatically widens the margins so nothing is drawn where the pen cannot go; the backend refuses any stroke
outside the sheet's safe area or the reach.

## Safety design

- Motion only happens from explicit clicks (Home, Write, test pattern, pen probe). Nothing moves on page load.
- The app can only send a whitelist of controller commands (`ctl.rs`); it cannot flash firmware, heat, or open the serial port.
- Before every job the backend rechecks: plotter ready, calibrated frame, every point inside the sheet and the reach,
  no job already running, then the controller's own job check.
- **Emergency stop**: button in the UI, or **Shift+Esc**. Stop cancels at the next stroke boundary and lifts the pen.
- The camera is optional and never required to draw.
- Software safeguards do not replace supervision or a hardware power cut-off.

## Simulation and tests

```sh
npx vitest run                          # frontend: layout, planning, geometry (117 tests)
cd src-tauri && cargo test --lib        # backend unit tests
python3 -I tools/e2e_sim.py             # end-to-end through the HTTP API
python3 -I tools/e2e_frame.py           # frame calibration end to end
tools/sim_stack.sh                      # fake controller + fake camera + ctop --serve on private ports; tools/sim_stop.sh
```

The simulation scripts need a fake controller, which `tools/fake_controller.py` builds from **your local controller
checkout** (`~/ctop-controller`, or `CTOP_CONTROLLER_DIR`): it imports that project's daemon and its `tests/fake_marlin`.
Those files are not included here, so the e2e scripts run only if you have that controller. The vitest and cargo
unit tests need nothing external.

## Project layout

- `src/` - Svelte 5 + TypeScript UI. `src/lib/plan/` mode planning, `src/lib/write/` page layout and image jobs,
  `src/lib/text/` fonts, `src/lib/frame.ts` frame maths, `src/components/` screens (Calibrate, Setup, Technical, ...).
- `src-tauri/src/` - Rust: `ctl.rs` controller client and whitelist, `frame.rs` frame profiles, `writer/` job execution,
  `web.rs` localhost HTTP service, `paper.rs`/`geom.rs` camera and homography, `fonts.rs`, `diag.rs` redacted logs.
- `tools/` - simulation stack and end-to-end scripts. `packaging/` - desktop entry.

## Known limitations

- Not verified on hardware (see top). Pen contact height is a recorded setting, not a measured plane.
- Linux only; needs an external controller with a specific unix-socket protocol.
- Old serial/simulator Rust modules (`app.rs`, `machine.rs`, ...) remain compiled but unused by the UI.
- Fixed A4 only; one sheet size.

## Contributing

Issues and pull requests are welcome. Run `npx vitest run` and `cargo test --lib` before submitting. Please do not
submit changes that bypass the explicit-click or reach checks.

## License

MIT, see [LICENSE](LICENSE). Copyright (c) 2026 CtoP contributors.
