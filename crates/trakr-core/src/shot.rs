use serde::{Deserialize, Serialize};

/// Per-metric confidence in `[0.0, 1.0]`; `1.0` means the device is certain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Confidence(pub f32);

impl Confidence {
    pub const CERTAIN: Confidence = Confidence(1.0);
}

/// Measured ball launch conditions. SI units throughout: m/s, degrees, rpm.
/// Sign conventions (right-handed golfer's view, target is +y):
/// * `horizontal_angle`: positive = right of target line.
/// * `spin_axis`: positive = tilted right (fade/slice for a righty).
/// * `side_spin`: positive = right.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BallData {
    pub speed_mps: f32,
    pub speed_conf: Confidence,
    pub launch_angle_deg: f32,
    pub launch_angle_conf: Confidence,
    pub horizontal_angle_deg: f32,
    pub horizontal_angle_conf: Confidence,
    pub total_spin_rpm: Option<f32>,
    pub back_spin_rpm: Option<f32>,
    pub side_spin_rpm: Option<f32>,
    pub spin_axis_deg: Option<f32>,
    pub spin_conf: Confidence,
}

impl BallData {
    pub fn speed_mph(&self) -> f32 {
        self.speed_mps * 2.236_936
    }
    /// Fill total spin and axis from components, or components from total and
    /// axis, whichever pair is present.
    pub fn complete_spin(&mut self) {
        match (
            self.total_spin_rpm,
            self.spin_axis_deg,
            self.back_spin_rpm,
            self.side_spin_rpm,
        ) {
            (Some(t), Some(a), None, None) => {
                let r = a.to_radians();
                self.back_spin_rpm = Some((t * r.cos()).round());
                self.side_spin_rpm = Some((t * r.sin()).round());
            }
            (None, None, Some(b), Some(s)) => {
                self.total_spin_rpm = Some((b * b + s * s).sqrt().round());
                self.spin_axis_deg = Some(if b != 0.0 {
                    s.atan2(b).to_degrees()
                } else {
                    0.0
                });
            }
            _ => {}
        }
    }
}

/// Club delivery data. Optional per device; the original SkyTrak has none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ClubData {
    pub speed_mps: Option<f32>,
    pub attack_angle_deg: Option<f32>,
    pub face_to_target_deg: Option<f32>,
    pub path_deg: Option<f32>,
    pub dynamic_loft_deg: Option<f32>,
    pub lie_deg: Option<f32>,
}

/// Flight the *device* predicted. Simulators run their own physics, so this
/// is informational only. Metres and seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FlightEstimate {
    pub carry_m: Option<f32>,
    pub offline_m: Option<f32>,
    pub apex_m: Option<f32>,
    pub duration_s: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    /// Monotonic per-session counter assigned by the driver.
    pub sequence: u32,
    pub ball: BallData,
    pub club: Option<ClubData>,
    pub flight: Option<FlightEstimate>,
    /// Driver's judgement that the read is usable.
    pub valid: bool,
}
