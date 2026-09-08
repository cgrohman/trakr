//! Byte-level protocol: framing, CRC, packet builders and parsers.
//! Every constant here is confirmed against a real original SkyTrak unit —
//! see docs/skytrak-protocol/{discovery,session,framing-and-crypto}.md.

use std::collections::HashMap;

pub const UDP_PORT: u16 = 5023;
pub const TCP_PORT: u16 = 5024;

pub const MAGIC_DISCOVER_REQ: u32 = 0xAAAAEEEE;
pub const MAGIC_CONN_CONFIRM: u32 = 0xAAAAFFFF;
pub const MAGIC_STATUS: u32 = 0xBBBBBBBB; // box -> host, also the discovery/confirm reply
pub const MAGIC_PARAMS: u32 = 0xAAAAAAAA; // box -> host
pub const MAGIC_SYS_CONFIG: u32 = 0xAAAABBBB; // host -> box
pub const MAGIC_CAM_CONFIG: u32 = 0xAAAACCCC; // host -> box
pub const MAGIC_ARM: u32 = 0xBBBBCCCC; // host -> box (also "host ready")
pub const MAGIC_DISARM: u32 = 0xBBBBFFFF; // host -> box
pub const MAGIC_SHOT_HEADER: u32 = 0xCCCCCCCC; // box -> host
pub const MAGIC_SHOT_IMAGE: u32 = 0xDDDDDDDD; // box -> host
pub const MAGIC_TRIGGER: u32 = 0xEEEEEEEE; // box -> host

/// CRC-32/MPEG-2 (poly 0x04C11DB7, init 0xFFFFFFFF, no reflect, no xorout)
/// fed the packet's little-endian u32 words in big-endian byte order — this
/// is what the box's STM32 hardware CRC unit produces. Covers `pkt[..pkt.len()-4]`.
pub fn skytrak_crc(pkt: &[u8]) -> u32 {
    debug_assert_eq!(
        pkt.len() % 4,
        0,
        "CRC input must be a whole number of u32 words"
    );
    let mut crc: u32 = 0xFFFFFFFF;
    let (chunks, _remainder) = pkt.as_chunks::<4>();
    for word in chunks {
        for &b in &[word[3], word[2], word[1], word[0]] {
            crc = (crc << 8) ^ CRC_TABLE[((crc >> 24) ^ b as u32) as usize & 0xFF];
        }
    }
    crc
}

static CRC_TABLE: [u32; 256] = build_crc_table();

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u32) << 24;
        let mut j = 0;
        while j < 8 {
            c = if c & 0x8000_0000 != 0 {
                (c << 1) ^ 0x04C1_1DB7
            } else {
                c << 1
            };
            j += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

/// A 12-byte control packet: `{magic, 0x0C, crc_or_zero}`.
fn control12(magic: u32, with_crc: bool) -> [u8; 12] {
    let mut buf = [0u8; 12];
    buf[0..4].copy_from_slice(&magic.to_le_bytes());
    buf[4..8].copy_from_slice(&0x0Cu32.to_le_bytes());
    if with_crc {
        let crc = skytrak_crc(&buf[..8]);
        buf[8..12].copy_from_slice(&crc.to_le_bytes());
    }
    buf
}

pub fn discovery_request() -> [u8; 12] {
    control12(MAGIC_DISCOVER_REQ, false)
}

pub fn connection_confirm() -> [u8; 12] {
    control12(MAGIC_CONN_CONFIRM, false)
}

pub fn arm_packet() -> [u8; 12] {
    control12(MAGIC_ARM, true)
}

pub fn disarm_packet() -> [u8; 12] {
    // The vendor SDK leaves this CRC stale; we compute it correctly anyway
    // per docs/skytrak-protocol/session.md §6 — costs nothing, the box accepts it.
    control12(MAGIC_DISARM, true)
}

/// SYS_CONFIG, 0x6C bytes. `initial` sets the "first config after connect" flag.
pub fn sys_config(
    initial: bool,
    right_handed: bool,
    putting: bool,
    reference_laser: bool,
) -> [u8; 0x6C] {
    let mut buf = [0u8; 0x6C];
    buf[0x00..0x04].copy_from_slice(&MAGIC_SYS_CONFIG.to_le_bytes());
    buf[0x04..0x08].copy_from_slice(&0x6Cu32.to_le_bytes());
    buf[0x14..0x18].copy_from_slice(&(if right_handed { 0i32 } else { 1 }).to_le_bytes());
    buf[0x44..0x48].copy_from_slice(&2i32.to_le_bytes()); // host interface: WiFi/Ethernet
    buf[0x4C..0x50].copy_from_slice(&(reference_laser as i32).to_le_bytes());
    buf[0x54..0x58].copy_from_slice(&(if putting { 2i32 } else { 1 }).to_le_bytes());
    buf[0x5C..0x60].copy_from_slice(&(initial as i32).to_le_bytes());
    buf[0x64..0x68].copy_from_slice(&2.0f32.to_le_bytes()); // protocol version
    let crc = skytrak_crc(&buf[..0x68]);
    buf[0x68..0x6C].copy_from_slice(&crc.to_le_bytes());
    buf
}

/// CAM_CONFIG, 0x84 bytes. Default timing values from `RIPEInit`; a full
/// reimplementation would recompute the exposure pairs from the box's params
/// packet (`docs/skytrak-protocol/session.md` §7.4) but the defaults are
/// accepted by the box for the handshake.
pub fn cam_config(right_handed: bool) -> [u8; 0x84] {
    let mut buf = [0u8; 0x84];
    buf[0x00..0x04].copy_from_slice(&MAGIC_CAM_CONFIG.to_le_bytes());
    buf[0x04..0x08].copy_from_slice(&0x84u32.to_le_bytes());
    buf[0x08..0x0C].copy_from_slice(&3i32.to_le_bytes());
    buf[0x10..0x14].copy_from_slice(&1i32.to_le_bytes());
    buf[0x20..0x24].copy_from_slice(&1i32.to_le_bytes());
    buf[0x24..0x28].copy_from_slice(&0xFAi32.to_le_bytes());
    buf[0x28..0x2C].copy_from_slice(&1i32.to_le_bytes());
    buf[0x2C..0x30].copy_from_slice(&0xFAi32.to_le_bytes());
    buf[0x30..0x34].copy_from_slice(&0x2Di32.to_le_bytes());
    buf[0x34..0x38].copy_from_slice(&0x2Di32.to_le_bytes());
    for off in [0x38, 0x3C, 0x40, 0x44] {
        buf[off..off + 4].copy_from_slice(&0x80i32.to_le_bytes());
    }
    let (a, b, c, d): (i32, i32, i32, i32) = if right_handed {
        (0x14, 0x16, 0x16, 0x14)
    } else {
        (0x16, 0x14, 0x14, 0x16)
    };
    buf[0x48..0x4C].copy_from_slice(&a.to_le_bytes());
    buf[0x4C..0x50].copy_from_slice(&b.to_le_bytes());
    buf[0x50..0x54].copy_from_slice(&c.to_le_bytes());
    buf[0x54..0x58].copy_from_slice(&d.to_le_bytes());
    buf[0x58..0x5C].copy_from_slice(&1i32.to_le_bytes());
    buf[0x5C..0x60].copy_from_slice(&1i32.to_le_bytes());
    buf[0x60..0x64].copy_from_slice(&0xFAi32.to_le_bytes());
    let crc = skytrak_crc(&buf[..0x80]);
    buf[0x80..0x84].copy_from_slice(&crc.to_le_bytes());
    buf
}

/// Decoded fields of a `0xBBBBBBBB` status packet (also the discovery/UDP
/// confirm reply, truncated to what that path reads).
#[derive(Debug, Clone, PartialEq)]
pub struct StatusPacket {
    pub code: i32,
    pub battery_pct: f32,
    pub charging: bool,
    pub rssi: Option<i32>,
    pub box_name: String,
    pub handedness_right: bool,
    /// 0 = network mode, 1 = direct (box-as-AP) mode, else unknown.
    pub connection_mode: i32,
}

pub fn parse_status(pkt: &[u8]) -> Option<StatusPacket> {
    if pkt.len() < 0x90 || u32::from_le_bytes(pkt[0..4].try_into().ok()?) != MAGIC_STATUS {
        return None;
    }
    let f32_at = |o: usize| f32::from_le_bytes(pkt[o..o + 4].try_into().unwrap());
    let i32_at = |o: usize| i32::from_le_bytes(pkt[o..o + 4].try_into().unwrap());
    let cstr =
        |s: &[u8]| String::from_utf8_lossy(s.split(|&b| b == 0).next().unwrap_or(&[])).into_owned();
    let rssi = cstr(&pkt[0x48..0x50]).trim().parse::<i32>().ok();
    Some(StatusPacket {
        code: i32_at(0x28),
        battery_pct: f32_at(0x1C),
        charging: i32_at(0x24) != 0,
        rssi,
        box_name: cstr(&pkt[0x50..0x70]),
        handedness_right: i32_at(0x38) == 0,
        connection_mode: i32_at(0x88),
    })
}

/// Fields read from a `0xAAAAAAAA` params packet.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamsPacket {
    pub firmware_version: f32,
    pub serial: String,
    pub ap_mode: bool,
}

pub fn parse_params(pkt: &[u8]) -> Option<ParamsPacket> {
    if pkt.len() < 0x170 || u32::from_le_bytes(pkt[0..4].try_into().ok()?) != MAGIC_PARAMS {
        return None;
    }
    let f32_at = |o: usize| f32::from_le_bytes(pkt[o..o + 4].try_into().unwrap());
    let i32_at = |o: usize| i32::from_le_bytes(pkt[o..o + 4].try_into().unwrap());
    let serial =
        String::from_utf8_lossy(pkt[0x164..0x170].split(|&b| b == 0).next().unwrap_or(&[]))
            .into_owned();
    Some(ParamsPacket {
        firmware_version: f32_at(0x14),
        serial,
        ap_mode: i32_at(0x2C) == 1,
    })
}

/// Incrementally reassembles length-prefixed packets from a TCP byte stream.
/// Framing: `u32 magic; u32 total_len_incl_header; ...`. No CRC check on
/// receive (the vendor SDK doesn't check inbound CRCs either).
#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
}

impl Framer {
    pub fn feed(&mut self, data: &[u8]) -> Vec<(u32, Vec<u8>)> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            if self.buf.len() < 8 {
                break;
            }
            let magic = u32::from_le_bytes(self.buf[0..4].try_into().unwrap());
            let len = u32::from_le_bytes(self.buf[4..8].try_into().unwrap()) as usize;
            if !(8..=0x20000).contains(&len) || self.buf.len() < len {
                break;
            }
            let pkt: Vec<u8> = self.buf.drain(..len).collect();
            out.push((magic, pkt));
        }
        out
    }
}

/// AES key selection by firmware version, per `docs/skytrak-protocol/framing-and-crypto.md`.
/// Shot images are AES-128-ECB encrypted with one of these fixed keys.
pub fn shot_aes_key(firmware_version: f32) -> [u8; 16] {
    if firmware_version < 1.2 {
        [0u8; 16]
    } else if firmware_version < 2.0 {
        let mut k = [0u8; 16];
        for (i, b) in k.iter_mut().enumerate() {
            *b = i as u8;
        }
        k
    } else {
        [
            0x42, 0x53, 0x89, 0x18, 0x77, 0x42, 0x87, 0x05, 0x81, 0x58, 0x55, 0x36, 0x86, 0x58,
            0x61, 0x08,
        ]
    }
}

pub fn magic_name(magic: u32) -> &'static str {
    static NAMES: &[(u32, &str)] = &[
        (MAGIC_STATUS, "status"),
        (MAGIC_PARAMS, "params"),
        (MAGIC_SYS_CONFIG, "sys_config"),
        (MAGIC_CAM_CONFIG, "cam_config"),
        (MAGIC_ARM, "arm"),
        (MAGIC_DISARM, "disarm"),
        (MAGIC_SHOT_HEADER, "shot_header"),
        (MAGIC_SHOT_IMAGE, "shot_image"),
        (MAGIC_TRIGGER, "trigger"),
    ];
    static LOOKUP: std::sync::OnceLock<HashMap<u32, &'static str>> = std::sync::OnceLock::new();
    LOOKUP
        .get_or_init(|| NAMES.iter().copied().collect())
        .get(&magic)
        .copied()
        .unwrap_or("unknown")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_captured_arm_packet() {
        // Captured live 2026-09-08 from a real unit: the box accepted this
        // exact ARM packet and armed, so this CRC is ground truth, not just
        // a documented guess.
        let pkt = control12(MAGIC_ARM, false);
        let crc = skytrak_crc(&pkt[..8]);
        assert_eq!(crc.to_le_bytes(), [0xF1, 0xE6, 0x65, 0xE4]);
    }

    #[test]
    fn crc_matches_captured_disarm_and_confirm_packets() {
        let disarm_crc = skytrak_crc(&control12(MAGIC_DISARM, false)[..8]);
        assert_eq!(disarm_crc.to_le_bytes(), [0xA3, 0x15, 0xBE, 0x4C]);
        let confirm_crc = skytrak_crc(&control12(MAGIC_CONN_CONFIRM, false)[..8]);
        assert_eq!(confirm_crc.to_le_bytes(), [0x7F, 0xF1, 0xE2, 0x13]);
    }

    #[test]
    fn parses_real_discovery_reply() {
        let hex = "bbbbbbbba00000000000000000000000000000006d66e041b6f385400000be4200000000010000000000000001000000000000000000000000000000020000000000000000000000\
2d34390000000000534b595452414b5f4334374635313930324545330000000000000000000000001400000001000000000000000000000000000000020000000000000000000000000000000000000000000000696875b5";
        let pkt = hex::decode(hex).unwrap();
        let s = parse_status(&pkt).expect("should parse as a status/reply packet");
        assert_eq!(s.code, 0); // there is no status+0x28 semantics at discovery time; field just happens to read 0 here
        assert_eq!(s.battery_pct, 95.0);
        assert_eq!(s.rssi, Some(-49));
        assert_eq!(s.box_name, "SKYTRAK_C47F51902EE3");
        assert_eq!(s.connection_mode, 0);
    }

    #[test]
    fn framer_splits_back_to_back_packets() {
        let mut f = Framer::default();
        let mut data = disarm_packet().to_vec();
        data.extend_from_slice(&arm_packet());
        data.push(0xAA); // partial next packet
        let pkts = f.feed(&data);
        assert_eq!(pkts.len(), 2);
        assert_eq!(pkts[0].0, MAGIC_DISARM);
        assert_eq!(pkts[1].0, MAGIC_ARM);
    }
}
