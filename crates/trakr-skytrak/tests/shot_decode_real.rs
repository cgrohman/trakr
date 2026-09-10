//! Regression test against real hardware capture (2026-09-09, firmware
//! 1.7000, original SkyTrak unit). Fixtures are the two clean (non-binned)
//! image packets from one real chip shot, decrypted-payload ciphertext with
//! the `[magic][len]` prefix already stripped -- see
//! `crates/trakr-skytrak/tests/fixtures/real_shot_1/`.
//!
//! This doesn't validate against the vendor SDK's own numbers (we don't have
//! those), only that the decoder produces a physically plausible shot from
//! real sensor data -- ball speed on the order of a chip/wedge shot, a
//! positive launch angle, and a small horizontal angle.

use trakr_skytrak::shot_decode::decode_shot;
use trakr_skytrak::wire::shot_aes_key;

#[test]
fn decodes_a_real_captured_shot_plausibly() {
    let view0 = include_bytes!("fixtures/real_shot_1/image_view0.bin").to_vec();
    let view1 = include_bytes!("fixtures/real_shot_1/image_view1.bin").to_vec();
    let key = shot_aes_key(1.7);

    let ball =
        decode_shot(&[view0, view1], &key, true, None).expect("should decode a ball position");

    assert!(
        (20.0..45.0).contains(&ball.speed_mps),
        "speed {} m/s outside plausible chip/wedge range",
        ball.speed_mps
    );
    assert!(
        (10.0..50.0).contains(&ball.launch_angle_deg),
        "launch angle {} deg outside plausible range",
        ball.launch_angle_deg
    );
    assert!(
        ball.horizontal_angle_deg.abs() < 15.0,
        "horizontal angle {} deg outside plausible range",
        ball.horizontal_angle_deg
    );
    assert!(ball.total_spin_rpm.is_none(), "spin is not implemented yet");
}
