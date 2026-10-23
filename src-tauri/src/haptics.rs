//! Controller vibration for interface feedback. Patterns are queued from
//! anywhere and written to the pads by the XInput polling thread, which also
//! switches the motors off: XInput keeps the last state, so pulses stay short
//! and are never left running.
// The queue and scheduler only run on the Windows polling thread; the tests cover them elsewhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pattern {
    /// Opening a menu, going back.
    Tick,
    /// Confirming a choice.
    Select,
    /// Something went wrong.
    Error,
    /// A game is starting: two beats.
    Launch,
    /// A toast worth noticing (update ready, battery low).
    Notify,
}

impl Pattern {
    pub fn parse(name: &str) -> Option<Pattern> {
        Some(match name {
            "tick" => Pattern::Tick,
            "select" => Pattern::Select,
            "error" => Pattern::Error,
            "launch" => Pattern::Launch,
            "notify" => Pattern::Notify,
            _ => return None,
        })
    }

    /// `(start, length, strong, weak)` in milliseconds and 0..1 motor
    /// speeds; every beat stays ≤150 ms so a crash can't leave a pad buzzing.
    fn beats(self) -> &'static [(u64, u64, f32, f32)] {
        match self {
            Pattern::Tick => &[(0, 30, 0.0, 0.4)],
            Pattern::Select => &[(0, 60, 0.25, 0.65)],
            Pattern::Error => &[(0, 140, 0.75, 0.2)],
            Pattern::Launch => &[(0, 70, 0.55, 0.3), (130, 110, 0.9, 0.5)],
            Pattern::Notify => &[(0, 40, 0.0, 0.55), (100, 40, 0.0, 0.55)],
        }
    }
}

/// Maps the `haptics` setting (0 off, 1 light, 2 normal) to a motor factor.
pub fn factor(level: u8) -> f32 {
    match level {
        0 => 0.0,
        1 => 0.45,
        _ => 1.0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Beat {
    from: Instant,
    until: Instant,
    strong: u16,
    weak: u16,
}

/// Pending patterns, drained by the polling thread. Bounded so a frontend
/// without a native backend can't grow it forever.
#[cfg(windows)]
static PENDING: std::sync::Mutex<Vec<(Pattern, u8)>> = std::sync::Mutex::new(Vec::new());

/// Queues a pattern at the given setting level. No-op where no native
/// controller backend exists.
pub fn rumble(pattern: Pattern, level: u8) {
    if level == 0 {
        return;
    }
    #[cfg(windows)]
    {
        let mut pending = PENDING.lock().unwrap();
        if pending.len() < 8 {
            pending.push((pattern, level));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pattern;
    }
}

#[cfg(windows)]
pub fn take_pending() -> Vec<(Pattern, u8)> {
    std::mem::take(&mut *PENDING.lock().unwrap())
}

/// Pure scheduler: turns queued patterns into the motor speeds to write each
/// poll. Overlapping beats add up to the loudest one; a single zero is
/// written when the last beat ends.
#[derive(Default)]
pub struct PulseQueue {
    beats: Vec<Beat>,
    last: (u16, u16),
}

impl PulseQueue {
    pub fn push(&mut self, pattern: Pattern, level: u8, now: Instant) {
        let factor = factor(level);
        if factor <= 0.0 {
            return;
        }
        let speed = |v: f32| (v * factor * f32::from(u16::MAX)).round().clamp(0.0, f32::from(u16::MAX)) as u16;
        for &(start, length, strong, weak) in pattern.beats() {
            let from = now + Duration::from_millis(start);
            self.beats.push(Beat { from, until: from + Duration::from_millis(length), strong: speed(strong), weak: speed(weak) });
        }
    }

    /// Motor speeds to write now, or `None` when nothing changed since the
    /// last write.
    pub fn step(&mut self, now: Instant) -> Option<(u16, u16)> {
        self.beats.retain(|b| b.until > now);
        let mut target = (0u16, 0u16);
        for b in self.beats.iter().filter(|b| b.from <= now) {
            target.0 = target.0.max(b.strong);
            target.1 = target.1.max(b.weak);
        }
        if target == self.last {
            return None;
        }
        self.last = target;
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn single_beat_writes_once_then_a_single_zero() {
        let mut q = PulseQueue::default();
        let t0 = Instant::now();
        q.push(Pattern::Select, 2, t0);
        let on = q.step(t0).expect("motors on");
        assert!(on.0 > 0 && on.1 > 0);
        assert_eq!(q.step(t0 + ms(30)), None);
        assert_eq!(q.step(t0 + ms(60)), Some((0, 0)));
        assert_eq!(q.step(t0 + ms(100)), None);
    }

    #[test]
    fn launch_has_two_beats_with_a_gap() {
        let mut q = PulseQueue::default();
        let t0 = Instant::now();
        q.push(Pattern::Launch, 2, t0);
        let first = q.step(t0).unwrap();
        assert_eq!(q.step(t0 + ms(100)), Some((0, 0)));
        let second = q.step(t0 + ms(130)).unwrap();
        assert!(second.0 > first.0);
        assert_eq!(q.step(t0 + ms(240)), Some((0, 0)));
    }

    #[test]
    fn off_level_queues_nothing_and_light_is_weaker() {
        let t0 = Instant::now();
        let mut off = PulseQueue::default();
        off.push(Pattern::Error, 0, t0);
        assert_eq!(off.step(t0), None);

        let mut light = PulseQueue::default();
        light.push(Pattern::Error, 1, t0);
        let mut normal = PulseQueue::default();
        normal.push(Pattern::Error, 2, t0);
        assert!(light.step(t0).unwrap().0 < normal.step(t0).unwrap().0);
    }

    #[test]
    fn overlapping_beats_take_the_loudest() {
        let mut q = PulseQueue::default();
        let t0 = Instant::now();
        q.push(Pattern::Tick, 2, t0);
        q.push(Pattern::Error, 2, t0 + ms(10));
        let tick = q.step(t0).unwrap();
        let both = q.step(t0 + ms(10)).unwrap();
        assert!(both.0 > tick.0 && both.1 == tick.1);
        // Tick ends, Error keeps the strong motor going.
        let later = q.step(t0 + ms(40)).unwrap();
        assert_eq!(later.0, both.0);
        assert!(later.1 < both.1);
        assert_eq!(q.step(t0 + ms(200)), Some((0, 0)));
    }

    #[test]
    fn every_beat_is_short() {
        for p in [Pattern::Tick, Pattern::Select, Pattern::Error, Pattern::Launch, Pattern::Notify] {
            for &(_, length, _, _) in p.beats() {
                assert!(length <= 150, "{p:?} beat of {length} ms");
            }
        }
        assert_eq!(Pattern::parse("launch"), Some(Pattern::Launch));
        assert_eq!(Pattern::parse("buzz"), None);
    }
}
