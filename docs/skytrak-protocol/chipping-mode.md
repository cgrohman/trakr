# Chipping mode: what it actually is

The original SkyTrak has exactly two hardware shot modes: Normal and Putting
(`STSWBoxSetShotModeNormal` / `STSWBoxSetShotModePutting`, confirmed in
`framing-and-crypto.md` and `session.md`). There is no third "chipping" mode
on the device. "Chipping" is entirely a host-side software concept invented
by the vendor's GSPro connector to work around the fact that a lob wedge
swung at chipping speed reads unreliably in Normal mode and needs Putting
mode's more forgiving capture window, while still wanting to be logged and
clamped differently than an actual putt.

Source: `research/gspro-connector/decompiled/` —
`GSProDeviceInterface/GSProDeviceInterface/MainWindow.cs` and
`SkytrakPlugin/SkytrakPlugin/SimpleSkytrakDevice.cs`.

## How the vendor connector decides "chipping"

Two independent triggers, evaluated per GSPro player-info update
(`MainWindow.cs:557-590`, `_proApi_OnResponse`):

1. **By club.** If the simulator reports the selected club as `"LW"` (lob
   wedge), the connector enters chipping. Any other non-putter club is
   Normal. (`MainWindow.cs:458-465`, `HandleClubChange`)
2. **By distance to target**, if the user set `ForceChipDistanceToTarget` to
   a nonzero yardage in `Skytrak.json`. This overrides the club check
   entirely: within that distance (and not on the putting green), chipping;
   beyond it, Normal — regardless of what club is actually selected.
   (`MainWindow.cs:568-585`)

Putter selection (`club == "PT"`) always wins and goes to real Putting mode,
independent of both of the above.

## What "chipping" actually changes

1. **Which hardware mode gets armed.** Controlled by `Skytrak.json`'s
   `LWMode` setting: `"CHIPPING"` (the shipped default) arms the box's real
   **Putting** mode; `"NORMAL"` arms **Normal** mode instead.
   (`SimpleSkytrakDevice.cs:369-376`, `SetChippingMode`)
2. **Horizontal launch angle clamping**, applied to the decoded shot before
   it's sent to the simulator — tighter than real Putting mode's dynamic,
   ball-speed-scaled clamp:
   - Chipping (when `LWHLAProtection` is enabled, the shipped default):
     clamp to **±2°** flat. (`SimpleSkytrakDevice.cs:447-460`)
   - Putting: clamp scales with ball speed, roughly ±0.1–0.5× the raw HLA
     depending on speed bands under ~6 mph. (`SimpleSkytrakDevice.cs:461+`)
   - Normal: no clamp.

## How trakr implements this

`trakr-core::ShotMode` has a third variant, `Chipping`, alongside `Normal`
and `Putting` — it's a first-class mode, not a flag layered on Putting.

- `trakr-skytrak::SkytrakDriver` maps `Chipping` to the box's Putting or
  Normal hardware mode via a constructor option (`chip_via_putting`,
  default `true`, matching the vendor default). The HLA clamp itself will
  apply once shot decoding is implemented (`shot-data.md`); the mode
  plumbing is in place now so that work only has to add the clamp value.
- `trakr-openconnect::Config` gets `force_chip_distance_yd: Option<f32>`
  (`None` = off, matching `ForceChipDistanceToTarget: 0`). When set, and a
  club/distance-to-target update arrives from the simulator, the mode is
  chosen the same way the vendor connector chooses it: putter → Putting;
  else if a distance threshold is configured, distance decides Chipping vs
  Normal; else club `"LW"` → Chipping, everything else → Normal.
- Configured at daemon startup: `trakr serve --force-chip-distance-yd 20`,
  or `TRAKR_FORCE_CHIP_DISTANCE_YD=20` for the Tauri app. Chipping can also
  always be set manually (`trakr mode chipping`, `POST /v1/session/mode
  {"mode":"chipping"}`, or the UI's mode selector) independent of auto rules.

## Open questions

- The vendor's `Club != "Putter"` string check likely never matches GSPro's
  actual club code (`"PT"`); trakr uses `"PT"` directly, which is what
  `putting_from_club` already relied on elsewhere in this codebase.
- Exact ball-speed bands for Putting mode's dynamic HLA clamp are
  transcribed in `SimpleSkytrakDevice.cs` but not yet ported anywhere in
  trakr, since no shot decoding exists yet to apply either clamp to.
