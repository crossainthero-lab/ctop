# CtoP handover

CtoP (Computer to Pencil): Tauri 2 + Svelte 5/TS + Rust desktop app that drives a Marlin printer holding a pencil.
**Status: developed and tested against the built-in simulator only. The real printer has never been connected.**

## Implemented and verified
- Rust backend (`src-tauri/src/`): calibration model (all fields unset by default), safety allow-list + independent G-code verifier, stroke planner, G-code generator, Marlin parser, exclusive serial access with holder detection (`serial.rs`), Marlin simulator with fault injection (`sim.rs`), machine actor with ok-window, M400 checkpoints, pause/cancel/quick-stop/emergency (`machine.rs`), job store (`app.rs`), Tauri commands.
- Frontend (`src/`): Workspace, Manual Control, Calibration, Jobs, Settings views; text (Hershey + outline fonts), image conversion (5 modes), SVG import, undo/redo, path preview.
- Hardware mode is locked behind `hardwareUnlocked` (default false). No auto-home, no heaters/extruder, no relative mode.
- Seen working in a headless screenshot run (private Xvfb): app boots, all views except Jobs render, simulator connects (only M115/M211/M114 sent), arming works, Text tool creates a Hershey text object.

## Test results (last run, before final UI-only fixes)
- `cd src-tauri && cargo test --lib`: 75 passed.
- `npx vitest run`: 52 passed.
- `npx svelte-check`: 0 errors. `npx vite build`: OK.
- Not re-run after the last small changes (render.ts -> render.svelte.ts rename, main.ts error overlay, App.svelte flex layout). Re-run first.

## Not done / untested
- No full GUI draw run (verify position -> Draw -> simulator traces it); only backend sim tests cover it.
- Jobs view, image/SVG import screens, drag-drop, clipboard paste not visually checked.
- No `.deb` built. No line-number/checksum resend protocol. No resume of interrupted jobs.
- Sim has no acceleration model. SVG fills drawn as outlines only.
- Landscape project vs calibrated-sheet orientation mismatch only caught by the stroke-level drawable check.
- Calibration values still needed from the user's physical calibration: pencil offset (tip - nozzle), pen-down/pen-up/safe Z, sheet origin (tip X/Y), axis orientation, safe envelope. Never invent them.

## Build and launch
```
npm/pnpm install            # pnpm: use npx / ./node_modules/.bin, `pnpm run` errors (ERR_PNPM_IGNORED_BUILDS)
npx vite build
cd src-tauri && cargo test --lib
# Low-impact release build:
CARGO_BUILD_JOBS=2 CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_STRIP=false nice -n 19 ./node_modules/.bin/tauri build --no-bundle
./src-tauri/target/release/ctop       # launch manually
```
Frontend is embedded in the binary: rebuild with `tauri build` after any `src/` change.

## Known issues / gotchas
- Files using Svelte runes must be named `.svelte.ts`, else a blank window (symptom: `$state is not defined`).
- M410/M112 are only immediate if firmware reports `Cap:EMERGENCY_PARSER:1`; the sim defaults to 0.
- Z homing is hazardous with a pencil fitted.
- Aurora (`ssh aurora`) has no cargo/webkit/xvfb; builds are done locally, niced.

## Rules from the user (still apply)
- Never open `/dev/ttyUSB0` or touch the printer until the user confirms the other session released it.
- Never show anything on the user's real desktop: no `tauri dev`, no launching the app. GUI tests only on a private Xvfb (`:97`, `DISPLAY` unset from Wayland, `GDK_BACKEND=x11`), clicks via XTest on that display only. Heavy work niced; no pushing to GitHub.

## Next steps
1. Re-run cargo tests, vitest, svelte-check; rebuild release binary (niced).
2. Headless GUI pass: connect sim -> arm -> verify position -> Draw -> watch sim; Jobs view; image/SVG import; fix issues.
3. Remove leftover `void` placeholders in the UI.
4. Optionally build the `.deb` (niced, never launched).
5. When the user confirms the printer is released: guided hardware bring-up, calibration first, unlock hardware mode only with their say-so.
