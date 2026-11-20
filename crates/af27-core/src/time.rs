// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Time and level helpers.

/// Formats seconds as `m:ss.mmm`, or `h:mm:ss.mmm` past an hour.
pub fn format_time(secs: f64) -> String {
    let secs = secs.max(0.0);
    let total_ms = (secs * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let s = (total_ms / 1000) % 60;
    let m = (total_ms / 60_000) % 60;
    let h = total_ms / 3_600_000;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{ms:03}")
    } else {
        format!("{m}:{s:02}.{ms:03}")
    }
}

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub fn gain_to_db(g: f32) -> f32 {
    if g <= 1e-9 {
        -180.0
    } else {
        20.0 * g.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format_time(0.0), "0:00.000");
        assert_eq!(format_time(75.0554), "1:15.055");
        assert_eq!(format_time(3725.5), "1:02:05.500");
    }

    #[test]
    fn db_round_trip() {
        assert!((gain_to_db(db_to_gain(-6.0)) + 6.0).abs() < 1e-4);
    }
}
