//! The narrow view of a compiled chart that the engine needs: how many objects,
//! each one's `oid`, and label resolution. A later slice builds it from the CCF.

use std::collections::HashMap;

use crate::types::{Idx, ObjectRef, Oid};

/// Most objects a chart may have (02 §14).
pub const MAX_OBJECTS: usize = 1_000_000;

#[derive(Debug)]
pub struct ChartView {
    oids: Vec<Oid>,
    by_label: HashMap<Box<str>, Idx>,
    by_oid: HashMap<Oid, Idx>,
    /// The public artefact's content hash, sent in HELLO (08 §8.1).
    hash: [u8; 16],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChartError {
    TooManyObjects,
    DuplicateLabel(Box<str>),
    DuplicateOid(Oid),
}

impl std::fmt::Display for ChartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyObjects => write!(f, "chart has more than {MAX_OBJECTS} objects"),
            Self::DuplicateLabel(l) => write!(f, "duplicate label {l:?}"),
            Self::DuplicateOid(o) => write!(f, "duplicate oid {o}"),
        }
    }
}

impl std::error::Error for ChartError {}

impl ChartView {
    /// `objects[idx] = (oid, label)`.
    pub fn new<L: Into<Box<str>>>(
        objects: impl IntoIterator<Item = (Oid, L)>,
    ) -> Result<Self, ChartError> {
        let mut view = Self {
            oids: Vec::new(),
            by_label: HashMap::new(),
            by_oid: HashMap::new(),
            hash: [0; 16],
        };
        for (oid, label) in objects {
            let idx = Idx::try_from(view.oids.len()).map_err(|_| ChartError::TooManyObjects)?;
            if view.oids.len() >= MAX_OBJECTS {
                return Err(ChartError::TooManyObjects);
            }
            let label = label.into();
            if view.by_oid.insert(oid, idx).is_some() {
                return Err(ChartError::DuplicateOid(oid));
            }
            if view.by_label.contains_key(&label) {
                return Err(ChartError::DuplicateLabel(label));
            }
            view.by_label.insert(label, idx);
            view.oids.push(oid);
        }
        Ok(view)
    }

    /// Set the public artefact's content hash (03 §4.1), which HELLO carries
    /// so that a client can check it has the right chart. Zero until set.
    #[must_use]
    pub fn with_hash(mut self, hash: [u8; 16]) -> Self {
        self.hash = hash;
        self
    }

    #[must_use]
    pub fn hash(&self) -> [u8; 16] {
        self.hash
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.oids.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.oids.is_empty()
    }

    #[must_use]
    pub fn oid(&self, idx: Idx) -> Option<Oid> {
        self.oids.get(idx as usize).copied()
    }

    #[must_use]
    pub fn idx_of_oid(&self, oid: Oid) -> Option<Idx> {
        self.by_oid.get(&oid).copied()
    }

    #[must_use]
    pub fn resolve(&self, object: &ObjectRef) -> Option<Idx> {
        match object {
            ObjectRef::Label(label) => self.by_label.get(label).copied(),
            ObjectRef::Idx(idx) => ((*idx as usize) < self.oids.len()).then_some(*idx),
        }
    }
}
