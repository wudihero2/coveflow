use std::collections::VecDeque;
use std::time::Instant;

const MAX_WINDOW_SECS: u64 = 30 * 60; // 30 minutes

pub(crate) struct OccupancyTracker {
    samples: VecDeque<(Instant, bool)>,
}

impl OccupancyTracker {
    pub(crate) fn new() -> Self {
        Self {
            samples: VecDeque::with_capacity(MAX_WINDOW_SECS as usize + 1),
        }
    }

    pub(crate) fn record(&mut self, now: Instant, busy: bool) {
        self.samples.push_back((now, busy));
        let cutoff = now - std::time::Duration::from_secs(MAX_WINDOW_SECS);
        while self.samples.front().is_some_and(|(t, _)| *t < cutoff) {
            self.samples.pop_front();
        }
    }

    pub(crate) fn occupancy(&self, window_secs: u64) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let now = self
            .samples
            .back()
            .map(|(t, _)| *t)
            .unwrap_or_else(Instant::now);
        let cutoff = now - std::time::Duration::from_secs(window_secs);
        let mut total = 0u32;
        let mut busy_count = 0u32;
        for (t, busy) in self.samples.iter().rev() {
            if *t < cutoff {
                break;
            }
            total += 1;
            if *busy {
                busy_count += 1;
            }
        }
        if total == 0 {
            0.0
        } else {
            busy_count as f32 / total as f32
        }
    }

    pub(crate) fn occupancy_15s(&self) -> f32 {
        self.occupancy(15)
    }

    pub(crate) fn occupancy_5m(&self) -> f32 {
        self.occupancy(5 * 60)
    }

    pub(crate) fn occupancy_30m(&self) -> f32 {
        self.occupancy(30 * 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn empty_tracker_returns_zero() {
        let tracker = OccupancyTracker::new();
        assert!((tracker.occupancy_15s()).abs() < f32::EPSILON);
        assert!((tracker.occupancy_5m()).abs() < f32::EPSILON);
        assert!((tracker.occupancy_30m()).abs() < f32::EPSILON);
    }

    #[test]
    fn all_busy_returns_one() {
        let mut tracker = OccupancyTracker::new();
        let start = Instant::now();
        for i in 0..20 {
            tracker.record(start + Duration::from_secs(i), true);
        }
        assert!((tracker.occupancy_15s() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn all_idle_returns_zero() {
        let mut tracker = OccupancyTracker::new();
        let start = Instant::now();
        for i in 0..20 {
            tracker.record(start + Duration::from_secs(i), false);
        }
        assert!((tracker.occupancy_15s()).abs() < f32::EPSILON);
    }

    #[test]
    fn half_busy() {
        let mut tracker = OccupancyTracker::new();
        let start = Instant::now();
        for i in 0..10 {
            tracker.record(start + Duration::from_secs(i), false);
        }
        for i in 10..20 {
            tracker.record(start + Duration::from_secs(i), true);
        }
        let occ = tracker.occupancy(20);
        assert!((occ - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn old_samples_pruned() {
        let mut tracker = OccupancyTracker::new();
        let start = Instant::now();
        // Fill 35 minutes of samples (only 30 min retained)
        for i in 0..(35 * 60) {
            tracker.record(start + Duration::from_secs(i), true);
        }
        assert!(tracker.samples.len() <= MAX_WINDOW_SECS as usize + 1);
    }
}
