//! Pads that aren't XInput — DualSense, DualShock 4, Switch Pro — read
//! straight from their HID input reports. Windows.Gaming.Input only hands
//! input to the focused window, so it couldn't summon the overlay during a
//! game; raw HID works in the background like XInput does, and the device
//! stays shared with the game. Report layouts follow SDL's hidapi drivers
//! and the Linux hid-playstation / hid-nintendo drivers.
//!
//! Parsing and the mirror detection are pure and unit-tested; the hidapi
//! side lives in `imp`: a prober thread enumerates and opens the pads, and
//! the XInput polling thread in `gamepad.rs` only does non-blocking reads,
//! ORing these bits into the same bitmask so Guide, Back+Start, LB+RB and
//! the store assist behave the same on every pad.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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

/// How long a press may wait for its twin on the other backend.
const MIRROR_WINDOW: Duration = Duration::from_millis(80);
/// Presses in a row an XInput slot must echo before it counts as the
/// virtual copy of a HID pad.
const MIRROR_CONFIRM: u8 = 3;

/// The buttons Steam Input and DS4Windows pass to their virtual Xbox pad,
/// folded so a Nintendo-layout swap (A/B, X/Y) still matches. Sticks and
/// triggers are left out (thresholds differ) and so is Guide (Steam keeps it).
fn mirror_classes(bits: u32) -> u32 {
    let mut classes = bits & (LB | RB | BACK | START);
    if bits & (A | B) != 0 {
        classes |= 1;
    }
    if bits & (X | Y) != 0 {
        classes |= 2;
    }
    classes
}

/// Press edges of one device, plus the latest one still waiting for a twin.
#[derive(Default)]
struct Edges {
    prev: u32,
    pending: Option<(Instant, u32)>,
}

impl Edges {
    /// Newly pressed button classes this frame.
    fn step(&mut self, bits: u32) -> u32 {
        let classes = mirror_classes(bits);
        let new = classes & !self.prev;
        self.prev = classes;
        new
    }
}

/// Per-HID-pad state of the mirror detection.
#[derive(Default)]
pub struct Shadow {
    edges: Edges,
    /// Echoed presses in a row, per XInput slot.
    streak: [u8; 4],
    /// The XInput slot that is this pad's virtual copy (Steam Input,
    /// DS4Windows): the pad is then read through it only.
    pub by: Option<usize>,
}

/// Tells HID pads that Steam Input or DS4Windows also expose as a virtual
/// Xbox pad: that slot echoes every press of the HID pad within a few
/// frames, while a real second pad presses on its own. A pad is shadowed
/// after `MIRROR_CONFIRM` echoed presses in a row, and back as soon as its
/// slot disconnects. Until then both are read and ORed into one bitmask,
/// so the twin presses still collapse into one.
#[derive(Default)]
pub struct Mirror {
    slots: [Edges; 4],
}

impl Mirror {
    /// One frame: `pads` are the HID pads with this frame's buttons,
    /// `xinput` the slots' buttons (`None` when disconnected). True when a
    /// pad got shadowed or released.
    pub fn step(&mut self, now: Instant, pads: &mut [(&mut Shadow, u32)], xinput: &[Option<u32>; 4]) -> bool {
        let mut changed = false;
        let mut claimed = [false; 4];
        for (pad, _) in pads.iter_mut() {
            if let Some(slot) = pad.by {
                if xinput[slot].is_none() {
                    pad.by = None;
                    pad.streak = [0; 4];
                    changed = true;
                } else {
                    claimed[slot] = true;
                }
            }
        }
        // Presses nobody echoed in time: not a twin.
        for (pad, _) in pads.iter_mut() {
            if pad.edges.pending.is_some_and(|(t, _)| now - t > MIRROR_WINDOW) {
                pad.edges.pending = None;
                pad.streak = [0; 4];
            }
        }
        for slot in 0..4 {
            if self.slots[slot].pending.is_some_and(|(t, _)| now - t > MIRROR_WINDOW) {
                self.slots[slot].pending = None;
                for (pad, _) in pads.iter_mut() {
                    pad.streak[slot] = 0;
                }
            }
        }
        let mut slot_new = [0u32; 4];
        for slot in 0..4 {
            match xinput[slot] {
                None => {
                    self.slots[slot] = Edges::default();
                    for (pad, _) in pads.iter_mut() {
                        pad.streak[slot] = 0;
                    }
                }
                Some(bits) => slot_new[slot] = self.slots[slot].step(bits),
            }
            if claimed[slot] {
                slot_new[slot] = 0;
                self.slots[slot].pending = None;
            }
        }
        // HID presses against this frame's or still-pending slot presses.
        for (pad, bits) in pads.iter_mut() {
            let new = pad.edges.step(*bits);
            if pad.by.is_some() || new == 0 {
                continue;
            }
            let hit = (0..4).find(|&s| !claimed[s] && (slot_new[s] == new || self.slots[s].pending.is_some_and(|(_, c)| c == new)));
            match hit {
                Some(slot) => {
                    if slot_new[slot] == new {
                        slot_new[slot] = 0;
                    } else {
                        self.slots[slot].pending = None;
                    }
                    pad.streak[slot] = pad.streak[slot].saturating_add(1);
                }
                None => {
                    // A press replaced before its twin came is a miss too.
                    if pad.edges.pending.is_some() {
                        pad.streak = [0; 4];
                    }
                    pad.edges.pending = Some((now, new));
                }
            }
        }
        // Slot presses left over: against still-pending HID presses.
        for slot in 0..4 {
            let new = slot_new[slot];
            if new == 0 {
                continue;
            }
            let hit = pads.iter_mut().find(|(p, _)| p.by.is_none() && p.edges.pending.is_some_and(|(_, c)| c == new));
            match hit {
                Some((pad, _)) => {
                    pad.edges.pending = None;
                    pad.streak[slot] = pad.streak[slot].saturating_add(1);
                }
                None => {
                    if self.slots[slot].pending.is_some() {
                        for (pad, _) in pads.iter_mut() {
                            pad.streak[slot] = 0;
                        }
                    }
                    self.slots[slot].pending = Some((now, new));
                }
            }
        }
        for (pad, _) in pads.iter_mut() {
            if pad.by.is_some() {
                continue;
            }
            if let Some(slot) = (0..4).find(|&s| !claimed[s] && pad.streak[s] >= MIRROR_CONFIRM) {
                pad.by = Some(slot);
                pad.streak = [0; 4];
                pad.edges.pending = None;
                claimed[slot] = true;
                changed = true;
            }
        }
        changed
    }
}

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;
    use std::ffi::CString;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::time::{Duration, Instant};

    use hidapi::{BusType, HidApi, HidDevice};

    use super::{parse, Mirror, Model, Reading, Shadow};
    use crate::gamepad::Controller;

    /// Enumerating opens every HID interface on the system, so it's rare
    /// and runs on its own thread, away from the input loop.
    const PROBE_INTERVAL: Duration = Duration::from_secs(2);
    /// A path whose reads failed (another app holds it exclusively) isn't
    /// reopened before this.
    const RETRY_AFTER: Duration = Duration::from_secs(10);
    /// Largest input report of the supported pads (Sony over BT: 78).
    const REPORT_BUF: usize = 128;

    /// A pad the prober opened and set up.
    struct Opened {
        dev: HidDevice,
        path: CString,
        model: Model,
        wired: bool,
    }

    /// One enumeration: the supported paths present, and the new ones.
    struct Probe {
        seen: Vec<CString>,
        fresh: Vec<Opened>,
    }

    struct Pad {
        dev: HidDevice,
        path: CString,
        model: Model,
        wired: bool,
        reading: Reading,
        /// Buttons this frame: every report read ORed, so a tap shorter
        /// than a frame still counts.
        bits: u32,
        shadow: Shadow,
    }

    pub struct Backend {
        pads: Vec<Pad>,
        mirror: Mirror,
        probes: Receiver<Probe>,
        /// Paths the input loop let go of; `true` when reads failed.
        closed: Sender<(CString, bool)>,
        dirty: bool,
    }

    impl Backend {
        pub fn new() -> Self {
            let (probe_tx, probes) = channel();
            let (closed, closed_rx) = channel();
            let _ = std::thread::Builder::new().name("hidpad-probe".into()).spawn(move || prober(probe_tx, closed_rx));
            Backend { pads: Vec::new(), mirror: Mirror::default(), probes, closed, dirty: false }
        }

        /// Reads every pad without blocking and returns the merged button
        /// bits of those that aren't read through XInput already.
        /// `xinput`: this frame's buttons per slot, `None` when disconnected.
        pub fn poll(&mut self, now: Instant, xinput: &[Option<u32>; 4]) -> u32 {
            if !super::enabled() {
                if !self.pads.is_empty() {
                    self.dirty = true;
                }
                for pad in self.pads.drain(..) {
                    let _ = self.closed.send((pad.path, false));
                }
                while let Ok(probe) = self.probes.try_recv() {
                    for opened in probe.fresh {
                        let _ = self.closed.send((opened.path, false));
                    }
                }
                return 0;
            }
            while let Ok(probe) = self.probes.try_recv() {
                let mut i = 0;
                while i < self.pads.len() {
                    if probe.seen.contains(&self.pads[i].path) {
                        i += 1;
                    } else {
                        let pad = self.pads.remove(i);
                        let _ = self.closed.send((pad.path, false));
                        self.dirty = true;
                    }
                }
                for Opened { dev, path, model, wired } in probe.fresh {
                    self.pads.push(Pad { dev, path, model, wired, reading: Reading::default(), bits: 0, shadow: Shadow::default() });
                    self.dirty = true;
                }
            }
            let mut i = 0;
            while i < self.pads.len() {
                let pad = &mut self.pads[i];
                let mut buf = [0u8; REPORT_BUF];
                let mut pressed = 0;
                let mut latest = None;
                let mut gone = false;
                loop {
                    match pad.dev.read_timeout(&mut buf, 0) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Some(reading) = parse(pad.model, &buf[..n]) {
                                pressed |= reading.buttons;
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
                    let pad = self.pads.remove(i);
                    let _ = self.closed.send((pad.path, true));
                    self.dirty = true;
                    continue;
                }
                pad.bits = match latest {
                    Some(reading) => {
                        if reading.level != pad.reading.level || reading.charging != pad.reading.charging {
                            self.dirty = true;
                        }
                        pad.reading = reading;
                        pressed
                    }
                    // No new report: the pad still holds what it last said.
                    None => pad.reading.buttons,
                };
                i += 1;
            }
            let mut pairs: Vec<(&mut Shadow, u32)> = self.pads.iter_mut().map(|p| (&mut p.shadow, p.bits)).collect();
            if self.mirror.step(now, &mut pairs, xinput) {
                self.dirty = true;
            }
            self.pads.iter().filter(|p| p.shadow.by.is_none()).fold(0, |acc, p| acc | p.bits)
        }

        /// True once after the pad list or a battery level changed.
        pub fn take_dirty(&mut self) -> bool {
            std::mem::take(&mut self.dirty)
        }

        /// Pads for the battery indicator; slots continue after XInput's four.
        pub fn controllers(&self, first_slot: u32) -> Vec<Controller> {
            self.pads
                .iter()
                .filter(|p| p.shadow.by.is_none())
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
    }

    /// Enumerates, opens and sets up the supported pads every
    /// `PROBE_INTERVAL`, handing them to the input loop. Opening and the
    /// setup writes can block, so they never run on the input thread.
    fn prober(tx: Sender<Probe>, closed: Receiver<(CString, bool)>) {
        HidApi::disable_device_discovery();
        let mut api: Option<HidApi> = None;
        // Paths the input loop holds, and failed ones waiting for a retry.
        let mut handed: Vec<CString> = Vec::new();
        let mut cooldown: HashMap<CString, Instant> = HashMap::new();
        loop {
            while let Ok((path, failed)) = closed.try_recv() {
                handed.retain(|p| *p != path);
                if failed {
                    cooldown.insert(path, Instant::now() + RETRY_AFTER);
                }
            }
            if super::enabled() {
                if api.is_none() {
                    api = HidApi::new().ok();
                }
                if let Some(probe) = api.as_mut().and_then(|api| probe(api, &mut handed, &mut cooldown)) {
                    if tx.send(probe).is_err() {
                        return;
                    }
                }
            }
            std::thread::sleep(PROBE_INTERVAL);
        }
    }

    fn probe(api: &mut HidApi, handed: &mut Vec<CString>, cooldown: &mut HashMap<CString, Instant>) -> Option<Probe> {
        if api.reset_devices().is_err() || api.add_devices(0, 0).is_err() {
            return None;
        }
        let now = Instant::now();
        cooldown.retain(|_, until| *until > now);
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
            if handed.contains(&path) || cooldown.contains_key(&path) {
                continue;
            }
            let Ok(dev) = api.open_path(&path) else { continue };
            let bus = info.bus_type();
            setup(&dev, model, bus);
            handed.push(path.clone());
            fresh.push(Opened { dev, path, model, wired: matches!(bus, BusType::Usb) });
        }
        handed.retain(|p| seen.contains(p));
        Some(Probe { seen, fresh })
    }

    /// Readies the pad for the parsers. Over Bluetooth the pads stay in
    /// their simple report (Sony 0x01, Nintendo 0x3F): the full ones
    /// (0x31, 0x11, 0x30) would carry the battery but are vendor-defined,
    /// and switching to them breaks DirectInput and generic HID readers in
    /// every other app until the pad reconnects (SDL leaves it off too).
    fn setup(dev: &HidDevice, model: Model, bus: BusType) {
        if model != Model::SwitchPro {
            return;
        }
        let bluetooth = matches!(bus, BusType::Bluetooth);
        if !bluetooth {
            // USB sends nothing until the handshake: handshake, 3 Mbaud,
            // handshake, no USB timeout; then full reports (0x30).
            for cmd in [0x02u8, 0x03, 0x02, 0x04] {
                let mut packet = [0u8; 64];
                packet[0] = 0x80;
                packet[1] = cmd;
                let _ = dev.write(&packet);
                std::thread::sleep(Duration::from_millis(20));
            }
            switch_subcommand(dev, 64, 0, 0x03, &[0x30]);
            std::thread::sleep(Duration::from_millis(20));
        }
        // Player LED 1, so the pad stops blinking.
        switch_subcommand(dev, if bluetooth { 49 } else { 64 }, 1, 0x30, &[0x01]);
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

    /// Runs one frame per entry: (HID buttons, slot 0 buttons), 8 ms apart.
    fn run(mirror: &mut Mirror, shadow: &mut Shadow, start: Instant, frames: &[(u32, Option<u32>)]) -> Instant {
        let mut now = start;
        for &(hid, slot0) in frames {
            now += Duration::from_millis(8);
            mirror.step(now, &mut [(&mut *shadow, hid)], &[slot0, None, None, None]);
        }
        now
    }

    #[test]
    fn a_virtual_twin_shadows_the_hid_pad() {
        let mut mirror = Mirror::default();
        let mut pad = Shadow::default();
        let t = Instant::now();
        // Both present from the start: nothing happens without presses.
        let t = run(&mut mirror, &mut pad, t, &[(0, Some(0)); 50]);
        assert_eq!(pad.by, None);
        // Three presses echoed one frame later, the last one swapped A/B
        // (Nintendo layout in Steam).
        let t = run(&mut mirror, &mut pad, t, &[(A, Some(0)), (A, Some(A)), (0, Some(A)), (0, Some(0))]);
        let t = run(&mut mirror, &mut pad, t, &[(0, Some(START)), (START, Some(START)), (0, Some(0))]);
        assert_eq!(pad.by, None);
        let t = run(&mut mirror, &mut pad, t, &[(A, Some(0)), (A, Some(B)), (0, Some(0))]);
        assert_eq!(pad.by, Some(0));
        // DS4Windows closes: the slot goes and the pad is read again.
        run(&mut mirror, &mut pad, t, &[(0, None)]);
        assert_eq!(pad.by, None);
    }

    #[test]
    fn a_real_second_pad_never_shadows() {
        let mut mirror = Mirror::default();
        let mut pad = Shadow::default();
        let t = Instant::now();
        // Two presses happen to coincide, then the Xbox pad presses alone.
        let t = run(&mut mirror, &mut pad, t, &[(A, Some(A)), (0, Some(0)), (LB, Some(LB)), (0, Some(0))]);
        let t = run(&mut mirror, &mut pad, t, &[(0, Some(X)); 20]);
        let t = run(&mut mirror, &mut pad, t, &[(0, Some(0)), (RB, Some(RB)), (0, Some(0))]);
        assert_eq!(pad.by, None);
        // The DualSense presses alone: the streak starts over.
        let t = run(&mut mirror, &mut pad, t, &[(Y, Some(0)); 20]);
        let t = run(&mut mirror, &mut pad, t, &[(0, Some(0)), (B, Some(B)), (0, Some(0))]);
        run(&mut mirror, &mut pad, t, &[(START, Some(START)), (0, Some(0))]);
        assert_eq!(pad.by, None);
        // Triggers, sticks and Guide never count.
        let mut pad = Shadow::default();
        let t = Instant::now();
        let mut frames = Vec::new();
        for bits in [GUIDE, LT, DPAD_UP, RT] {
            frames.push((bits, Some(bits)));
            frames.push((0, Some(0)));
        }
        run(&mut mirror, &mut pad, t, &frames);
        assert_eq!(pad.by, None);
    }

    #[test]
    fn each_slot_shadows_one_pad() {
        let mut mirror = Mirror::default();
        let mut one = Shadow::default();
        let mut two = Shadow::default();
        let mut now = Instant::now();
        // Pad one is mirrored on slot 1, pad two on slot 0.
        for (a, b) in [(A, 0), (0, 0), (0, X), (0, 0), (LB, 0), (0, 0), (0, BACK), (0, 0), (Y, 0), (0, 0), (0, RB), (0, 0)] {
            now += Duration::from_millis(8);
            mirror.step(now, &mut [(&mut one, a), (&mut two, b)], &[Some(b), Some(a), None, None]);
        }
        assert_eq!(one.by, Some(1));
        assert_eq!(two.by, Some(0));
    }

    #[test]
    fn xinput_interfaces_are_skipped() {
        assert!(is_xinput_path(r"\\?\hid#vid_045e&pid_02ff&ig_00#8&1f3c&0&0000#{guid}"));
        assert!(!is_xinput_path(r"\\?\hid#vid_054c&pid_0ce6&mi_03#9&2a&0&0000#{guid}"));
    }
}
