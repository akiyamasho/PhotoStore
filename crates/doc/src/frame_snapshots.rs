//! Copy-on-write frame metadata. Serialization remains the existing array-of-arrays format.
use crate::FrameLayerState;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    ops::{Deref, DerefMut},
    sync::{Arc, OnceLock},
};

#[derive(Debug, Default)]
struct Bank {
    rows: Vec<Vec<FrameLayerState>>,
    bytes: OnceLock<usize>,
}
impl Clone for Bank {
    fn clone(&self) -> Self {
        Self { rows: self.rows.clone(), bytes: OnceLock::new() }
    }
}

/// Document/history clones share authored frame banks. An edit detaches before mutation;
/// playback reads the bank without copying every layer snapshot at every presentation.
#[derive(Clone, Debug, Default)]
pub struct FrameSnapshots(Arc<Bank>);
impl FrameSnapshots {
    pub fn allocation_key(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
    /// Cached allocation estimate for history/resource accounting, excluding shared pixel tiles.
    pub fn retained_bytes(&self) -> usize {
        *self.0.bytes.get_or_init(|| {
            let own = std::mem::size_of::<Bank>().saturating_add(self.0.rows.capacity().saturating_mul(std::mem::size_of::<Vec<FrameLayerState>>()));
            self.0.rows.iter().fold(own, |sum, row| {
                let inline = row.capacity().saturating_mul(std::mem::size_of::<FrameLayerState>());
                let effects = row
                    .iter()
                    .filter_map(|state| state.effects.as_ref())
                    .fold(0usize, |sum, effects| sum.saturating_add(effects.items.capacity().saturating_mul(512)));
                sum.saturating_add(inline).saturating_add(effects)
            })
        })
    }
}
impl PartialEq for FrameSnapshots {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0.rows == other.0.rows
    }
}
impl Deref for FrameSnapshots {
    type Target = Vec<Vec<FrameLayerState>>;
    fn deref(&self) -> &Self::Target {
        &self.0.rows
    }
}
impl DerefMut for FrameSnapshots {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let bank = Arc::make_mut(&mut self.0);
        bank.bytes.take();
        &mut bank.rows
    }
}
impl From<Vec<Vec<FrameLayerState>>> for FrameSnapshots {
    fn from(rows: Vec<Vec<FrameLayerState>>) -> Self {
        Self(Arc::new(Bank { rows, bytes: OnceLock::new() }))
    }
}
impl FromIterator<Vec<FrameLayerState>> for FrameSnapshots {
    fn from_iter<T: IntoIterator<Item = Vec<FrameLayerState>>>(rows: T) -> Self {
        Vec::from_iter(rows).into()
    }
}
impl<'a> IntoIterator for &'a FrameSnapshots {
    type Item = &'a Vec<FrameLayerState>;
    type IntoIter = std::slice::Iter<'a, Vec<FrameLayerState>>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl<'a> IntoIterator for &'a mut FrameSnapshots {
    type Item = &'a mut Vec<FrameLayerState>;
    type IntoIter = std::slice::IterMut<'a, Vec<FrameLayerState>>;
    fn into_iter(self) -> Self::IntoIter {
        self.deref_mut().iter_mut()
    }
}
impl IntoIterator for FrameSnapshots {
    type Item = Vec<FrameLayerState>;
    type IntoIter = std::vec::IntoIter<Vec<FrameLayerState>>;
    fn into_iter(self) -> Self::IntoIter {
        match Arc::try_unwrap(self.0) {
            Ok(bank) => bank.rows,
            Err(bank) => bank.rows.clone(),
        }
        .into_iter()
    }
}
impl Serialize for FrameSnapshots {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.rows.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for FrameSnapshots {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<Vec<FrameLayerState>>::deserialize(deserializer).map(Into::into)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn scalable_playback_snapshot_banks_share_detach_and_serialize_as_arrays() {
        let state = crate::FrameLayerState::legacy(crate::LayerId::fresh(), true, 1.0);
        let original = super::FrameSnapshots::from(vec![vec![state]]);
        let mut copy = original.clone();
        assert_eq!(original.allocation_key(), copy.allocation_key());
        assert!(original.retained_bytes() > 0);
        copy[0][0].visible = false;
        assert_ne!(original.allocation_key(), copy.allocation_key());
        assert!(original[0][0].visible);
        let json = serde_json::to_string(&copy).unwrap();
        assert!(json.starts_with("[["));
        let back: super::FrameSnapshots = serde_json::from_str(&json).unwrap();
        assert_eq!(back, copy);
    }
}
