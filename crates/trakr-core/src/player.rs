//! Player profiles: a name, handedness, and a "bag" of clubs with the carry
//! distance (yards) that player hits each one. Reference data by default --
//! `POST /v1/session/shot {"club": ..., "player": ...}` (see trakr-daemon)
//! is the one place it drives behavior, turning a club + carry distance into
//! a plausible synthetic shot instead of requiring five hand-typed numbers.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClubCarry {
    /// Free-form code, e.g. "DR", "3W", "7I", "PW", "SW", "LW" -- matches
    /// what simulators report in a Player response (see trakr-openconnect's
    /// `wire::Player::club`). Matched case-insensitively.
    pub club: String,
    pub carry_yd: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Player {
    /// Display name; also the id. Matched case-insensitively.
    pub name: String,
    #[serde(default = "default_right_handed")]
    pub right_handed: bool,
    #[serde(default)]
    pub bag: Vec<ClubCarry>,
}

fn default_right_handed() -> bool {
    true
}

impl Player {
    pub fn carry_for(&self, club: &str) -> Option<f32> {
        self.bag
            .iter()
            .find(|c| c.club.eq_ignore_ascii_case(club))
            .map(|c| c.carry_yd)
    }
}

/// Typical launch conditions for a club, used to turn a desired carry
/// distance into plausible ball-flight numbers for a synthetic shot. These
/// are rough average-golfer reference points, not a physics model: ball
/// speed is scaled from the reference by `sqrt(desired_carry / carry_yd)`
/// (carry grows roughly with the square of speed absent drag), while launch
/// angle and spin are kept fixed. Good enough for exercising trakr end to
/// end -- don't mistake the output for real ballistics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ClubProfile {
    pub club: &'static str,
    pub carry_yd: f32,
    pub ball_speed_mph: f32,
    pub vla_deg: f32,
    pub spin_rpm: f32,
}

pub const CLUB_PROFILES: &[ClubProfile] = &[
    ClubProfile { club: "DR", carry_yd: 214.0, ball_speed_mph: 140.0, vla_deg: 10.9, spin_rpm: 2686.0 },
    ClubProfile { club: "3W", carry_yd: 195.0, ball_speed_mph: 130.0, vla_deg: 9.2, spin_rpm: 3655.0 },
    ClubProfile { club: "5W", carry_yd: 180.0, ball_speed_mph: 124.0, vla_deg: 9.4, spin_rpm: 4350.0 },
    ClubProfile { club: "3H", carry_yd: 170.0, ball_speed_mph: 121.0, vla_deg: 10.2, spin_rpm: 4587.0 },
    ClubProfile { club: "3I", carry_yd: 170.0, ball_speed_mph: 118.0, vla_deg: 9.4, spin_rpm: 4360.0 },
    ClubProfile { club: "4I", carry_yd: 160.0, ball_speed_mph: 116.0, vla_deg: 10.0, spin_rpm: 4500.0 },
    ClubProfile { club: "5I", carry_yd: 155.0, ball_speed_mph: 115.0, vla_deg: 10.1, spin_rpm: 4700.0 },
    ClubProfile { club: "6I", carry_yd: 150.0, ball_speed_mph: 112.0, vla_deg: 11.5, spin_rpm: 5100.0 },
    ClubProfile { club: "7I", carry_yd: 140.0, ball_speed_mph: 109.0, vla_deg: 13.6, spin_rpm: 6200.0 },
    ClubProfile { club: "8I", carry_yd: 130.0, ball_speed_mph: 106.0, vla_deg: 15.8, spin_rpm: 6800.0 },
    ClubProfile { club: "9I", carry_yd: 115.0, ball_speed_mph: 101.0, vla_deg: 18.1, spin_rpm: 7500.0 },
    ClubProfile { club: "PW", carry_yd: 105.0, ball_speed_mph: 96.0, vla_deg: 20.5, spin_rpm: 8300.0 },
    ClubProfile { club: "GW", carry_yd: 95.0, ball_speed_mph: 90.0, vla_deg: 23.3, spin_rpm: 8700.0 },
    ClubProfile { club: "SW", carry_yd: 85.0, ball_speed_mph: 85.0, vla_deg: 25.4, spin_rpm: 9200.0 },
    ClubProfile { club: "LW", carry_yd: 65.0, ball_speed_mph: 74.0, vla_deg: 30.0, spin_rpm: 10000.0 },
];

pub fn club_profile(club: &str) -> Option<&'static ClubProfile> {
    CLUB_PROFILES.iter().find(|p| p.club.eq_ignore_ascii_case(club))
}

/// `(ball_speed_mph, vla_deg, spin_rpm)` for a plausible shot with `club`
/// carrying `desired_carry_yd`. `None` if `club` isn't a known code (see
/// [`CLUB_PROFILES`]).
pub fn ball_data_for_carry(club: &str, desired_carry_yd: f32) -> Option<(f32, f32, f32)> {
    let p = club_profile(club)?;
    let speed = p.ball_speed_mph * (desired_carry_yd / p.carry_yd).sqrt();
    Some((speed, p.vla_deg, p.spin_rpm))
}
