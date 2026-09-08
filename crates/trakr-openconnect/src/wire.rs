//! JSON shapes from https://gsprogolf.com/GSProConnectV1.html
use serde::{Deserialize, Serialize};
use trakr_core::Shot;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Request {
    #[serde(rename = "DeviceID")]
    pub device_id: String,
    pub units: String,
    pub shot_number: u32,
    #[serde(rename = "APIversion")]
    pub api_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ball_data: Option<BallData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub club_data: Option<ClubData>,
    pub shot_data_options: ShotDataOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct BallData {
    pub speed: f64,
    pub spin_axis: f64,
    pub total_spin: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub back_spin: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side_spin: Option<f64>,
    #[serde(rename = "HLA")]
    pub hla: f64,
    #[serde(rename = "VLA")]
    pub vla: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carry_distance: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ClubData {
    pub speed: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub angle_of_attack: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face_to_target: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lie: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loft: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ShotDataOptions {
    pub contains_ball_data: bool,
    pub contains_club_data: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_monitor_is_ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_monitor_ball_detected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_heart_beat: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Response {
    pub code: u16,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub player: Option<Player>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Player {
    #[serde(default)]
    pub handed: Option<String>,
    #[serde(default)]
    pub club: Option<String>,
    #[serde(default)]
    pub distance_to_target: Option<f32>,
    #[serde(default)]
    pub surface: Option<String>,
}

impl Request {
    pub fn from_shot(device_id: &str, shot_number: u32, shot: &Shot) -> Self {
        let mut ball = shot.ball.clone();
        ball.complete_spin();
        let club = shot.club.as_ref().and_then(|c| {
            c.speed_mps.map(|s| ClubData {
                speed: round1(s * 2.236_936),
                angle_of_attack: c.attack_angle_deg.map(round1),
                face_to_target: c.face_to_target_deg.map(round1),
                lie: c.lie_deg.map(round1),
                loft: c.dynamic_loft_deg.map(round1),
                path: c.path_deg.map(round1),
            })
        });
        Request {
            device_id: device_id.into(),
            units: "Yards".into(),
            shot_number,
            api_version: "1".into(),
            ball_data: Some(BallData {
                speed: round1(ball.speed_mph()),
                spin_axis: round1(ball.spin_axis_deg.unwrap_or(0.0)),
                total_spin: ball.total_spin_rpm.unwrap_or(0.0).round() as f64,
                back_spin: ball.back_spin_rpm.map(|v| v.round() as f64),
                side_spin: ball.side_spin_rpm.map(|v| v.round() as f64),
                hla: round1(ball.horizontal_angle_deg),
                vla: round1(ball.launch_angle_deg),
                carry_distance: shot.flight.as_ref().and_then(|f| f.carry_m).map(|m| round1(m * 1.093_613)),
            }),
            shot_data_options: ShotDataOptions {
                contains_ball_data: true,
                contains_club_data: club.is_some(),
                launch_monitor_is_ready: Some(true),
                launch_monitor_ball_detected: Some(true),
                is_heart_beat: Some(false),
            },
            club_data: club,
        }
    }
}

fn round1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

/// GSPro sometimes sends two JSON objects back to back in one TCP segment.
/// Split on balanced braces and parse each; keep any trailing partial object.
pub fn drain_responses(acc: &mut Vec<u8>) -> Vec<Response> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start: Option<usize> = None;
    let mut consumed = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in acc.iter().enumerate() {
        if in_str {
            if esc { esc = false; } else if b == b'\\' { esc = true; } else if b == b'"' { in_str = false; }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => { if depth == 0 { start = Some(i); } depth += 1; }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start.take() {
                        if let Ok(r) = serde_json::from_slice::<Response>(&acc[s..=i]) { out.push(r); }
                        consumed = i + 1;
                    }
                }
            }
            _ => {}
        }
    }
    acc.drain(..consumed);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_concatenated_responses() {
        let mut acc = br#"{"Code":201,"Message":"GSPro Player Information","Player":{"Handed":"RH","Club":"DR"}}{"Code":200,"Message":"Shot received successfully"}{"Code":20"#.to_vec();
        let r = drain_responses(&mut acc);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].player.as_ref().unwrap().club.as_deref(), Some("DR"));
        assert_eq!(acc, br#"{"Code":20"#);
    }

    #[test]
    fn shot_request_matches_spec_shape() {
        let shot = Shot {
            sequence: 1,
            ball: trakr_core::BallData { speed_mps: 65.9, launch_angle_deg: 14.3, horizontal_angle_deg: 2.3, total_spin_rpm: Some(3250.0), spin_axis_deg: Some(-13.2), ..Default::default() },
            club: None, flight: None, valid: true,
        };
        let req = Request::from_shot("trakr", 13, &shot);
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["DeviceID"], "trakr");
        assert_eq!(v["APIversion"], "1");
        assert_eq!(v["ShotNumber"], 13);
        assert_eq!(v["BallData"]["HLA"], 2.3);
        assert_eq!(v["BallData"]["Speed"], 147.4);
        assert_eq!(v["ShotDataOptions"]["ContainsBallData"], true);
        assert_eq!(v["ShotDataOptions"]["IsHeartBeat"], false);
        assert!(v.get("ClubData").is_none());
    }
}
