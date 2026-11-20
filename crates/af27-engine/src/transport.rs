// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Where playback goes: the stop point and the loop or skipped range.
//!
//! Pure position logic, shared by the audio thread and its tests. Times
//! are in whatever unit the caller uses (seconds on the audio thread,
//! frames in the tests). Jumps are exact: what playback overshoots a seam
//! by carries over to the far side, so no sample is lost or repeated.

/// The repeat or skip part of a plan.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Region {
    /// Play straight through.
    #[default]
    None,
    /// Reaching `end` from before it jumps back to `start`. Starting at or
    /// after `end` plays straight on.
    Loop { start: f64, end: f64 },
    /// Reaching `start` jumps on to `end`; starting inside starts at `end`.
    Skip { start: f64, end: f64 },
}

impl Region {
    /// A loop or skip with no length (under a microsecond) is no region.
    fn real(self) -> Region {
        match self {
            Region::Loop { start, end } | Region::Skip { start, end } if end <= start + 1e-6 => {
                Region::None
            }
            r => r,
        }
    }
}

/// What happens at a seam.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Stop,
    Jump(f64),
}

/// Where playback stops and what it repeats or skips.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    /// Playback stops here (`f64::INFINITY`: never).
    pub stop: f64,
    pub region: Region,
}

impl Default for Plan {
    fn default() -> Plan {
        Plan {
            stop: f64::INFINITY,
            region: Region::None,
        }
    }
}

impl Plan {
    pub fn new(stop: f64, region: Region) -> Plan {
        Plan {
            stop,
            region: region.real(),
        }
    }

    /// Where playback asked to start at `t` really starts: inside a
    /// skipped range it starts at the range's end; past a loop it rolls
    /// over into the loop's start (before it, it plays into the loop).
    pub fn start_at(&self, t: f64) -> f64 {
        match self.region.real() {
            Region::Skip { start, end } if t >= start && t < end => end,
            Region::Loop { start, end } if t >= end => start,
            _ => t,
        }
    }

    /// The next seam at or after `t`, and what happens there.
    pub fn next_seam(&self, t: f64) -> (f64, Action) {
        match self.region.real() {
            Region::Loop { start, end } if t < end => (end, Action::Jump(start)),
            Region::Skip { start, end } if t < start && start < self.stop => {
                (start, Action::Jump(end))
            }
            _ => (self.stop, Action::Stop),
        }
    }

    /// Plays `d` on from `t`: the position reached, or None when playback
    /// stopped (at the stop point) on the way.
    pub fn advance(&self, mut t: f64, mut d: f64) -> Option<f64> {
        loop {
            let (at, action) = self.next_seam(t);
            if t + d < at {
                return Some(t + d);
            }
            d -= at - t;
            match action {
                Action::Stop => return None,
                Action::Jump(to) => t = to,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn looping(a: f64, b: f64) -> Plan {
        Plan::new(f64::INFINITY, Region::Loop { start: a, end: b })
    }

    #[test]
    fn a_loop_wraps_to_its_start_not_where_play_began() {
        let p = looping(100.0, 200.0);
        assert_eq!(p.advance(50.0, 40.0), Some(90.0));
        assert_eq!(p.advance(50.0, 150.0), Some(100.0));
        assert_eq!(p.advance(50.0, 275.0), Some(125.0));
        // Started inside: back to the loop start, not to 150.
        assert_eq!(p.advance(150.0, 60.0), Some(110.0));
    }

    #[test]
    fn starting_after_a_loop_rolls_into_its_start() {
        let p = looping(100.0, 200.0);
        assert_eq!(p.start_at(300.0), 100.0);
        assert_eq!(p.start_at(200.0), 100.0);
        assert_eq!(p.start_at(50.0), 50.0);
        assert_eq!(p.start_at(150.0), 150.0);
    }

    #[test]
    fn a_skip_jumps_over_its_range() {
        let p = Plan::new(
            f64::INFINITY,
            Region::Skip {
                start: 100.0,
                end: 200.0,
            },
        );
        assert_eq!(p.advance(50.0, 60.0), Some(210.0));
        assert_eq!(p.start_at(150.0), 200.0);
        assert_eq!(p.start_at(50.0), 50.0);
    }

    #[test]
    fn the_stop_point_stops() {
        let p = Plan::new(200.0, Region::None);
        assert_eq!(p.advance(150.0, 49.0), Some(199.0));
        assert_eq!(p.advance(150.0, 50.0), None);
        // A skip past the stop point never happens.
        let p = Plan::new(
            80.0,
            Region::Skip {
                start: 100.0,
                end: 200.0,
            },
        );
        assert_eq!(p.advance(50.0, 40.0), None);
    }

    #[test]
    fn an_empty_region_is_ignored() {
        let p = Plan::new(
            f64::INFINITY,
            Region::Loop {
                start: 5.0,
                end: 5.0,
            },
        );
        assert_eq!(p.region, Region::None);
        assert_eq!(p.advance(0.0, 10.0), Some(10.0));
    }
}
