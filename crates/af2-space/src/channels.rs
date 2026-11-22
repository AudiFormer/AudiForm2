// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Multichannel files in a 3D Space session: what each channel of a file
//! is (by its WAV speaker mask when the file has one, else by how many
//! channels it has), and where each channel plays in a layout.

use crate::pan::{point_gains_into, speakers, Gains, MAX_SPEAKERS};
use af2_core::space::SpaceLayout;

/// Names for a file's channels by channel count (the usual orders).
/// Counts with no usual order are numbered.
pub fn names_by_count(n: usize) -> Vec<String> {
    let fixed: &[&str] = match n {
        1 => &["M"],
        2 => &["L", "R"],
        3 => &["L", "R", "C"],
        4 => &["L", "R", "Ls", "Rs"],
        5 => &["L", "R", "C", "Ls", "Rs"],
        6 => &["L", "R", "C", "LFE", "Ls", "Rs"],
        7 => &["L", "R", "C", "LFE", "Cs", "Ls", "Rs"],
        8 => &["L", "R", "C", "LFE", "Lrs", "Rrs", "Lss", "Rss"],
        10 => &["L", "R", "C", "LFE", "Ls", "Rs", "Ltf", "Rtf", "Ltr", "Rtr"],
        12 => &[
            "L", "R", "C", "LFE", "Lrs", "Rrs", "Lss", "Rss", "Ltf", "Rtf", "Ltr", "Rtr",
        ],
        _ => &[],
    };
    if fixed.is_empty() {
        (1..=n).map(|i| i.to_string()).collect()
    } else {
        fixed.iter().map(|s| s.to_string()).collect()
    }
}

/// Names for a file's channels from its WAV speaker mask (channels in
/// ascending bit order). Falls back to [`names_by_count`] when the mask
/// is zero or does not name as many channels as the file has.
pub fn names(n: usize, mask: Option<u32>) -> Vec<String> {
    let Some(mask) = mask.filter(|m| *m != 0 && m.count_ones() as usize == n) else {
        return names_by_count(n);
    };
    // With side bits present, the back pair is the rear pair of a 7.x.
    let sides = mask & 0x600 != 0;
    let mut out = Vec::with_capacity(n);
    for bit in 0..18 {
        if mask & (1 << bit) == 0 {
            continue;
        }
        let name = match bit {
            0 => "L",
            1 => "R",
            2 => "C",
            3 => "LFE",
            4 if sides => "Lrs",
            4 => "Ls",
            5 if sides => "Rrs",
            5 => "Rs",
            6 => "Lc",
            7 => "Rc",
            8 => "Cs",
            9 => "Lss",
            10 => "Rss",
            11 => "Tc",
            12 => "Ltf",
            13 => "Tfc",
            14 => "Rtf",
            15 => "Ltr",
            16 => "Tbc",
            _ => "Rtr",
        };
        out.push(name.to_string());
    }
    if out.len() == n {
        out
    } else {
        names_by_count(n)
    }
}

/// Where one channel of a file plays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    /// Straight to this speaker of the layout.
    Speaker(usize),
    /// Placed at the spot its speaker has in another layout (the layout
    /// has no speaker of that name).
    Spot(Gains),
    /// Not played (a low-frequency channel in a layout without one).
    Drop,
}

impl Target {
    /// The gain per speaker of the layout.
    pub fn gains(&self) -> Gains {
        match self {
            Target::Speaker(i) => {
                let mut g = [0.0; MAX_SPEAKERS];
                if *i < MAX_SPEAKERS {
                    g[*i] = 1.0;
                }
                g
            }
            Target::Spot(g) => *g,
            Target::Drop => [0.0; MAX_SPEAKERS],
        }
    }
}

/// Where each named channel plays in `layout`: on the speaker of its
/// name; dropped if it is the LFE and the layout has none; else at the
/// spot that speaker has in a bigger layout; else (a channel of no known
/// speaker) on the speaker with its index, if there is one.
pub fn targets(layout: SpaceLayout, names: &[String]) -> Vec<Target> {
    let sp = speakers(layout);
    names
        .iter()
        .enumerate()
        .map(|(k, name)| {
            if let Some(i) = sp.iter().position(|s| s.name == name) {
                return Target::Speaker(i);
            }
            if name == "LFE" {
                return Target::Drop;
            }
            let elsewhere = [
                SpaceLayout::Surround714,
                SpaceLayout::Surround51,
                SpaceLayout::Surround512,
            ]
            .into_iter()
            .flat_map(speakers)
            .find(|s| s.name == name)
            .and_then(|s| s.at);
            if let Some([x, y, z]) = elsewhere {
                let mut g = [0.0; MAX_SPEAKERS];
                point_gains_into(layout, x, y, z, 0.0, &mut g);
                return Target::Spot(g);
            }
            if k < sp.len() {
                Target::Speaker(k)
            } else {
                Target::Drop
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_count_and_mask() {
        assert_eq!(names_by_count(6), ["L", "R", "C", "LFE", "Ls", "Rs"]);
        assert_eq!(names_by_count(9)[8], "9");
        // A 5.1.2 file (8 channels) is not taken for 7.1 when its mask says so.
        let five12 = names(8, Some(0x503F));
        assert_eq!(five12, ["L", "R", "C", "LFE", "Ls", "Rs", "Ltf", "Rtf"]);
        assert_eq!(names(8, None)[4], "Lrs");
        assert_eq!(names(8, Some(0x63F))[4], "Lrs");
        // A mask that does not match the channel count is ignored.
        assert_eq!(names(6, Some(0x3)), names_by_count(6));
        // 5.1 with side bits.
        assert_eq!(names(6, Some(0x60F))[4], "Lss");
    }

    #[test]
    fn a_5_1_file_in_7_1_4_pins_its_speakers() {
        let t = targets(SpaceLayout::Surround714, &names_by_count(6));
        assert_eq!(
            t[..4],
            [
                Target::Speaker(0),
                Target::Speaker(1),
                Target::Speaker(2),
                Target::Speaker(3)
            ]
        );
        // Ls has no speaker of that name in 7.1.4: placed at the back
        // corner, which is the rear speaker.
        let Target::Spot(g) = t[4] else {
            panic!("{:?}", t[4])
        };
        assert!((g[4] - 1.0).abs() < 1e-6);
        let Target::Spot(g) = t[5] else {
            panic!("{:?}", t[5])
        };
        assert!((g[5] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_7_1_4_file_in_5_1_places_heights_and_2_0_drops_lfe() {
        let t = targets(SpaceLayout::Surround51, &names_by_count(12));
        // Top front left in 5.1 (no heights): the front left speaker.
        let Target::Spot(g) = t[8] else {
            panic!("{:?}", t[8])
        };
        assert!((g[0] - 1.0).abs() < 1e-6);
        // Side left: between front left and the left surround.
        let Target::Spot(g) = t[6] else {
            panic!("{:?}", t[6])
        };
        assert!((g[0] - g[4]).abs() < 1e-6 && g[0] > 0.7);
        let t = targets(SpaceLayout::Stereo20, &names_by_count(6));
        assert_eq!(t[3], Target::Drop);
        // Numbered channels go by index.
        let t = targets(SpaceLayout::Surround51, &names_by_count(9));
        assert_eq!(t[4], Target::Speaker(4));
        assert_eq!(t[8], Target::Drop);
    }
}
