//! The mock clock: maps wall time to tape time.
//!
//! `tape_ms(now) = anchor_tape_ms + (now - anchor_wall) × speed`. `speed = 60` plays one hour of
//! tape in one minute; `speed = 0` freezes the clock (tests). Setting the time or the speed
//! re-anchors at the current wall instant, so the tape time is continuous across a speed change.

use std::time::Instant;

/// Wall-to-tape clock. Not shared by itself: the engine owns it under its lock.
#[derive(Debug, Clone)]
pub struct MockClock {
    anchor_wall: Instant,
    anchor_tape_ms: u64,
    speed: f64,
}

impl MockClock {
    /// A clock showing `start_tape_ms` now, running at `speed` (negative or NaN is treated as 0).
    pub fn new(start_tape_ms: u64, speed: f64) -> Self {
        MockClock {
            anchor_wall: Instant::now(),
            anchor_tape_ms: start_tape_ms,
            speed: sanitize(speed),
        }
    }

    /// Tape time now, Unix ms.
    pub fn now_ms(&self) -> u64 {
        self.at(Instant::now())
    }

    /// Tape time at a wall instant (instants before the anchor give the anchor time).
    pub fn at(&self, wall: Instant) -> u64 {
        let elapsed = wall.saturating_duration_since(self.anchor_wall);
        tape_after(
            self.anchor_tape_ms,
            elapsed.as_secs_f64() * 1000.0,
            self.speed,
        )
    }

    /// Current speed (tape ms per wall ms).
    pub fn speed(&self) -> f64 {
        self.speed
    }

    /// Jumps to `tape_ms`, keeping the speed.
    pub fn set_time(&mut self, tape_ms: u64) {
        self.anchor_wall = Instant::now();
        self.anchor_tape_ms = tape_ms;
    }

    /// Changes the speed from now on, keeping the current tape time.
    pub fn set_speed(&mut self, speed: f64) {
        let now = Instant::now();
        self.anchor_tape_ms = self.at(now);
        self.anchor_wall = now;
        self.speed = sanitize(speed);
    }
}

fn sanitize(speed: f64) -> f64 {
    if speed.is_finite() && speed > 0.0 {
        speed
    } else {
        0.0
    }
}

/// `anchor + wall_ms × speed`, rounded down, saturating.
pub(crate) fn tape_after(anchor_tape_ms: u64, wall_ms: f64, speed: f64) -> u64 {
    let delta = (wall_ms * speed).floor();
    if delta.is_finite() && delta > 0.0 {
        // f64 → u64 `as` saturates at u64::MAX.
        anchor_tape_ms.saturating_add(delta as u64)
    } else {
        anchor_tape_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn speed_sixty_maps_one_minute_to_one_hour() {
        assert_eq!(tape_after(1_000, 60_000.0, 60.0), 1_000 + 3_600_000);
        assert_eq!(tape_after(1_000, 0.5, 60.0), 1_030);
        assert_eq!(tape_after(1_000, 123.0, 0.0), 1_000);
    }

    #[test]
    fn at_uses_the_anchor() {
        let c = MockClock::new(10_000, 60.0);
        assert_eq!(c.at(c.anchor_wall), 10_000);
        assert_eq!(c.at(c.anchor_wall + Duration::from_secs(1)), 70_000);
        assert_eq!(MockClock::new(5, -3.0).speed(), 0.0);
        assert_eq!(MockClock::new(5, f64::NAN).speed(), 0.0);
    }

    #[test]
    fn frozen_clock_and_jumps() {
        let mut c = MockClock::new(42, 0.0);
        assert_eq!(c.now_ms(), 42);
        c.set_time(1_000);
        assert_eq!(c.now_ms(), 1_000);
        c.set_speed(0.0);
        assert_eq!(c.now_ms(), 1_000);
    }
}
