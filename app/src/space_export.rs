// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! What a 3D Space export writes: the speaker bed in any layout (chosen
//! per export, whatever the session's own), a stereo or binaural file,
//! and the channel order of a 7.x bed (WAV speaker-mask order, or the film
//! order with sides before rears).

use af27_core::space::{Monitor, SpaceLayout};
use af27_core::Session;
use af27_io::export::Container;

/// What the bed export writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BedTarget {
    /// The session's own layout.
    #[default]
    Session,
    Layout(SpaceLayout),
    /// Two channels, as heard on headphones.
    Binaural,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BedOptions {
    pub target: BedTarget,
    /// 7.x beds: sides before rears, no speaker mask.
    pub film_order: bool,
}

impl BedOptions {
    /// The layout written, or None for binaural.
    pub fn layout(&self, session: Option<SpaceLayout>) -> Option<SpaceLayout> {
        match self.target {
            BedTarget::Session => Some(session.unwrap_or(SpaceLayout::Surround51)),
            BedTarget::Layout(l) => Some(l),
            BedTarget::Binaural => None,
        }
    }

    pub fn channels(&self, session: Option<SpaceLayout>) -> usize {
        self.layout(session)
            .map_or(2, |l| af27_space::pan::speakers(l).len())
    }

    fn film(&self, layout: Option<SpaceLayout>) -> Option<Vec<usize>> {
        layout
            .filter(|_| self.film_order)
            .and_then(af27_space::pan::film_order)
    }

    /// Readies `s` (a copy of the session) for the render. Returns whether
    /// it renders the bed (else the stereo listening mix), the WAV speaker
    /// mask, and the channel order.
    pub fn prepare(&self, s: &mut Session) -> (bool, Option<u32>, Option<Vec<usize>>) {
        let layout = self.layout(s.space);
        match layout {
            None => {
                s.space = s.space.or(Some(SpaceLayout::Surround714));
                s.monitor = Monitor::Binaural;
                (false, None, None)
            }
            Some(l) => {
                s.space = Some(l);
                s.monitor = Monitor::StereoFold;
                let order = self.film(Some(l));
                // Film order is not the speaker-mask order: no mask.
                let mask = if order.is_some() {
                    0
                } else {
                    af27_space::pan::channel_mask(l)
                };
                (true, Some(mask), order)
            }
        }
    }

    /// Why this export cannot be written in `container`, if it cannot.
    pub fn refuse(&self, session: Option<SpaceLayout>, container: Container) -> Option<String> {
        let l = self.layout(session)?;
        (container == Container::Flac && !af27_space::pan::flac_ok(l)).then(|| {
            format!(
                "FLAC knows the channel order of 2.0, 5.1 and 7.1 only; write {} as WAV.",
                l.label()
            )
        })
    }

    /// The dialog rows (in its two-column grid).
    pub fn ui(&mut self, ui: &mut egui::Ui, session: Option<SpaceLayout>) {
        let label = |t: BedTarget| match t {
            BedTarget::Session => format!(
                "Session layout ({})",
                session.map_or("5.1", SpaceLayout::label)
            ),
            BedTarget::Layout(l) => l.label().to_string(),
            BedTarget::Binaural => "Headphones (binaural stereo)".to_string(),
        };
        ui.label("Speakers");
        egui::ComboBox::from_id_salt("export-bed-target")
            .selected_text(label(self.target))
            .width(220.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.target,
                    BedTarget::Session,
                    label(BedTarget::Session),
                );
                for l in SpaceLayout::ALL {
                    let t = BedTarget::Layout(l);
                    ui.selectable_value(&mut self.target, t, label(t));
                }
                ui.selectable_value(
                    &mut self.target,
                    BedTarget::Binaural,
                    label(BedTarget::Binaural),
                );
            })
            .response
            .on_hover_text("Spots are kept in the room, so any layout can be written");
        ui.end_row();
        if self
            .layout(session)
            .and_then(af27_space::pan::film_order)
            .is_some()
        {
            ui.label("Channel order");
            ui.vertical(|ui| {
                ui.radio_value(&mut self.film_order, false, "WAV order (rears before sides)")
                    .on_hover_text("L R C LFE Lrs Rrs Lss Rss ..., with the speaker mask: any reader that honours the mask places every channel");
                ui.radio_value(&mut self.film_order, true, "Film order (sides before rears)")
                    .on_hover_text("L R C LFE Lss Rss Lrs Rrs ..., as many film tools expect; written without a speaker mask");
            });
            ui.end_row();
        }
        if self.channels(session) > 2 {
            ui.label("");
            ui.small("Players in web browsers and chat apps fold every channel into stereo without turning it down, so a speaker bed can play loud there. 24-bit is the safest to share.");
            ui.end_row();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_pick_their_own_layout_and_order() {
        let mut s = Session::new("t", 48_000, 1);
        s.space = Some(SpaceLayout::Surround51);
        s.monitor = Monitor::Binaural;
        let o = BedOptions::default();
        assert_eq!(o.channels(s.space), 6);
        let (bed, mask, order) = o.prepare(&mut s);
        assert!(bed && order.is_none());
        assert_eq!(mask, Some(0x3F));
        assert_eq!(s.monitor, Monitor::StereoFold);

        let o = BedOptions {
            target: BedTarget::Layout(SpaceLayout::Surround714),
            film_order: true,
        };
        assert_eq!(o.channels(s.space), 12);
        let (bed, mask, order) = o.prepare(&mut s);
        assert!(bed);
        assert_eq!(mask, Some(0));
        assert_eq!(order.unwrap()[4], 6);
        assert_eq!(s.space, Some(SpaceLayout::Surround714));
        assert!(o.refuse(s.space, Container::Flac).is_some());
        assert!(o.refuse(s.space, Container::Wav).is_none());

        let o = BedOptions {
            target: BedTarget::Binaural,
            film_order: false,
        };
        assert_eq!(o.channels(s.space), 2);
        let (bed, _, _) = o.prepare(&mut s);
        assert!(!bed);
        assert_eq!(s.monitor, Monitor::Binaural);
        assert!(o.refuse(s.space, Container::Flac).is_none());
    }
}
