# SkyTrak shot-data path: what the box sends and how the host turns it into a shot

Source: Ghidra decompile of the vendor's Windows SDK `PlatformLib.dll` (x64,
Rapsodo "RIPE" SDK), files under
`research/ghidra/out/PlatformLib.dll/` (`fn/<name>.c`, `functions.txt`,
`strings_xref.txt`). Companion documents: `framing-and-crypto.md` (byte-stream
framing, CRC, AES) and `discovery.md`. This document does **not** repeat the
framing layer except where the shot path needs it.

Confidence tags used throughout: **confirmed** (read directly in the decompile,
usually with the constant or offset quoted), **likely** (single consistent
interpretation of the code, not cross-checked against a capture), **guess**
(plausible, needs a real capture to settle).

---

## 0. Answer to the key question

**The box sends camera images, not finished numbers.** Everything the SDK
reports — ball speed, launch angle, horizontal angle, spin, spin axis,
confidences, ball position — is computed on the host from 2–3 grayscale images
of the ball in flight plus a per-image microsecond timestamp. (**confirmed**)

Evidence chain:

| Step | Function | What it shows |
|---|---|---|
| Wire → packets | `FUN_18000c180 @ 18000c180` | `0xDDDDDDDD` image packets land at `parser+0x54 + n*0x6001A`; `0xCCCCCCCC` shot header at `parser+0x2C` |
| Packet → thread | `FUN_1800165d0 @ 1800165d0` case 7 (`PSR: IMG_PACKET_END`) | posts message 0 to per-image queue `G+0x3F0/0x428/0x460` for image 0/1/2 |
| Image → ball circle | `FUN_18001d840/8f0/9b0` → `FUN_180009530 @ 180009530` | decodes pixels, thresholds, edge-detects, circular Hough transform → radius + centre |
| Circle → 3-D position | `FUN_18000b420 @ 18000b420` | pinhole model: `Z = D·376/(2·tan(26.7°)·r)` etc. |
| Positions → speed/angles | `FUN_18000b620 @ 18000b620` | `v = ΔX/Δt` with `Δt = (ts_j − ts_i)/1e6`, `LA = atan(vy/vx)`, `HA = atan(vz/vx)` |
| Speed → event | `FUN_18001cd40 @ 18001cd40` ("IPE:SUMT") | fills `RIPESpeedParams`, posts `0x20 = RIPE_EVT_DATA_SPEEDREADY` |
| Images → spin | `FUN_180007290 @ 180007290` ×3 threads → `FUN_180007140` | rotation search on the ball texture, posts `0x21 = RIPE_EVT_DATA_SPINREADY` |
| Getter | `RIPEGetSpeedReadyData @ 180014630` | copies the 7-word result the SUMT thread wrote |

The `"6 axis calib data"` / `"1 axis calib data"` strings refer to the box's
accelerometer calibration, which the host uses to correct launch angle for
device tilt (section 6.4). `RESP_ERR_IMGTRANSFER_TIMEOUT` and
`RESP_ERR_FPGATRIGGER_TIMEOUT` are box-side error codes reported in the status
packet.

The statically linked Intel IPP library (`"IPP Init Success"`, the ~2.5 MB of
code at `0x18001F000–0x1802B0000` with duplicated CPU-dispatch variants) is
initialised by `FUN_180004650 @ 180004650` but **none of the image primitives
call into it** — every primitive in the engine (`0x180001000–0x18000C800`,
42 221 bytes of code total) is hand-written C. (**confirmed** by callee scan.)

---

## 1. Sequence of a shot on the wire (box → host)

All values little-endian. Packets are `[u32 magic][u32 total_len][payload]`,
`total_len` includes the 8-byte header (see `framing-and-crypto.md` §1.2).

```
box ──► 0xCCCCCCCC  shot header      (AES-128-ECB payload)     PSR 0  "HEADER_PACKET_START"
box ──► 0xDDDDDDDD  image 0          (AES-128-ECB payload)     PSR 6/7
box ──► 0xDDDDDDDD  image 1          (AES-128-ECB payload)     PSR 6/7
box ──► 0xDDDDDDDD  image 2          (AES-128-ECB payload)     PSR 6/7
box ──► 0xEEEEEEEE  trigger packet   (clear, only if header word 2 != 0)  PSR 4/5
        ── shot complete ──                                    PSR 1  "HEADER_PACKET_END" → event 0x1F TRIGGERDETECTED
host    ~1.1 s after start:  event 0x20 SPEEDREADY
host    ~2.45 s after start: event 0x21 SPINREADY
```

(**confirmed**: `FUN_18000c180` state machine; `FUN_1800165d0` cases 0/1/4–7;
`FUN_18001cd40` for the two events.)

Details of the reassembler that matter for the shot path (`FUN_18000c180`):

* `0xCCCCCCCC` sets the "in-shot" flag (`DAT_18046b218 = 1`), resets the image
  counter `parser[10] = 0` and invokes PSR case 0. Its payload is written to
  `parser+0x2C` (magic at `+0x2C`, length at `+0x30`, payload from `+0x34`).
* Each `0xDDDDDDDD` is written to `parser + 0x54 + n*0x6001A` where `n` is the
  running image counter (arrival order = image index). After the packet
  completes, PSR case 7 is invoked with `n`. When
  `n+1 == parser[0xD] + parser[0xE]` (header words 0 and 1) **and**
  `parser[0xF] == 0` (header word 2) the shot is complete after this image.
* `0xEEEEEEEE` is only honoured while in-shot; it is written to
  `parser+0x120142` and marks the shot complete after it finishes.
* Shot completion invokes PSR case 1, which writes the `.rif` file (section 8)
  and posts internal message `0x1F` = `RIPE_EVT_DATA_TRIGGERDETECTED` to the
  user.
* Only `0xCCCCCCCC` and `0xDDDDDDDD` payloads are AES-decrypted (key by
  firmware version — see `framing-and-crypto.md` §3). Magic and length are
  always in the clear.
* There is no per-packet CRC check on box→host data and no chunk/sequence
  numbering: each image is one logical packet (up to 393 KB) that the byte
  stream parser reassembles from arbitrary USB (16 KB bulk reads on EP 0x81,
  `FUN_180018890`) or TCP segments. (**confirmed**)

---

## 2. Shot header packet `0xCCCCCCCC`

Stored at `parser+0x2C`. Payload is AES-encrypted on the wire.

| Offset from magic | Offset in parser | Type | Meaning | Confidence |
|---|---|---|---|---|
| 0x00 | 0x2C | u32 | `0xCCCCCCCC` | confirmed |
| 0x04 | 0x30 | u32 | total length (printed in `HEADER_PACKET_END (%d bytes)`) | confirmed |
| 0x08 | 0x34 | u32 | image count A (`parser[0xD]`) | confirmed (used as count) |
| 0x0C | 0x38 | u32 | image count B (`parser[0xE]`); `A+B` = images expected by SUMT | confirmed |
| 0x10 | 0x3C | u32 | non-zero ⇒ a `0xEEEEEEEE` trigger packet follows the images (`parser[0xF]`) | confirmed |
| 0x14 | 0x40 | u32 | hardware/firmware variant code; compared with 58 and 68 to pick thresholds and which image is the "reference" image | likely (meaning), confirmed (use) |
| 0x18… | 0x44… | ? | not read by the SDK | — |

Only image indices 0, 1, 2 have processing queues, and `FUN_18001cd40`
waits for `A+B` stage-1 completions, so in practice `A+B == 3` (three images
per shot). Which images are "A" vs "B" is not used anywhere except the sum.
(**likely**: 3 images/shot.)

---

## 3. Image packet `0xDDDDDDDD`

Stored at `parser + 0x54 + n*0x6001A`. Slot size 0x6001A = 8-byte frame header
+ 18-byte image header + 0x60000 (393 216) bytes of pixel space. Payload is
AES-encrypted on the wire.

### 3.1 Image header (decoded by `FUN_180002860 @ 180002860`, fields also read in `FUN_180009530`, `FUN_18000b420`, `FUN_18000b620`)

| Offset from magic | Type | Meaning | Confidence |
|---|---|---|---|
| 0x00 | u32 | `0xDDDDDDDD` | confirmed |
| 0x04 | u32 | total length incl. header | confirmed |
| 0x08 | u16 | **exposure timestamp in microseconds** (wraps at 65 535). Velocity uses `(ts_j − ts_i)/1e6` seconds; two images with equal timestamps are treated as a simultaneous pair | confirmed |
| 0x0A | u16 | **view/camera flag** 0 or 1. Selects the sign of the lateral offset in `FUN_18000b420`, whether the Y pixel offset `E+0x920` is applied, and which image is used for ball-position and speed logic | confirmed (use), likely (meaning: two optical views) |
| 0x0C | u16 | 0 = plain decode; non-zero = decode then double the width and run `FUN_180004ae0` (sub-sampled / binned frame) | likely |
| 0x0E | u16 | bytes per pixel: 1 (8-bit, value<<2 to 10-bit scale) or 2 (16-bit LE) | confirmed |
| 0x10 | u16 | not read by the decompiled code | — |
| 0x12 | u16 | ROI offset along the A axis (added to detected `cx`) | confirmed |
| 0x14 | u16 | A dimension (`rows` in the source buffer; becomes host image **width**) | confirmed |
| 0x16 | u16 | ROI offset along the B axis (added to detected `cy`) | confirmed |
| 0x18 | u16 | B dimension (`cols` in the source buffer; becomes host image **height**) | confirmed |
| 0x1A | u8/u16 × A×B | pixels, row-major with row length B: `src[a*B + b]` | confirmed |

The decoder **transposes**: host pixel `(x=a, y=b)` = `src[a*B + b]`, stored as
`float`. 8-bit sources are scaled `<<2` so all later thresholds are on a
0…1020 scale (`1000.0` = near-saturated, `200.0`/`100.5`…`650` for ball
detection).

### 3.2 Geometry constants → sensor size

`FUN_18000b420` uses image centre `x = 240 (0xF0)`, `y = 376 (0x178)` and the
focal-length term `376/tan(26.7°)`; `FUN_180009530` special-cases
`height > 500`. Together with the 18-byte header and `0x60000` pixel budget
this indicates a **480 × 752 host image (752 × 480 sensor, 8-bit)**, i.e. a
WVGA global-shutter sensor of the MT9V034 class, giving `f ≈ 747.6 px`.
(**likely**; the buffer would also fit 512 × 768 exactly, so verify against a
capture.) Every working buffer is capped at `0x40000` pixels (the code logs an
assertion above that), so the analysis runs on cropped ROIs, not the full frame.

### 3.3 What is *not* in the image packet

No exposure time, gain, strobe timing or per-image CRC is read by the host.
Time between images comes solely from the u16 microsecond timestamps.

---

## 4. Trigger packet `0xEEEEEEEE`

Stored at `parser+0x120142`; only accepted while in-shot. **The host never
reads its payload** — the only reference is the length print
`PSR:TRIGGER_PACKET_END (%d bytes)` from `parser+0x120146`. Its role is purely
to terminate the shot sequence when header word 2 is non-zero. Content:
unknown (**confirmed** unused; content **guess**). Not encrypted.

---

## 5. Box status packet `0xBBBBBBBB` fields that affect shot processing

The status packet (0xA0 bytes, clear) is copied by PSR case 3 into the engine
at `E+0xE0C` (magic included). Fields used by the shot path (offsets from the
packet magic):

| Offset | Engine addr | Type | Use | Confidence |
|---|---|---|---|---|
| 0x08, 0x0C, 0x10 | `E+0xE14/E18/E1C` | i32×3 | accelerometer X/Y/Z, tilt correction of launch angle (`FUN_1800140e0`) | confirmed |
| 0x18 | `E+0xE24` | i32 | reset to 0 by defaults | — |
| 0x1C | `E+0xE28` | f32/i32 | default 100.0 | — |
| 0x38 | `E+0xE44` | i32 | **player hand**: 0 = right, non-zero = left. Flips sign of lateral velocity/horizontal angle, swaps which images are reference | confirmed (use), likely (RH/LH mapping) |
| 0x74 | `E+0xE80` | i32 | **shot mode**: 2 disables flight data and gates spin (`RIPEGetSpinReadyData` returns 4 when mode==2 and launch angle < 10°). Consistent with `RIPE_CMD_SHOT_MODE_PUTTING` | likely |

Other status fields are documented in `framing-and-crypto.md` §1.3.

---

## 6. Host processing pipeline

Thread/queue map (`G` = global SDK context `PTR_DAT_180419908`, `E` = image
engine at `*(G+0x40)`, per-image record `R_i = E + 8 + i*0x158`):

| Queue | Thread fn | Message | Work |
|---|---|---|---|
| `G+0x3F0/0x428/0x460` | `FUN_18001d840/8f0/9b0` | 0 | stage 1 `FUN_180009530(E, i, parser+0x54+i*0x6001A, {1.0,1.0,35.0,80.0})` then post 1 to SUMT |
| `G+0x3B8` | `FUN_18001cd40` (SUMT) | 1 / 5 / 3 | orchestrates; produces speed and spin results |
| `G+0x498/0x4D0/0x508` | `FUN_18001da70/dae0/db50` | 4 | stage 2 `FUN_180009c40(E, i)` radius refinement, post 5 |
| `G+0x540/0x578/0x5B0` | `FUN_18001dbc0/dc40/dcc0` | 2 | spin `FUN_180007290(E, k)` for axis band k, post 3 |

### 6.1 Per-image record layout `R_i` (stride 0x158) — confirmed offsets

| Offset | Type | Content |
|---|---|---|
| 0x000 | ptr | image packet (magic at +0) |
| 0x008…0x128 | 18 × {ptr, u16 w @+8, u16 h @+0xA} | float working images (source, threshold, edge, Hough accumulator, ROI crops, unwrapped texture …) |
| 0x130 | f32 | ball radius **r** (px) |
| 0x134 | f32 | per-image horizontal angle (deg) |
| 0x138 | f32 | per-image vertical angle (deg) |
| 0x13C | f32 | circle score (Hough votes / r) |
| 0x140 / 0x142 | u16 | centre `cx`, `cy` (ROI-relative) |
| 0x144 / 0x146 | u16 | absolute centre = ROI-relative + header `+0x12` / `+0x16` |
| 0x148 / 0x14C / 0x150 | f32 | ball position **X, Y, Z** (metres; X along target line, Y up-in-image, Z depth from camera) |
| 0x154 | u16 | valid flag |
| 0x156 | u16 | ball-touches-edge flag (`FUN_180006e00`) |

### 6.2 Stage 1 — ball detection (`FUN_180009530`, confirmed algorithm)

1. Decode packet → float image (`FUN_180002860`).
2. If `h > 500`: threshold at 1000.0 (`FUN_180002b20`), count (`FUN_180002aa0`);
   if > 5000 saturated pixels, crop to a 300-row band (`FUN_180003610`,
   `FUN_180003320`) — bright-background handling.
3. 3×3 smoothing filter with kernel `DAT_1803FC210` (`FUN_180005890` →
   `FUN_180005760`; pads by 2 and re-crops).
4. Threshold at 200.0 → binary; edge map via two directional 3×3 kernels
   `DAT_1803FBE48` / `DAT_1803FBDB8` combined by `FUN_1800029f0`
   (`FUN_180004f80`, gradient-magnitude style edge detector).
5. **Circular Hough transform** `FUN_180005060`: for r = 26…80 step 1.0, vote
   both x-intersections of the circle through every edge pixel, take the peak,
   score = votes / r; keep the best radius. Accept when score > 1.7.
6. Re-run on a `(2r+60)²` ROI around the hit with threshold 40.5 and radii
   `r+2 … r+12` step 0.5 (`local_90 = 0.5`). If the refined hit fails, keep the
   coarse result when at least half the ROI pixels are lit; otherwise reject
   (`return 4`).
7. Reject radii < 26 px. Set edge flag `FUN_180006e00(R, 4.0)` (ball within
   r/4 px of the image border), `valid = 1`.

### 6.3 SUMT decision after stage 1 (`FUN_18001cd40`, confirmed)

* Any valid image with circle score < 2.7 (variant ≤ 58) or < 3.9 (variant > 58)
  marks the shot "questionable" (`E+4 = 1`).
* If not questionable (or the smallest radius ≤ 35 px on ≤58 variants): run
  `FUN_18000bbe0` (radius consistency — linearly interpolates the third image's
  radius from the other two using the timestamps and snaps it if the measured
  value deviates ≥ 0.5 px), then go straight to geometry (post 5 × n).
* Otherwise post 4 to each stage-2 thread for radius refinement.

### 6.4 Stage 2 — iso-intensity radius refinement (`FUN_180009c40`, confirmed)

Filter the source once, then for threshold `T = 100.5, 120.5, … 630.5`
threshold → edge → Hough (radii 35…70) and record `(r, cx, cy)` when score
> 2.0 (a preferred set uses > 2.6). Needs ≥ 5 samples. For three row
offsets (−2, −1, 0 around the mean centre row) it computes left/right
edge x = `cx ∓ sqrt(r² − dy²)` for every sample, fits a line of edge
position against `T²` (`FUN_180005480` = linear regression), and
extrapolates both edges to `T² = 3600` (T = 60). Radius := half the
edge separation at that iso-level; accepted if 40 ≤ r ≤ 70 px, else the
stage-1 radius is kept. This is where the `2.5 px` fallback perturbation in
SUMT is applied when the shot is questionable (radii ±2.5 and recomputed).

### 6.5 Geometry — pixels to metres (`FUN_18000b420`, confirmed formulas)

Constants (defaults `FUN_18001c720 @ 18001c720`):

| Symbol | Engine addr | Default | Meaning |
|---|---|---|---|
| `D` | `E+0x918` | 0.04267 (0x3D2EC6BD) | ball diameter, **metres** |
| `offX0` | `E+0x91C` | −0.057 | lateral offset between the two views (re-estimated per shot when a same-timestamp pair exists) |
| `offY` | `E+0x920` | −11.0 px | vertical pixel offset applied to view 0 (re-estimated likewise) |
| `θ½` | `E+0xA64` | 26.7° (0x41D5999A, also set on every static-data packet) | half field of view along the 752-px axis |

```
f      = 376 / tan(θ½)                       # ≈ 747.6 px
Z      = D * f / (2 r)                       # depth (m)
sign   = +1 if hdr[0x0A]==0 else -1
offX   = ((Z - 0.25) * 0.02 + offX0) * sign * 0.5
X      = (cx_abs - 240) * D / (2 r) + offX   # along target line (m)
Y      = (cy_abs + (offY if hdr[0x0A]==0 else 0) - 376) * D / (2 r)   # image-down positive (m)
HAimg  = atan((X - offX) / Z) * 57.2958
VAimg  = atan(((cy_abs - 376) * D / (2 r)) / Z) * 57.2958
```

Note: `RIPESetEngineParams` accepts a value in (38, 48) and stores it raw into
`E+0x918`, which the engine reads as metres. Either the managed wrapper divides
by 1000 before calling, or calling it breaks the geometry — treat as an open
question and do not copy that behaviour.

### 6.6 Velocity, launch and horizontal angle (`FUN_18000b620`, confirmed)

For each image pair `(i, j)`, `i < j < 3`, both valid:

* If `ts_i == ts_j` → simultaneous pair: update `offY = cy_j − cy_i` and
  `offX0 = (cx_j−240)·D/(2r) − (cx_i−240)·D/(2r)` (views swapped by the
  `hdr[0x0A]` flag). This is the in-shot stereo calibration.
* Else: `dt = (ts_j − ts_i)/1e6` (u16 wrap is **not** handled — assume the
  three timestamps never straddle 65 535 µs),
  `vx = (X_j−X_i)/dt`, `vy = −(Y_j−Y_i)/dt`, `vz = −(Z_j−Z_i)/dt`
  (`vz` and HA negated again for left-handed),
  `speed = |v|`, `LA = atan(vy/vx)·57.2958`, `HA = atan(vz/vx)·57.2958`.
  Results per pair at `E+0x7D8 + k*0x68` (k = 0,1,2 for (0,1),(0,2),(1,2)).
* The first pair with `LA > −5°`, `vx > 0`, and neither image touching the edge
  is taken as the shot result (`E+0xEAC..0xEC0` = LA, HA, |v|, vx, vy, vz) with
  `LA += tilt_pitch` from the accelerometer (below). A second acceptable pair
  with the same `hdr[0x0A]` flags is not taken.
* `FUN_18000be90`: if `LA > 45°` and the third image is unreliable, fall back to
  pair (0,1) (or (1,2) for LH) and adjust HA by ±4° or halve it depending on the
  radius trend (**confirmed** code, **guess** intent).
* `FUN_18000bd40` writes the public result `E+0xEF8..0xF18` with confidences:

| Public field | Engine addr | Rule |
|---|---|---|
| launchAngle | `E+0xEF8` | conf 1.0 if 0 < LA < 56; 0.5 if −5 < LA < 56; else invalid |
| horizontalAngle | `E+0xEFC` | conf 1.0 if \|HA\| ≤ 24; 0.5 if ≤ 32 (or speed > 44 m/s → 0.5, angle dropped); else invalid |
| totalSpeed (m/s) | `E+0xF00` | conf 1.0 if 1 < v < 96; ≥ 96 → value clamped to 2.0, conf 0.5; ≤ 1 invalid |
| vx, vy, vz | `E+0xF04..0xF0C` | copied |
| speed / HA / LA conf | `E+0xF10 / 0xF14 / 0xF18` | any 0 ⇒ the whole record is zeroed (no shot) |

* `FUN_18000c010` then adds a **speed correction from a 7 × 5 table**
  `DAT_1803FC050` indexed by speed bin (mph: <140, 140–150, 151–160, 161–170,
  171–180, 181–190, 191–212) and HA bin (−24..−6, −6..−2, −2..2, 2..24, else):
  `speed += table[bin][col] * 0.4472` (mph→m/s). Table values are in `.rdata`
  and were not dumped — see open questions.

Tilt correction (`FUN_1800140e0`, `FUN_180016110`, **confirmed**): the static
data packet carries 12 floats at packet offset `0x33C` (three `{1/scale, ox, oy,
oz}` sets; valid when `1/scale ∈ (14744.7, 18021.3)` or `(920.7, 1125.3)`) =
"6 axis calib", else three i32 at offset `0x158` = "1 axis calib" reference
vector. With the live accelerometer `E+0xE14..E1C`, pitch and roll are computed
with `atan2`; pitch (`E+0x928`) is added to LA, roll (`E+0x924`) is stored but
unused by the getters.

### 6.7 Public speed record

`RIPEGetSpeedReadyData` copies 7 u32 from `*(state+0x18)` (written by SUMT) and
then calls `FUN_1800145b0` for the position status:

| Index | Source | Field |
|---|---|---|
| 0 | `E+0xF00` | totalSpeed (m/s) |
| 1 | `E+0xF10` | totalSpeedConfidence |
| 2 | `E+0xEF8` | launchAngle (deg) |
| 3 | `E+0xF18` | launchAngleConfidence |
| 4 | `E+0xEFC` | horizontalAngle (deg) |
| 5 | `E+0xF14` | horizontalAngleConfidence |
| 6 | `FUN_1800145b0` | startBallPositionStatus |

Ball position (`FUN_1800149a0`, **confirmed** formula, **likely** meaning):
`pos = 0.33344 − |Z| − (|k| − |X_noOff|) / tan(90° − sign·HA)` with
`k = −0.1790` (variant ≤ 68) or `−0.1209` (variant > 68), using the reference
image chosen by hand and variant. Then `d = 300 − pos·1000` (mm):
`d ≤ 260 → 1 NEAR`, `d > 315 → 2 FAR`, else `0 OK`; no valid image → `3
UNKNOWN`. The nominal ball-to-unit distance is therefore 0.333 m along the
camera axis.

### 6.8 Timing of the events (confirmed, `FUN_18001cd40`)

Speed is deliberately delayed to `1100 + rand()%200` ms after shot start, spin
to `2450 + rand()%200` ms (`Sleep`). A reimplementation can drop the delays.

### 6.9 Spin (`FUN_180007290` ×3, `FUN_180007140`, `FUN_180006ee0`, `FUN_180007090`)

**Confirmed structure, algorithm details likely:**

* Three threads run the same function with `k = 0,1,2`, each searching a
  spin-axis band: `[−40°, −14°]`, `[−13°, +13°]`, `[+14°, +40°]` in 60 steps.
* Image pair selection depends on hand, `speed ≥ 80` and the valid/edge flags of
  the three records (preference order in the code; the pair must not touch the
  image edge).
* Both ball images are normalised to a fixed radius (constants `219.9115 =
  2π·35` and `109.95575 = π·35` ⇒ a 35-px reference radius, texture maps of
  ~220 px circumference), the first is rotated by a candidate
  `(axis, angle)` via `FUN_180003db0` (cos/sin, 2π) with pixel warping
  `FUN_180002d80`, and compared with the second via `FUN_1800059d0`
  (normalised correlation with masks; asserts `IPE(9)…`). Rotation angle /
  `dt` → rpm.
* Per band result block `E+0x804 + k*0x68`: total rpm `+0x804`, axis `+0x808`,
  backspin `+0x80C`, sidespin `+0x810`, confidence `+0x820`.
  `FUN_180007140` picks the band with the highest confidence whose backspin lies
  in `(−15000, −50)` rpm (negative = backspin).
* `FUN_180006ee0`: confidence 1.0, or 0.5 if either image is within r/4 px of
  the border; if the axis came out 0 it is synthesised as `0.625 · HA` and
  back/side spin are re-projected from total spin with confidence 0.5.
* `FUN_180007090` publishes only if speed/HA/LA confidences are all non-zero
  and `0 ≤ total < 15001`.

Public spin record (`RIPEGetSpinReadyData`): `[0]=E+0xF1C total, [1]=E+0xF24
back, [2]=−(E+0xF28) side (sign flipped in the getter), [3]=E+0xF20 axis,
[4]=E+0xF34 confidence`. Returns 4 (not ready) when mode==2 and LA < 10°.

---

## 7. Flight model (`FUN_180001df0`, `FUN_1800020b0`, `FUN_180002480`) — confirmed

The flight data is a pure numeric integration on the host with no box input, so
it can be replaced by any trajectory model.

* Inputs: `speed{LA°, HA°, v m/s}` (`E+0xEF8`), `spin{total rpm, axis°}`.
  Speed-only variant uses `total = 3000 rpm, axis = 0` (`0x453B8000`).
* Initial state (9 doubles): position 0, `v = (v cosLA cosHA, v sinLA, v cosLA
  sinHA)`, `ω = total/60 · (cos axis · …)` rev/s decomposed by axis and HA.
* Integrator: **RK4**, `dt = 0.01 s`, up to 20 s (≤ 2000 steps), stop when
  height < 0; sample `{x, y, z, t}` as doubles every step ⇒ the 2010-point
  `RIPEFlightParams.pts` array.
* Derivative `FUN_180002480`:

```
r = 0.0213 m, A = π r² = 0.0014253 m², ρ = 1.1766 kg/m³, m = 1/21.7865 = 0.0459 kg
S  = |ω|·2π·r / |v|                        # spin ratio
CL = 0.9879·S + 0.0694      (S ≤ 0.24)
   = 0.22·S + 0.275         (0.24 < S ≤ 0.7)
   = 0.4                    (S > 0.7)
CD = 1.49455·CL² + 0.19027
L  = ½ ρ A CL |v|²,  Dr = ½ ρ A CD |v|²
a  = (−Dr·v̂ + L·(lift dir from spin-axis angles)) / m − (0, 9.81, 0)
dω/dt = −2e-5 · ω · |v| / r                # spin decay (per component)
```

* Outputs: `carry = x_end`, `side = z_end`, `maxHeight`, `flightDuration`,
  `validPointCount` (`RIPEFlightParams`, 0xFB58 bytes: 4 floats + int + pad +
  2010 × 32 bytes). Available only when `RIPEInit` param `+0x14` enabled flight
  (`E+0xF44`) and mode ≠ 2.

---

## 8. `.rif` files (`RIPEStartSaving`, PSR case 0/1, `RIPEFeedOfflineRifData`) — confirmed

`RIPEStartSaving(handle, path)` only stores the path (`state+0x40 → +0x30`).
The comm threads append **every raw received byte** to a 0x12CE20-byte ring
buffer (`G+0x38`, index `G+0x30`) *before* feeding it to the parser
(`FUN_180018890` lines 138–147; same in the TCP thread). PSR case 0 (shot
start) resets the index and pre-writes context; PSR case 1 (shot end) writes
the buffer to the file (`remove` + `fopen "wb"` + `fwrite`) and resets it.

Resulting file layout:

| Offset | Size | Content |
|---|---|---|
| 0 | 0x4E0 | copy of the last `0xAAAAAAAA` static-data packet exactly as stored at `parser+0x12C6F6` (magic + len + payload, clear) |
| 0x4E0 | 0xA0 (optional) | copy of the `0xBBBBBBBB` status packet from `parser+0x1200A2`, present only if status word `+0x2C` ("configured") ≠ 0 |
| next | 4 + 4 | `0xCCCCCCCC`, `total_len` of the shot header packet |
| next | … | **raw wire bytes, still AES-encrypted**: shot header payload, all `0xDDDDDDDD` image packets, the `0xEEEEEEEE` trigger packet if any |

`RIPEFeedOfflineRifData(handle, path)` reads the file in 90-byte (0x5A) chunks
and pushes them through `FUN_18000c180` as if received from the box. The
leading static-data copy makes the parser initialise the AES key from the
recorded firmware version, so a `.rif` is self-contained and replays the whole
pipeline (events 0x1F/0x20/0x21 fire again). This is the recording format to
use for offline regression tests; decrypt with the FW-version key from
`framing-and-crypto.md` §3.3 to get plaintext images.

Caveats: the ring buffer wraps at 0x12CE20 bytes (1.23 MB) — three 393 KB
images plus headers fit, but a shot with more/larger packets would be
truncated; the file is only written when the shot completes normally.

---

## 9. Constants summary (confirmed unless noted)

| Item | Value |
|---|---|
| Image slot stride | 0x6001A |
| Image header size | 0x1A (26 bytes incl. magic/len) |
| Working-buffer cap | 0x40000 px |
| Image centre | (240, 376) px; half-FOV 26.7° over 752 px ⇒ f ≈ 747.6 px |
| Ball diameter | 0.04267 m |
| Detection thresholds | 200 (coarse), 40.5 (refine), 100.5…630.5 step 20 (iso sweep), 1000 (saturation), 1.7 Hough score |
| Radii | 26…80 coarse, refine +2…+12 step 0.5, stage-2 35…70, accept 40…70; edge flag r/4 |
| Score gates | 2.7 (variant ≤ 58) / 3.9 (variant > 58); 2.0 / 2.6 (stage 2) |
| Pair acceptance | LA > −5°, vx > 0, no edge contact |
| Confidence gates | speed (1, 96) m/s, LA (−5, 56)°, HA 24° / 32° |
| Spin gates | back ∈ (−15000, −50) rpm, total < 15001, bands ±14/±40°, 60 steps |
| Event delays | 1100 + rand%200 ms (speed), 2450 + rand%200 ms (spin) |
| Ball position | nominal 0.33344 m, NEAR ≤ 260 mm, FAR > 315 mm |

---

## 10. Reimplementation checklist (Rust, no DLL)

1. **Transport + framing** (see `framing-and-crypto.md`): byte-stream sync on the
   eight magics, LE `total_len`, route `0xCCCCCCCC`/`0xDDDDDDDD` through
   AES-128-ECB with the FW-version key, keep the in-shot flag semantics
   (`0xEEEEEEEE` ignored outside a shot).
2. **Shot assembly**: parse header words 0–3; collect `A+B` images (expect 3);
   end the shot on the last image or on the trigger packet when word 2 ≠ 0.
3. **Image decode**: header fields at 0x08–0x18; transpose `src[a*B+b] →
   img[b][a]`; 8-bit ×4 or 16-bit; carry `ts_us`, `view`, `roiA`, `roiB`.
4. **Ball detection**: 3×3 smooth (recover kernel `DAT_1803FC210` from
   `.rdata`), threshold 200, gradient edge (kernels `DAT_1803FBE48/DB8`),
   circular Hough 26…80 px, score > 1.7, ROI refine at 40.5 / step 0.5.
5. **Radius refinement** (optional first cut: skip when score ≥ 2.7): iso-level
   sweep and linear extrapolation to T = 60 as in §6.4; accept 40…70 px.
6. **Geometry**: §6.5 formulas with `D = 0.04267 m`, `f = 376/tan(26.7°)`,
   centre (240, 376), view-dependent offsets; re-estimate `offX0/offY` from a
   same-timestamp pair when present.
7. **Velocity/angles**: §6.6, first acceptable pair, hand sign, tilt correction
   from the static-data calib floats + live accelerometer.
8. **Plausibility/confidence**: §6.6 table; then the 7×5 speed correction (dump
   `DAT_1803FC050` first — until then omit; it only affects ≥ 140 mph rows in
   practice if row 0 is zero, which must be verified).
9. **Ball position status**: §6.7 formula.
10. **Spin**: unwrap both ball images to a 35-px-radius texture, search
    `(axis ∈ ±40°, angle)` by normalised correlation, rpm = angle/Δt; report
    total/back/side/axis with the sign convention `side = −stored`. Expect this
    to need real captures to tune; the vendor code is ~9 KB of tightly coupled
    float code.
11. **Flight**: RK4 model of §7 or your own; output 0.01 s samples until y < 0.
12. **Recording**: implement `.rif` writing exactly as §8 so files are
    interchangeable with the vendor SDK, and build offline tests from them.

Size estimate (honest): framing/AES ≈ 300 lines, shot assembly + decode ≈ 200,
detection + refinement ≈ 700, geometry/velocity/confidence/tilt ≈ 400, flight
≈ 150, `.rif` ≈ 100 → **~1.9 k lines for a speed/LA/HA-complete port that can
be validated bit-for-bit against `.rif` replays**. Spin adds ≈ 1–1.5 k lines
and is the only part whose numerical agreement with the vendor is uncertain
without captures. The engine is small (42 KB of x64), hand-written, and free of
library dependencies, so a faithful port is realistic; the unknown `.rdata`
tables/kernels are the only missing inputs.

---

## 11. Open questions

1. **Sensor geometry**: 480 × 752 (from the 240/376 centres and `h > 500`
   branch) vs 512 × 768 (exact `0x60000` fit). Needs one decrypted image
   header (`+0x14`, `+0x18`).
2. **Meaning of `hdr[0x0A]` view flag**: two cameras, a mirror-split sensor,
   or two strobes? Same-timestamp pairs with different flags exist in the code
   path (`FUN_18000b620`), which implies two simultaneous views.
3. **Header word 3 (`parser+0x40`, thresholds 58/68)**: firmware/hardware
   variant, frame rate, or exposure? Recover from captures of a known box.
4. **Image header `+0x10`** and `+0x0C` semantics (unused / binning flag).
5. **`.rdata` tables not in the function dump**: smoothing kernel
   `DAT_1803FC210`, edge kernels `DAT_1803FBE48`/`DAT_1803FBDB8`, speed
   correction table `DAT_1803FC050` (7×5 f32). Dump the data section.
6. **Trigger packet payload**: never read; content unknown. Might carry
   FPGA trigger timestamps useful for validation.
7. **`RIPESetEngineParams` units**: validates 38–48 but stores into a metres
   field; check the managed wrapper.
8. **Timestamp wrap**: u16 µs with no wrap handling — confirm the box resets the
   counter per shot.
9. **Left-hand mapping** of `E+0xE44` (assumed 0 = RH) and shot-mode code 2 =
   putting — confirm against `RIPE_CMD_PLAYER_HAND_*` / `SHOT_MODE_PUTTING`
   command encodings in `framing-and-crypto.md` §4.
10. **Spin algorithm fidelity**: the exact texture unwrapping and correlation
    (`FUN_180003db0`, `FUN_1800059d0`, `FUN_180002d80`) were characterised, not
    transcribed; a line-by-line port should start from those three functions.
