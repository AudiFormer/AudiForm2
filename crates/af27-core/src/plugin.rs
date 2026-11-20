// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! What an effect slot holding a VST3 plug-in keeps (in `EffectSlot.data`,
//! as JSON): which running copy it is, the plug-in's name for when it is
//! missing, its control values (keys `p<param id>`, normalized 0..1, as
//! Audiform sessions store them) and its settings blob (base64, the form
//! Audiform sessions keep in `plugin_state`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// The effect id prefix of a plug-in slot: `vst3:<32 hex class id>`.
pub const PREFIX: &str = "vst3:";

pub fn is_plugin(effect: &str) -> bool {
    effect.starts_with(PREFIX)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginData {
    /// This slot's running copy (unique per slot; copies of a slot get a
    /// new one).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub inst: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vendor: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

pub fn param_key(id: u32) -> String {
    format!("p{id}")
}

pub fn param_id(key: &str) -> Option<u32> {
    key.strip_prefix('p')?.parse().ok()
}

/// A fresh id for a running copy.
pub fn new_inst() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!("{:016x}{:08x}{:04x}", t, std::process::id(), n & 0xffff)
}

impl PluginData {
    pub fn parse(data: Option<&str>) -> PluginData {
        data.and_then(|d| serde_json::from_str(d).ok())
            .unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn param(&self, id: u32) -> Option<f64> {
        self.params.get(&param_key(id)).copied()
    }

    pub fn set_param(&mut self, id: u32, v: f64) {
        self.params.insert(param_key(id), v);
    }

    /// The control values as (id, value).
    pub fn pairs(&self) -> Vec<(u32, f64)> {
        self.params
            .iter()
            .filter_map(|(k, v)| Some((param_id(k)?, *v)))
            .collect()
    }
}

impl crate::session::EffectSlot {
    /// A plug-in slot's data (empty for other effects).
    pub fn plugin_data(&self) -> PluginData {
        PluginData::parse(self.data.as_deref())
    }

    pub fn set_plugin_data(&mut self, d: &PluginData) {
        self.data = Some(d.to_json());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_data_round_trips_and_ids_are_unique() {
        let mut d = PluginData {
            inst: new_inst(),
            name: "Gain".into(),
            ..Default::default()
        };
        d.set_param(7, 0.25);
        d.state = Some("AAAA".into());
        let back = PluginData::parse(Some(&d.to_json()));
        assert_eq!(back, d);
        assert_eq!(back.pairs(), vec![(7, 0.25)]);
        assert_eq!(param_id("p42"), Some(42));
        assert_eq!(param_id("x"), None);
        assert_ne!(new_inst(), new_inst());
        assert_eq!(PluginData::parse(Some("not json")), PluginData::default());
    }
}
