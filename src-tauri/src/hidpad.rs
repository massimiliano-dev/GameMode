//! Pads that aren't XInput — DualSense, DualShock 4, Switch Pro — read
//! straight from their HID input reports. Windows.Gaming.Input only hands
//! input to the focused window, so it couldn't summon the overlay during a
//! game; raw HID works in the background like XInput does, and the device
//! stays shared with the game. Report layouts follow SDL's hidapi drivers
//! and the Linux hid-playstation / hid-nintendo drivers.
//!
//! Parsing is pure and unit-tested; the hidapi side lives in `imp` and is
//! driven from the XInput polling thread in `gamepad.rs`, which ORs these
//! bits into the same bitmask so Guide, Back+Start, LB+RB and the store
//! assist behave the same on every pad.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

/// Button layout family, for the glyphs the UI shows.
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Xbox,
    PlayStation,
    Nintendo,
}

/// Button bits: XInput's `wButtons` layout plus the virtual trigger bits,
/// identical to the constants in `gamepad.rs` so both backends can be ORed.
pub mod bits {
    pub const DPAD_UP: u32 = 0x0001;
    pub const DPAD_DOWN: u32 = 0x0002;
    pub const DPAD_LEFT: u32 = 0x0004;
    pub const DPAD_RIGHT: u32 = 0x0008;
    pub const START: u32 = 0x0010;
    pub const BACK: u32 = 0x0020;
    pub const LB: u32 = 0x0100;
    pub const RB: u32 = 0x0200;
    pub const GUIDE: u32 = 0x0400;
    pub const A: u32 = 0x1000;
    pub const B: u32 = 0x2000;
    pub const X: u32 = 0x4000;
    pub const Y: u32 = 0x8000;
    pub const LT: u32 = 0x1_0000;
    pub const RT: u32 = 0x2_0000;
}
use bits::*;

/// The HID backend can be switched off when Steam Input or DS4Windows
/// already turn these pads into XInput ones.
static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub const VID_SONY: u16 = 0x054C;
pub const VID_NINTENDO: u16 = 0x057E;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Model {
    DualSense,
    DualShock4,
    SwitchPro,
}

impl Model {
    /// Known pads by vendor/product id. Generic HID pads have no report
    /// table and are left to the browser's Gamepad API (foreground only).
    pub fn from_ids(vid: u16, pid: u16) -> Option<Model> {
        match (vid, pid) {
            // DualSense, DualSense Edge.
            (VID_SONY, 0x0CE6) | (VID_SONY, 0x0DF2) => Some(Model::DualSense),
            // DualShock 4 v1, v2.
            (VID_SONY, 0x05C4) | (VID_SONY, 0x09CC) => Some(Model::DualShock4),
            (VID_NINTENDO, 0x2009) => Some(Model::SwitchPro),
            _ => None,
        }
    }

    pub fn kind(self) -> Kind {
        match self {
            Model::DualSense | Model::DualShock4 => Kind::PlayStation,
            Model::SwitchPro => Kind::Nintendo,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Model::DualSense => "DualSense",
            Model::DualShock4 => "DualShock 4",
            Model::SwitchPro => "Pro Controller",
        }
    }
}

/// What one input report says.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Reading {
    pub buttons: u32,
    /// 0 empty … 3 full, when the report carries the battery.
    pub level: Option<u8>,
    pub charging: Option<bool>,
}

/// Sony/Nintendo hat switch: 0 = north, clockwise, 8 = released.
fn hat_bits(hat: u8) -> u32 {
    match hat & 0x0F {
        0 => DPAD_UP,
        1 => DPAD_UP | DPAD_RIGHT,
        2 => DPAD_RIGHT,
        3 => DPAD_DOWN | DPAD_RIGHT,
        4 => DPAD_DOWN,
        5 => DPAD_DOWN | DPAD_LEFT,
        6 => DPAD_LEFT,
        7 => DPAD_UP | DPAD_LEFT,
        _ => 0,
    }
}

/// Left stick as d-pad. `up_is_high`: Nintendo reports up as a large Y,
/// Sony as a small one.
fn stick_bits(x: i32, y: i32, center: i32, threshold: i32, up_is_high: bool) -> u32 {
    let mut bits = 0;
    if x < center - threshold {
        bits |= DPAD_LEFT;
    }
    if x > center + threshold {
        bits |= DPAD_RIGHT;
    }
    let (up, down) = if up_is_high { (y > center + threshold, y < center - threshold) } else { (y < center - threshold, y > center + threshold) };
    if up {
        bits |= DPAD_UP;
    }
    if down {
        bits |= DPAD_DOWN;
    }
    bits
}

/// 8-bit sticks and triggers: half deflection, like XInput's 16000/32767.
const BYTE_CENTER: i32 = 128;
const BYTE_THRESHOLD: i32 = 64;
const TRIGGER_THRESHOLD: u8 = 128;

/// Sony face/shoulder bytes shared by DualShock 4 and DualSense:
/// `b0` hat + □✕○△, `b1` L1 R1 L2 R2 Share/Create Options L3 R3, `b2` PS.
fn sony_buttons(b0: u8, b1: u8, b2: u8) -> u32 {
    let mut bits = hat_bits(b0);
    if b0 & 0x10 != 0 {
        bits |= X; // Square
    }
    if b0 & 0x20 != 0 {
        bits |= A; // Cross
    }
    if b0 & 0x40 != 0 {
        bits |= B; // Circle
    }
    if b0 & 0x80 != 0 {
        bits |= Y; // Triangle
    }
    if b1 & 0x01 != 0 {
        bits |= LB;
    }
    if b1 & 0x02 != 0 {
        bits |= RB;
    }
    if b1 & 0x04 != 0 {
        bits |= LT;
    }
    if b1 & 0x08 != 0 {
        bits |= RT;
    }
    if b1 & 0x10 != 0 {
        bits |= BACK; // Share / Create
    }
    if b1 & 0x20 != 0 {
        bits |= START; // Options
    }
    if b2 & 0x01 != 0 {
        bits |= GUIDE; // PS
    }
    bits
}

fn sony_sticks(x: u8, y: u8) -> u32 {
    stick_bits(x as i32, y as i32, BYTE_CENTER, BYTE_THRESHOLD, false)
}

fn sony_triggers(l2: u8, r2: u8) -> u32 {
    (if l2 > TRIGGER_THRESHOLD { LT } else { 0 }) | (if r2 > TRIGGER_THRESHOLD { RT } else { 0 })
}

/// 0..=10 (tenths) to the 0..=3 scale XInput uses.
fn tenths_to_level(tenths: u8) -> u8 {
    match tenths {
        0..=1 => 0,
        2..=4 => 1,
        5..=7 => 2,
        _ => 3,
    }
}

/// DualSense: report 0x01 is the full USB layout (64 bytes) or, over
/// Bluetooth before calibration is requested, a 10-byte simple one that
/// Windows pads to 78; 0x31 is the full Bluetooth layout.
fn parse_dualsense(r: &[u8]) -> Option<Reading> {
    let (offset, full) = match (r.first()?, r.len()) {
        (0x01, 10) | (0x01, 78) => (1, false),
        (0x01, n) if n >= 11 => (1, true),
        (0x31, n) if n >= 12 => (2, true),
        _ => return None,
    };
    let o = offset;
    let mut reading = Reading::default();
    if full {
        // x y rx ry z rz seq buttons[3] … status at struct offset 52.
        let mut bits = sony_buttons(r[o + 7], r[o + 8], r[o + 9]);
        bits |= sony_sticks(r[o], r[o + 1]);
        bits |= sony_triggers(r[o + 4], r[o + 5]);
        reading.buttons = bits;
        if let Some(&status) = r.get(o + 52) {
            let tenths = status & 0x0F;
            let state = status >> 4;
            reading.charging = Some(state == 1 || state == 2);
            reading.level = Some(if state == 2 { 3 } else { tenths_to_level(tenths) });
        }
    } else {
        // x y rx ry buttons[3] z rz
        let mut bits = sony_buttons(r[5], r[6], r[7]);
        bits |= sony_sticks(r[1], r[2]);
        if r.len() >= 10 {
            bits |= sony_triggers(r[8], r[9]);
        }
        reading.buttons = bits;
    }
    Some(reading)
}

/// DualShock 4: report 0x01 over USB (64 bytes, battery at 30) shares its
/// first bytes with the Bluetooth simple report; 0x11 is the full
/// Bluetooth layout with two header bytes.
fn parse_dualshock4(r: &[u8]) -> Option<Reading> {
    let o = match r.first()? {
        0x01 if r.len() >= 10 => 1,
        0x11 if r.len() >= 12 => 3,
        _ => return None,
    };
    let mut bits = sony_buttons(r[o + 4], r[o + 5], r[o + 6]);
    bits |= sony_sticks(r[o], r[o + 1]);
    bits |= sony_triggers(r[o + 7], r[o + 8]);
    let mut reading = Reading { buttons: bits, ..Default::default() };
    // The simple Bluetooth report is padded with zeros: no battery there.
    let has_battery = match r[0] {
        0x01 => r.len() == 64,
        _ => r.len() > o + 29,
    };
    if has_battery {
        let status = r[o + 29];
        let cable = status & 0x10 != 0;
        let tenths = (status & 0x0F).min(10);
        reading.charging = Some(cable);
        reading.level = Some(if cable && tenths >= 10 { 3 } else { tenths_to_level(tenths) });
    }
    Some(reading)
}

/// Switch Pro: 0x3F is the simple report the pad sends over Bluetooth
/// before any subcommand; 0x30/0x21 are the full reports after the input
/// mode is set (needed over USB anyway, where the pad is silent until the
/// handshake).
fn parse_switch_pro(r: &[u8]) -> Option<Reading> {
    match r.first()? {
        0x3F if r.len() >= 12 => {
            let b1 = r[1];
            let b2 = r[2];
            let mut bits = hat_bits(r[3]);
            // Positional mapping: B (bottom) confirms like Xbox A.
            if b1 & 0x01 != 0 {
                bits |= A; // B
            }
            if b1 & 0x02 != 0 {
                bits |= B; // A
            }
            if b1 & 0x04 != 0 {
                bits |= X; // Y
            }
            if b1 & 0x08 != 0 {
                bits |= Y; // X
            }
            if b1 & 0x10 != 0 {
                bits |= LB;
            }
            if b1 & 0x20 != 0 {
                bits |= RB;
            }
            if b1 & 0x40 != 0 {
                bits |= LT;
            }
            if b1 & 0x80 != 0 {
                bits |= RT;
            }
            if b2 & 0x01 != 0 {
                bits |= BACK; // Minus
            }
            if b2 & 0x02 != 0 {
                bits |= START; // Plus
            }
            if b2 & 0x10 != 0 {
                bits |= GUIDE; // Home
            }
            let x = u16::from_le_bytes([r[4], r[5]]) as i32;
            let y = u16::from_le_bytes([r[6], r[7]]) as i32;
            bits |= stick_bits(x, y, 0x8000, 16_000, true);
            Some(Reading { buttons: bits, ..Default::default() })
        }
        0x30 | 0x21 if r.len() >= 12 => {
            let right = r[3];
            let shared = r[4];
            let left = r[5];
            let mut bits = 0;
            if right & 0x01 != 0 {
                bits |= X; // Y
            }
            if right & 0x02 != 0 {
                bits |= Y; // X
            }
            if right & 0x04 != 0 {
                bits |= A; // B
            }
            if right & 0x08 != 0 {
                bits |= B; // A
            }
            if right & 0x40 != 0 {
                bits |= RB;
            }
            if right & 0x80 != 0 {
                bits |= RT;
            }
            if shared & 0x01 != 0 {
                bits |= BACK;
            }
            if shared & 0x02 != 0 {
                bits |= START;
            }
            if shared & 0x10 != 0 {
                bits |= GUIDE;
            }
            if left & 0x01 != 0 {
                bits |= DPAD_DOWN;
            }
            if left & 0x02 != 0 {
                bits |= DPAD_UP;
            }
            if left & 0x04 != 0 {
                bits |= DPAD_RIGHT;
            }
            if left & 0x08 != 0 {
                bits |= DPAD_LEFT;
            }
            if left & 0x40 != 0 {
                bits |= LB;
            }
            if left & 0x80 != 0 {
                bits |= LT;
            }
            // Left stick: two 12-bit values packed in three bytes, no
            // calibration applied; the nominal centre is good enough for
            // a half-deflection d-pad.
            let x = (r[6] as i32) | ((r[7] as i32 & 0x0F) << 8);
            let y = ((r[7] as i32) >> 4) | ((r[8] as i32) << 4);
            bits |= stick_bits(x, y, 2048, 800, true);
            let bat = r[2];
            let level = match bat >> 5 {
                0 | 1 => 0,
                2 => 1,
                3 => 2,
                _ => 3,
            };
            Some(Reading { buttons: bits, level: Some(level), charging: Some(bat & 0x10 != 0) })
        }
        _ => None,
    }
}

/// Decodes one input report; `None` for reports that aren't state.
pub fn parse(model: Model, report: &[u8]) -> Option<Reading> {
    match model {
        Model::DualSense => parse_dualsense(report),
        Model::DualShock4 => parse_dualshock4(report),
        Model::SwitchPro => parse_switch_pro(report),
    }
}

/// XInput device interfaces carry `IG_` in their path (SDL's heuristic):
/// such a pad is already read through XInput.
pub fn is_xinput_path(path: &str) -> bool {
    path.to_ascii_lowercase().contains("ig_")
}

#[cfg(windows)]
mod imp {
    use std::ffi::CString;
    use std::time::{Duration, Instant};

    use hidapi::{BusType, HidApi, HidDevice};

    use super::{parse, Model, Reading};
    use crate::gamepad::Controller;

    /// Enumerating opens every HID interface on the system, so it's rare.
    const PROBE_INTERVAL: Duration = Duration::from_secs(2);
    /// A pad that Steam Input or DS4Windows mirror as a virtual Xbox one
    /// shows up on XInput right after we open it: that pad is theirs.
    const SHADOW_WINDOW: Duration = Duration::from_secs(3);
    /// Largest input report of the supported pads (Sony over BT: 78).
    const REPORT_BUF: usize = 128;

    struct Pad {
        dev: HidDevice,
        path: CString,
        model: Model,
        wired: bool,
        reading: Reading,
        opened: Instant,
        shadowed: bool,
    }

    pub struct Backend {
        api: Option<HidApi>,
        pads: Vec<Pad>,
        last_probe: Option<Instant>,
        dirty: bool,
    }

    impl Backend {
        pub fn new() -> Self {
            HidApi::disable_device_discovery();
            Backend { api: HidApi::new().ok(), pads: Vec::new(), last_probe: None, dirty: false }
        }

        /// Reads every pad and returns the merged button bits.
        /// `xinput_appeared` is true the frame a new XInput slot connected.
        pub fn poll(&mut self, now: Instant, xinput_appeared: bool) -> u32 {
            if !super::enabled() {
                if !self.pads.is_empty() {
                    self.pads.clear();
                    self.dirty = true;
                }
                return 0;
            }
            if self.last_probe.is_none_or(|t| now - t >= PROBE_INTERVAL) {
                self.last_probe = Some(now);
                self.probe(now);
            }
            if xinput_appeared {
                for pad in &mut self.pads {
                    if !pad.shadowed && now - pad.opened < SHADOW_WINDOW {
                        pad.shadowed = true;
                        self.dirty = true;
                    }
                }
            }
            let mut buttons = 0;
            let mut i = 0;
            while i < self.pads.len() {
                let pad = &mut self.pads[i];
                let mut buf = [0u8; REPORT_BUF];
                let mut latest = None;
                let mut gone = false;
                loop {
                    match pad.dev.read_timeout(&mut buf, 0) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Some(reading) = parse(pad.model, &buf[..n]) {
                                latest = Some(reading);
                            }
                        }
                        Err(_) => {
                            gone = true;
                            break;
                        }
                    }
                }
                if gone {
                    self.pads.remove(i);
                    self.dirty = true;
                    continue;
                }
                if let Some(reading) = latest {
                    if reading.level != pad.reading.level || reading.charging != pad.reading.charging {
                        self.dirty = true;
                    }
                    pad.reading = reading;
                }
                if !pad.shadowed {
                    buttons |= pad.reading.buttons;
                }
                i += 1;
            }
            buttons
        }

        /// True once after the pad list or a battery level changed.
        pub fn take_dirty(&mut self) -> bool {
            std::mem::take(&mut self.dirty)
        }

        /// Pads for the battery indicator; slots continue after XInput's four.
        pub fn controllers(&self, first_slot: u32) -> Vec<Controller> {
            self.pads
                .iter()
                .filter(|p| !p.shadowed)
                .enumerate()
                .map(|(i, p)| Controller {
                    slot: first_slot + i as u32,
                    wired: p.wired || p.reading.charging.unwrap_or(false),
                    level: p.reading.level.unwrap_or(3),
                    kind: p.model.kind(),
                    name: p.model.name().to_string(),
                })
                .collect()
        }

        fn probe(&mut self, now: Instant) {
            let Some(api) = self.api.as_mut() else { return };
            if api.reset_devices().is_err() || api.add_devices(0, 0).is_err() {
                return;
            }
            let mut seen: Vec<CString> = Vec::new();
            let mut fresh = Vec::new();
            for info in api.device_list() {
                let Some(model) = Model::from_ids(info.vendor_id(), info.product_id()) else { continue };
                // The gamepad collection only; skip XInput interfaces.
                if info.usage_page() != 0 && (info.usage_page() != 1 || !matches!(info.usage(), 4 | 5)) {
                    continue;
                }
                if super::is_xinput_path(&info.path().to_string_lossy()) {
                    continue;
                }
                let path = info.path().to_owned();
                seen.push(path.clone());
                if self.pads.iter().any(|p| p.path == path) {
                    continue;
                }
                let Ok(dev) = api.open_path(&path) else { continue };
                let bus = info.bus_type();
                let wired = matches!(bus, BusType::Usb);
                setup(&dev, model, bus);
                fresh.push(Pad { dev, path, model, wired, reading: Reading::default(), opened: now, shadowed: false });
            }
            let before = self.pads.len();
            self.pads.retain(|p| seen.contains(&p.path));
            if self.pads.len() != before || !fresh.is_empty() {
                self.dirty = true;
            }
            self.pads.append(&mut fresh);
        }
    }

    /// Puts the pad in the report mode the parsers expect. Best effort:
    /// a failed write just leaves the simpler report, which still works.
    fn setup(dev: &HidDevice, model: Model, bus: BusType) {
        match model {
            // Over Bluetooth the Sony pads send the full report only after
            // the calibration feature report has been requested.
            Model::DualSense | Model::DualShock4 => {
                if !matches!(bus, BusType::Usb) {
                    let mut buf = [0u8; 41];
                    buf[0] = 0x05;
                    let _ = dev.get_feature_report(&mut buf);
                }
            }
            Model::SwitchPro => {
                if !matches!(bus, BusType::Bluetooth) {
                    // USB: handshake, 3 Mbaud, handshake, no USB timeout.
                    for cmd in [0x02u8, 0x03, 0x02, 0x04] {
                        let mut packet = [0u8; 64];
                        packet[0] = 0x80;
                        packet[1] = cmd;
                        let _ = dev.write(&packet);
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                let len = if matches!(bus, BusType::Bluetooth) { 49 } else { 64 };
                // Full input reports (0x30), then player LED 1 so the pad
                // stops blinking.
                switch_subcommand(dev, len, 0, 0x03, &[0x30]);
                std::thread::sleep(Duration::from_millis(20));
                switch_subcommand(dev, len, 1, 0x30, &[0x01]);
            }
        }
    }

    /// Output report 0x01: packet counter, neutral rumble, subcommand, args.
    fn switch_subcommand(dev: &HidDevice, len: usize, seq: u8, subcommand: u8, args: &[u8]) {
        let mut packet = vec![0u8; len];
        packet[0] = 0x01;
        packet[1] = seq & 0x0F;
        packet[2..10].copy_from_slice(&[0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40]);
        packet[10] = subcommand;
        let end = (11 + args.len()).min(len);
        packet[11..end].copy_from_slice(&args[..end - 11]);
        let _ = dev.write(&packet);
    }
}

#[cfg(windows)]
pub use imp::Backend;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_the_supported_pads() {
        assert_eq!(Model::from_ids(VID_SONY, 0x0CE6), Some(Model::DualSense));
        assert_eq!(Model::from_ids(VID_SONY, 0x09CC), Some(Model::DualShock4));
        assert_eq!(Model::from_ids(VID_NINTENDO, 0x2009), Some(Model::SwitchPro));
        assert_eq!(Model::from_ids(0x045E, 0x02FF), None);
        assert_eq!(Model::DualSense.kind(), Kind::PlayStation);
        assert_eq!(Model::SwitchPro.kind(), Kind::Nintendo);
    }

    #[test]
    fn hat_and_stick_become_dpad() {
        assert_eq!(hat_bits(0), DPAD_UP);
        assert_eq!(hat_bits(3), DPAD_DOWN | DPAD_RIGHT);
        assert_eq!(hat_bits(8), 0);
        // Sony: Y 0 is up.
        assert_eq!(sony_sticks(128, 0), DPAD_UP);
        assert_eq!(sony_sticks(255, 255), DPAD_RIGHT | DPAD_DOWN);
        assert_eq!(sony_sticks(130, 120), 0);
        // Nintendo: a large Y is up.
        assert_eq!(stick_bits(0x8000, 0xFFFF, 0x8000, 16_000, true), DPAD_UP);
        assert_eq!(stick_bits(0, 0, 0x8000, 16_000, true), DPAD_LEFT | DPAD_DOWN);
    }

    #[test]
    fn dualsense_usb_report() {
        let mut r = [0u8; 64];
        r[0] = 0x01;
        r[1] = 128;
        r[2] = 128;
        r[5] = 0;
        r[6] = 200; // R2 pulled
        r[8] = 0x20 | 0x02; // Cross, hat east
        r[9] = 0x20 | 0x01; // Options, L1
        r[10] = 0x01; // PS
        r[53] = 0x06; // discharging, 60%
        let reading = parse(Model::DualSense, &r).unwrap();
        assert_eq!(reading.buttons, A | DPAD_RIGHT | START | LB | GUIDE | RT);
        assert_eq!(reading.level, Some(2));
        assert_eq!(reading.charging, Some(false));
    }

    #[test]
    fn dualsense_bluetooth_reports() {
        // Simple report, padded by Windows to 78 bytes.
        let mut r = [0u8; 78];
        r[0] = 0x01;
        r[1] = 128;
        r[2] = 128;
        r[5] = 0x80 | 0x08; // Triangle, hat released
        r[6] = 0x02; // R1
        r[7] = 0x01; // PS
        let reading = parse(Model::DualSense, &r).unwrap();
        assert_eq!(reading.buttons, Y | RB | GUIDE);
        assert_eq!(reading.level, None);
        // Full report after calibration was requested.
        let mut r = [0u8; 78];
        r[0] = 0x31;
        r[2] = 128;
        r[3] = 255; // stick down
        r[9] = 0x40 | 0x08; // Circle
        r[10] = 0x10; // Create
        r[54] = 0x2A; // full, charging complete
        let reading = parse(Model::DualSense, &r).unwrap();
        assert_eq!(reading.buttons, B | BACK | DPAD_DOWN);
        assert_eq!(reading.level, Some(3));
        assert_eq!(reading.charging, Some(true));
    }

    #[test]
    fn dualshock4_reports() {
        let mut r = [0u8; 64];
        r[0] = 0x01;
        r[1] = 128;
        r[2] = 128;
        r[5] = 0x10 | 0x04; // Square, hat south
        r[6] = 0x04; // L2 digital
        r[7] = 0x01; // PS
        r[30] = 0x12; // cable, 20%
        let reading = parse(Model::DualShock4, &r).unwrap();
        assert_eq!(reading.buttons, X | DPAD_DOWN | LT | GUIDE);
        assert_eq!(reading.level, Some(1));
        assert_eq!(reading.charging, Some(true));
        // Bluetooth simple report: same bytes, no battery.
        let reading = parse(Model::DualShock4, &r[..10]).unwrap();
        assert_eq!(reading.buttons, X | DPAD_DOWN | LT | GUIDE);
        assert_eq!(reading.level, None);
        // Bluetooth full report shifts everything by two.
        let mut r = [0u8; 78];
        r[0] = 0x11;
        r[3] = 0; // stick left
        r[4] = 128;
        r[7] = 0x20 | 0x08; // Cross
        r[8] = 0x20; // Options
        r[32] = 0x08; // 80% on battery
        let reading = parse(Model::DualShock4, &r).unwrap();
        assert_eq!(reading.buttons, A | START | DPAD_LEFT);
        assert_eq!(reading.level, Some(3));
        assert_eq!(reading.charging, Some(false));
    }

    #[test]
    fn switch_pro_reports() {
        // Simple report: B confirms, A goes back (positional).
        let mut r = [0u8; 12];
        r[0] = 0x3F;
        r[1] = 0x01 | 0x10 | 0x80; // B, L, ZR
        r[2] = 0x02 | 0x10; // Plus, Home
        r[3] = 6; // hat west
        r[4..6].copy_from_slice(&0x8000u16.to_le_bytes());
        r[6..8].copy_from_slice(&0x8000u16.to_le_bytes());
        let reading = parse(Model::SwitchPro, &r).unwrap();
        assert_eq!(reading.buttons, A | LB | RT | START | GUIDE | DPAD_LEFT);
        assert_eq!(reading.level, None);
        // Full report.
        let mut r = [0u8; 64];
        r[0] = 0x30;
        r[2] = 0x60; // medium, not charging
        r[3] = 0x08 | 0x40; // A, R
        r[4] = 0x01; // Minus
        r[5] = 0x02 | 0x80; // Up, ZL
        // Left stick centred: x = 2048, y = 2048.
        r[6] = 0x00;
        r[7] = 0x08;
        r[8] = 0x80;
        let reading = parse(Model::SwitchPro, &r).unwrap();
        assert_eq!(reading.buttons, B | RB | BACK | DPAD_UP | LT);
        assert_eq!(reading.level, Some(2));
        assert_eq!(reading.charging, Some(false));
        // Stick pushed right: x = 3500.
        r[6] = (3500 & 0xFF) as u8;
        r[7] = ((3500 >> 8) as u8) | 0x00;
        r[8] = 0x80;
        assert!(parse(Model::SwitchPro, &r).unwrap().buttons & DPAD_RIGHT != 0);
        // Subcommand replies and unknown ids aren't state.
        assert!(parse(Model::SwitchPro, &[0x81, 0x02, 0, 0]).is_none());
    }

    #[test]
    fn xinput_interfaces_are_skipped() {
        assert!(is_xinput_path(r"\\?\hid#vid_045e&pid_02ff&ig_00#8&1f3c&0&0000#{guid}"));
        assert!(!is_xinput_path(r"\\?\hid#vid_054c&pid_0ce6&mi_03#9&2a&0&0000#{guid}"));
    }
}
