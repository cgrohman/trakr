//! Turns a shot's decrypted camera images into ball launch conditions.
//!
//! The box sends 2-3 grayscale camera images per shot rather than finished
//! numbers (see `docs/skytrak-protocol/shot-data.md`); this module is the
//! missing "host SDK" half of that pipeline. Ball detection
//! (`detect_ball`/`hough_circle`) is a direct transcription of the vendor's
//! own decompiled functions (`PlatformLib.dll`'s `FUN_180009530` /
//! `FUN_180005060` and friends): the same recovered smoothing kernel, the
//! same two Sobel-style edge kernels, the same threshold (200 on a 0..1020
//! scale), and the same per-radius circular Hough transform with the
//! `score = peak_votes / r > 1.7` acceptance gate. All four previously
//! "unrecovered" `.rdata` constants (smoothing kernel, two edge kernels, the
//! 7x5 mph speed-correction table) were pulled directly from
//! `research/oracle/PlatformLib.dll`'s data section by mapping documented
//! virtual addresses through the PE section table -- see git history for the
//! recovery script. Geometry and velocity (`compute_position`/
//! `compute_velocity`) implement the formulas from that document, confirmed
//! against the same decompile.
//!
//! What's *not* a faithful port: the vendor's ROI-based radius refinement
//! (re-running Hough on a tight crop with a lower threshold, `shot-data.md`
//! §6.2 step 6) and the iso-intensity stage-2 refinement (§6.4) are both
//! skipped -- the doc's own reimplementation checklist marks stage 2 as
//! optional "first cut: skip when score >= 2.7", and the ROI refine step is
//! a performance optimization over the full-frame Hough (same result set,
//! smaller search space), not a correctness difference. Boundary handling
//! for the 3x3 convolutions is zero-padding rather than the vendor's
//! pad-and-recrop; irrelevant in practice since a real ball is never that
//! close to the sensor edge. Not validated against the vendor SDK's own
//! output (see `docs/skytrak-protocol/session.md` for why -- no working
//! path to run the vendor SDK against this hardware was found), so
//! confidences stay deliberately below `CERTAIN`.
//!
//! **Spin is not implemented.** It needs the vendor's texture-unwrap +
//! rotation-correlation search (`shot-data.md` §6.9), which is ~1-1.5k lines
//! of numerically uncertain code even by the reverse-engineering doc's own
//! estimate. `BallData::total_spin_rpm` etc. are always `None` here.
//!
//! **Tilt correction is partially implemented.** `tilt_pitch_deg` covers
//! only the "1 axis calib" branch of `FUN_1800140e0` -- the one this real
//! box actually uses (confirmed live: its 6-axis `1/scale` value doesn't
//! fall in either documented valid range). The "6 axis calib" branch is
//! recognized but not computed (`TiltCalibration::SixAxisUnsupported`),
//! since it needs an undocumented rotation transform (`FUN_180013fc0`) we
//! haven't recovered. On this box, measured tilt is under half a degree, so
//! the correction is real but small in practice.

use aes::cipher::{BlockDecrypt, KeyInit};
use aes::Aes128;
use generic_array::GenericArray;

use trakr_core::{BallData, Confidence};

use crate::wire::TiltCalibration;

/// Accelerometer-based launch-angle tilt correction, `shot-data.md` §6.6,
/// transcribed from `FUN_1800140e0`'s "1 axis calib" branch (the only branch
/// implemented -- see `TiltCalibration::SixAxisUnsupported`). Returns the
/// degrees to *add* to the raw launch angle, or `None` when no usable
/// calibration reference is available.
pub fn tilt_pitch_deg(calib: TiltCalibration, accel: (i32, i32, i32)) -> Option<f32> {
    let TiltCalibration::OneAxis {
        x: rx,
        y: ry,
        z: rz,
    } = calib
    else {
        return None;
    };
    let (ax, ay, az) = accel;
    let live = (az as f64).atan2(((ax * ax + ay * ay) as f64).sqrt());
    let reference = (rz as f64).atan2(((rx * rx + ry * ry) as f64).sqrt());
    Some(-((reference.to_degrees() - live.to_degrees()) as f32))
}

/// Image sensor geometry constants, `docs/skytrak-protocol/shot-data.md` §6.5.
const IMAGE_CENTER_X: f32 = 240.0;
const IMAGE_CENTER_Y: f32 = 376.0;
const BALL_DIAMETER_M: f32 = 0.04267;
const HALF_FOV_DEG: f32 = 26.7;
const DEFAULT_OFFSET_X0: f32 = -0.057;
const DEFAULT_OFFSET_Y_PX: f32 = -11.0;

fn focal_px() -> f32 {
    376.0 / HALF_FOV_DEG.to_radians().tan()
}

/// 3x3 smoothing kernel, recovered from `PlatformLib.dll`'s `.rdata` at the
/// documented `DAT_1803fc210` (file offset `0x3fac10`). Normalized Gaussian
/// -like blur (sums to ~1.0).
const SMOOTH_KERNEL: [[f32; 3]; 3] = [
    [0.0439, 0.1217, 0.0439],
    [0.1217, 0.3377, 0.1217],
    [0.0439, 0.1217, 0.0439],
];

/// Sobel-style horizontal-edge kernel, `DAT_1803fbe48` (file offset `0x3fa848`).
const EDGE_KERNEL_GX: [[f32; 3]; 3] = [[1.0, 2.0, 1.0], [0.0, 0.0, 0.0], [-1.0, -2.0, -1.0]];

/// Sobel-style vertical-edge kernel, `DAT_1803fbdb8` (file offset `0x3fa7b8`).
const EDGE_KERNEL_GY: [[f32; 3]; 3] = [[1.0, 0.0, -1.0], [2.0, 0.0, -2.0], [1.0, 0.0, -1.0]];

const HOUGH_THRESHOLD: f32 = 200.0;
const HOUGH_R_MIN: f32 = 26.0;
const HOUGH_R_MAX: f32 = 80.0;
const HOUGH_R_STEP: f32 = 1.0;
const HOUGH_SCORE_MIN: f32 = 1.7;

/// mph correction added to speed based on (speed bin, horizontal-angle bin),
/// `DAT_1803fc050` (file offset `0x3faa50`), applied as
/// `speed_mps += TABLE[speed_bin][ha_bin] * 0.44704`. Speed bins (mph):
/// `<140, 140-150, 151-160, 161-170, 171-180, 181-190, 191-212`. HA bins
/// (deg): `-24..-6, -6..-2, -2..2, 2..24, else`.
const SPEED_CORRECTION_MPH: [[f32; 5]; 7] = [
    [0.0, 0.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 2.0, 0.0, 0.0],
    [1.0, 2.0, 3.0, 2.0, 0.0],
    [2.0, 3.0, 4.0, 3.0, 0.0],
    [4.0, 5.0, 6.0, 5.0, 0.0],
    [5.0, 6.0, 7.0, 5.0, 0.0],
    [6.0, 6.0, 8.0, 6.0, 0.0],
];

fn speed_correction_mph(speed_mph: f32, ha_deg: f32) -> f32 {
    let speed_bin = if speed_mph < 140.0 {
        0
    } else if speed_mph <= 150.0 {
        1
    } else if speed_mph <= 160.0 {
        2
    } else if speed_mph <= 170.0 {
        3
    } else if speed_mph <= 180.0 {
        4
    } else if speed_mph <= 190.0 {
        5
    } else {
        6
    };
    let ha_bin = if ha_deg < -6.0 {
        0
    } else if ha_deg < -2.0 {
        1
    } else if ha_deg < 2.0 {
        2
    } else if ha_deg <= 24.0 {
        3
    } else {
        4
    };
    SPEED_CORRECTION_MPH[speed_bin][ha_bin]
}

/// AES-128-ECB decrypt, block by block, no padding (the wire format has none
/// -- ciphertext length is always a multiple of 16 by construction).
pub fn aes128_ecb_decrypt(key: &[u8; 16], ciphertext: &[u8]) -> Vec<u8> {
    let cipher = Aes128::new(&GenericArray::clone_from_slice(key));
    let mut out = ciphertext.to_vec();
    for block in out.as_chunks_mut::<16>().0 {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
    }
    out
}

/// Decrypted `0xCCCCCCCC` shot header payload (magic+len already stripped).
pub struct ShotHeader {
    pub expected_images: u32,
    pub trigger_follows: bool,
}

pub fn parse_shot_header(payload: &[u8]) -> Option<ShotHeader> {
    if payload.len() < 12 {
        return None;
    }
    let count_a = u32::from_le_bytes(payload[0..4].try_into().ok()?);
    let count_b = u32::from_le_bytes(payload[4..8].try_into().ok()?);
    let trigger_follows = u32::from_le_bytes(payload[8..12].try_into().ok()?) != 0;
    Some(ShotHeader {
        expected_images: count_a + count_b,
        trigger_follows,
    })
}

/// Decrypted `0xDDDDDDDD` image payload (magic+len already stripped).
struct ShotImage {
    ts_us: u16,
    view: u16,
    binned: bool,
    width: usize,
    height: usize,
    roi_a: u16,
    roi_b: u16,
    /// One byte per pixel, row-major with row length `height`
    /// (`pixels[a * height + b]`, per the doc's transpose convention).
    pixels: Vec<u8>,
}

fn parse_shot_image(payload: &[u8]) -> Option<ShotImage> {
    if payload.len() < 18 {
        return None;
    }
    let ts_us = u16::from_le_bytes(payload[0..2].try_into().ok()?);
    let view = u16::from_le_bytes(payload[2..4].try_into().ok()?);
    let mode = u16::from_le_bytes(payload[4..6].try_into().ok()?);
    let bpp = u16::from_le_bytes(payload[6..8].try_into().ok()?);
    let roi_a = u16::from_le_bytes(payload[10..12].try_into().ok()?);
    let width = u16::from_le_bytes(payload[12..14].try_into().ok()?) as usize;
    let roi_b = u16::from_le_bytes(payload[14..16].try_into().ok()?);
    let height = u16::from_le_bytes(payload[16..18].try_into().ok()?) as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let bytes_per_pixel = if bpp == 2 { 2 } else { 1 };
    let needed = width.checked_mul(height)?.checked_mul(bytes_per_pixel)?;
    if payload.len() < 18 + needed {
        return None;
    }
    let raw = &payload[18..18 + needed];
    let pixels: Vec<u8> = if bytes_per_pixel == 1 {
        raw.to_vec()
    } else {
        // 16-bit LE, already on a wider scale; downscale to 8-bit for the
        // detector below. Untested against real hardware (both real
        // captures we have are 8-bit).
        raw.as_chunks::<2>()
            .0
            .iter()
            .map(|b| ((u16::from_le_bytes(*b) >> 2).min(255)) as u8)
            .collect()
    };
    Some(ShotImage {
        ts_us,
        view,
        binned: mode != 0,
        width,
        height,
        roi_a,
        roi_b,
        pixels,
    })
}

struct BallHit {
    /// ROI-relative centroid on the `a` (width) axis.
    cx_roi: f32,
    /// ROI-relative centroid on the `b` (height) axis.
    cy_roi: f32,
    radius: f32,
    edge_touch: bool,
}

/// `src[a*height+b]` (doc's storage convention, a = width/x axis outer) into
/// a standard row-major host image (`img[y*width+x]`), applying the
/// documented 8-bit -> 0..1020 scale (`<<2`). This is literally "the decoder
/// transposes" step from `shot-data.md` §3.1.
fn to_host_image(img: &ShotImage) -> Vec<f32> {
    let mut out = vec![0f32; img.width * img.height];
    for x in 0..img.width {
        for y in 0..img.height {
            out[y * img.width + x] = (img.pixels[x * img.height + y] as f32) * 4.0;
        }
    }
    out
}

/// 3x3 convolution, zero-padded at the border (see module docs for why this
/// doesn't match the vendor's pad-and-recrop exactly).
fn convolve3x3(img: &[f32], width: usize, height: usize, kernel: &[[f32; 3]; 3]) -> Vec<f32> {
    let mut out = vec![0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut acc = 0f32;
            for (ky, krow) in kernel.iter().enumerate() {
                for (kx, &kval) in krow.iter().enumerate() {
                    let sy = y as isize + ky as isize - 1;
                    let sx = x as isize + kx as isize - 1;
                    let v = if sy >= 0 && (sy as usize) < height && sx >= 0 && (sx as usize) < width
                    {
                        img[sy as usize * width + sx as usize]
                    } else {
                        0.0
                    };
                    acc += v * kval;
                }
            }
            out[y * width + x] = acc;
        }
    }
    out
}

/// `FUN_180002b20`: binarize against a threshold.
fn threshold_binary(img: &[f32], threshold: f32) -> Vec<f32> {
    img.iter()
        .map(|&v| if v > threshold { 1.0 } else { 0.0 })
        .collect()
}

/// `FUN_1800029f0`: gradient-magnitude-style edge map, `|gx| + |gy|`.
fn edge_map(binary: &[f32], width: usize, height: usize) -> Vec<f32> {
    let gx = convolve3x3(binary, width, height, &EDGE_KERNEL_GX);
    let gy = convolve3x3(binary, width, height, &EDGE_KERNEL_GY);
    gx.iter()
        .zip(gy.iter())
        .map(|(a, b)| a.abs() + b.abs())
        .collect()
}

struct HoughHit {
    cx: f32,
    cy: f32,
    r: f32,
    score: f32,
}

/// Circular Hough transform, transcribed directly from `FUN_180005060`: for
/// each candidate radius, every edge pixel votes for the two candidate
/// centers (on each candidate center row) that a circle of that radius
/// through it would have; keep the radius whose peak accumulator cell has
/// the best `votes / r` score. Accept only if that score beats 1.7 (the
/// vendor's exact gate).
fn hough_circle(edge: &[f32], width: usize, height: usize) -> Option<HoughHit> {
    // The edge map doesn't change across radii, so collect the (typically
    // sparse) nonzero pixels once instead of rescanning the full width x
    // height grid on every one of the ~55 radius steps.
    let edge_pixels: Vec<(usize, usize)> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|&(x, y)| edge[y * width + x] != 0.0)
        .collect();

    let mut acc = vec![0f32; width * height];
    let mut best: Option<HoughHit> = None;
    let mut r = HOUGH_R_MIN;
    while r <= HOUGH_R_MAX {
        for v in acc.iter_mut() {
            *v = 0.0;
        }
        let mut peak = (0f32, 0usize, 0usize);
        for &(x, y) in &edge_pixels {
            let cy_lo = ((y as f32 - r).floor().max(0.0)) as usize;
            let cy_hi = (((y as f32 + r).ceil()) as usize).min(height - 1);
            for cy in cy_lo..=cy_hi {
                let dy2 = ((y as isize - cy as isize) as f32).powi(2);
                if dy2 > r * r {
                    continue;
                }
                let dx = (r * r - dy2).sqrt();
                let cx_minus = (x as f32 - dx + 0.5).floor();
                if cx_minus >= 0.0 {
                    let idx = cy * width + cx_minus as usize;
                    acc[idx] += 1.0;
                    if acc[idx] > peak.0 {
                        peak = (acc[idx], cy, cx_minus as usize);
                    }
                }
                let cx_plus = (x as f32 + dx + 0.5).floor();
                if (cx_plus as usize) < width {
                    let idx = cy * width + cx_plus as usize;
                    acc[idx] += 1.0;
                    if acc[idx] > peak.0 {
                        peak = (acc[idx], cy, cx_plus as usize);
                    }
                }
            }
        }
        let score = peak.0 / r;
        if best.as_ref().map(|b| score > b.score).unwrap_or(true) {
            best = Some(HoughHit {
                cx: peak.2 as f32,
                cy: peak.1 as f32,
                r,
                score,
            });
        }
        r += HOUGH_R_STEP;
    }
    best.filter(|b| b.score > HOUGH_SCORE_MIN)
}

/// Ball detection: `shot-data.md` §6.2, transcribed from `FUN_180009530`'s
/// coarse pass (smooth -> threshold 200 -> Sobel edges -> circular Hough).
/// See module docs for the two skipped refinement passes.
fn detect_ball(img: &ShotImage) -> Option<BallHit> {
    if img.binned {
        // The one binned/sub-sampled real capture we have showed a smeared
        // blob spanning almost the whole frame, not a clean ball position --
        // treat as informational only until the doubling/rebinning semantics
        // (shot-data.md open question 4) are understood.
        return None;
    }
    let host = to_host_image(img);
    let smoothed = convolve3x3(&host, img.width, img.height, &SMOOTH_KERNEL);
    let binary = threshold_binary(&smoothed, HOUGH_THRESHOLD);
    let edges = edge_map(&binary, img.width, img.height);
    let hit = hough_circle(&edges, img.width, img.height)?;
    let edge_margin = hit.r / 4.0;
    let edge_touch = hit.cx < edge_margin
        || hit.cx > img.width as f32 - edge_margin
        || hit.cy < edge_margin
        || hit.cy > img.height as f32 - edge_margin;
    Some(BallHit {
        cx_roi: hit.cx,
        cy_roi: hit.cy,
        radius: hit.r,
        edge_touch,
    })
}

/// A ball detection plus everything geometry needs, kept separate from
/// `ImagePosition` because computing `X`/`Y` requires the shot's stereo
/// calibration (`off_x0`/`off_y_px`), which isn't known until all of a
/// shot's images have been through detection (see `estimate_stereo_calibration`).
struct RawHit {
    ts_us: u16,
    view: u16,
    /// Absolute image-frame centroid (ROI-relative + ROI offset).
    cx_abs: f32,
    cy_abs: f32,
    radius: f32,
    edge_touch: bool,
}

fn raw_hit(img: &ShotImage, hit: &BallHit) -> RawHit {
    RawHit {
        ts_us: img.ts_us,
        view: img.view,
        cx_abs: hit.cx_roi + img.roi_a as f32,
        cy_abs: hit.cy_roi + img.roi_b as f32,
        radius: hit.radius,
        edge_touch: hit.edge_touch,
    }
}

/// Per-shot stereo calibration, `shot-data.md` §6.6: when two images share a
/// timestamp (a simultaneous stereo pair), `offX0` (lateral offset between
/// the two camera views) and `offY` (vertical pixel offset applied to view 0)
/// are re-estimated from that pair instead of using the fixed defaults.
/// Falls back to the defaults when no shot has a same-timestamp pair (as with
/// both real shots captured so far -- all three images arrived at distinct
/// timestamps), which is why this made no difference to that regression test.
fn estimate_stereo_calibration(hits: &[RawHit]) -> (f32, f32) {
    for i in 0..hits.len() {
        for j in (i + 1)..hits.len() {
            let (a, b) = (&hits[i], &hits[j]);
            if a.ts_us != b.ts_us {
                continue;
            }
            let x_term =
                |h: &RawHit| (h.cx_abs - IMAGE_CENTER_X) * BALL_DIAMETER_M / (2.0 * h.radius);
            let off_x0 = x_term(b) - x_term(a);
            let off_y = b.cy_abs - a.cy_abs;
            return (off_x0, off_y);
        }
    }
    (DEFAULT_OFFSET_X0, DEFAULT_OFFSET_Y_PX)
}

struct ImagePosition {
    ts_us: u16,
    x: f32,
    y: f32,
    z: f32,
    edge_touch: bool,
}

/// Pixel position + radius -> 3-D ball position, `shot-data.md` §6.5.
fn compute_position(hit: &RawHit, off_x0: f32, off_y_px: f32) -> ImagePosition {
    let f = focal_px();
    let r = hit.radius;
    let z = BALL_DIAMETER_M * f / (2.0 * r);
    let sign = if hit.view == 0 { 1.0 } else { -1.0 };
    let off_x = ((z - 0.25) * 0.02 + off_x0) * sign * 0.5;
    let x = (hit.cx_abs - IMAGE_CENTER_X) * BALL_DIAMETER_M / (2.0 * r) + off_x;
    let off_y = if hit.view == 0 { off_y_px } else { 0.0 };
    let y = (hit.cy_abs + off_y - IMAGE_CENTER_Y) * BALL_DIAMETER_M / (2.0 * r);
    ImagePosition {
        ts_us: hit.ts_us,
        x,
        y,
        z,
        edge_touch: hit.edge_touch,
    }
}

struct Velocity {
    speed_mps: f32,
    launch_angle_deg: f32,
    horizontal_angle_deg: f32,
}

/// First acceptable image pair -> speed/launch/horizontal angle,
/// `shot-data.md` §6.6.
fn compute_velocity(positions: &[ImagePosition], right_handed: bool) -> Option<Velocity> {
    for i in 0..positions.len() {
        for j in (i + 1)..positions.len() {
            let a = &positions[i];
            let b = &positions[j];
            if a.edge_touch || b.edge_touch {
                continue;
            }
            if a.ts_us == b.ts_us {
                continue; // simultaneous stereo pair, not a velocity sample
            }
            let dt = (b.ts_us as i32 - a.ts_us as i32) as f32 / 1_000_000.0;
            if dt <= 0.0 {
                continue; // arrival order should match timestamp order
            }
            let vx = (b.x - a.x) / dt;
            let mut vy = -(b.y - a.y) / dt;
            let mut vz = -(b.z - a.z) / dt;
            if !right_handed {
                vy = -vy;
                vz = -vz;
            }
            let la = vy.atan2(vx).to_degrees();
            if la > -5.0 && vx > 0.0 {
                let speed = (vx * vx + vy * vy + vz * vz).sqrt();
                let ha = vz.atan2(vx).to_degrees();
                return Some(Velocity {
                    speed_mps: speed,
                    launch_angle_deg: la,
                    horizontal_angle_deg: ha,
                });
            }
        }
    }
    None
}

/// Decodes a complete shot from its decrypted-payload-ready ciphertexts.
/// `header_ciphertext` / `image_ciphertexts` are each a packet's payload
/// with the 8-byte `[magic][len]` prefix already stripped (still
/// AES-encrypted). Returns `None` if the ball couldn't be found in enough
/// images to compute a velocity -- callers should fall back to
/// `Event::Misread` in that case.
pub fn decode_shot(
    image_ciphertexts: &[Vec<u8>],
    aes_key: &[u8; 16],
    right_handed: bool,
    tilt_correction_deg: Option<f32>,
) -> Option<BallData> {
    let mut hits = Vec::new();
    for ct in image_ciphertexts {
        let plain = aes128_ecb_decrypt(aes_key, ct);
        let Some(img) = parse_shot_image(&plain) else {
            continue;
        };
        let Some(hit) = detect_ball(&img) else {
            continue;
        };
        hits.push(raw_hit(&img, &hit));
    }
    let (off_x0, off_y_px) = estimate_stereo_calibration(&hits);
    let positions: Vec<ImagePosition> = hits
        .iter()
        .map(|h| compute_position(h, off_x0, off_y_px))
        .collect();
    let est = compute_velocity(&positions, right_handed)?;
    // shot-data.md §6.6/§9: 7x5 mph correction table applied after the base
    // speed/angle estimate, keyed by the uncorrected speed and horizontal
    // angle.
    let speed_mph = est.speed_mps * 2.236_936;
    let correction_mps = speed_correction_mph(speed_mph, est.horizontal_angle_deg) * 0.44704;
    let corrected_speed_mps = est.speed_mps + correction_mps;
    let launch_angle_deg = est.launch_angle_deg + tilt_correction_deg.unwrap_or(0.0);
    Some(BallData {
        speed_mps: corrected_speed_mps,
        // Vendor's own detection algorithm (Hough transform, recovered
        // kernels), but not validated against the vendor SDK's own output
        // -- see module docs. Capped below `CERTAIN` on purpose.
        speed_conf: Confidence(0.5),
        launch_angle_deg,
        launch_angle_conf: Confidence(0.5),
        horizontal_angle_deg: est.horizontal_angle_deg,
        horizontal_angle_conf: Confidence(0.5),
        total_spin_rpm: None,
        back_spin_rpm: None,
        side_spin_rpm: None,
        spin_axis_deg: None,
        spin_conf: Confidence(0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real shot header captured from hardware (2026-09-09), firmware 1.7,
    /// decrypted with `shot_aes_key(1.7)`: image count A=1, B=2 (3 images
    /// total), no trailing trigger packet.
    #[test]
    fn parses_real_shot_header() {
        let key = crate::wire::shot_aes_key(1.7);
        let ciphertext =
            hex::decode("d1c11d55d29b493fc72db95de5c768b3f4a8a5b95699961bd1fe2d1eba7df48f")
                .unwrap();
        let plain = aes128_ecb_decrypt(&key, &ciphertext);
        let hdr = parse_shot_header(&plain).unwrap();
        assert_eq!(hdr.expected_images, 3);
        assert!(!hdr.trigger_follows);
    }

    #[test]
    fn rejects_short_header() {
        assert!(parse_shot_header(&[0u8; 4]).is_none());
    }
}
