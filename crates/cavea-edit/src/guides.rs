//! User guides (`docs/07-designer.md` §5.2): vertical and horizontal lines
//! dragged out of the rulers, per floor. They are editor state, not chart
//! data: the chart document has no place for them (03 §2), so the session
//! holds them and the shell saves them beside the draft as [`FloorGuides`].
//! Undo and redo do not touch them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::command::CommandError;

/// Guides on one floor, at most.
pub const MAX_GUIDES: usize = 256;

/// A line across the canvas: `x = at` for `vertical`, `y = at` for
/// `horizontal`, in world chart units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserGuide {
    pub axis: GuideAxis,
    pub at: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GuideAxis {
    /// From the top ruler: a vertical line, `x = at`.
    Vertical,
    /// From the left ruler: a horizontal line, `y = at`.
    Horizontal,
}

impl GuideAxis {
    /// The world axis the guide fixes: 0 for x, 1 for y.
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Vertical => 0,
            Self::Horizontal => 1,
        }
    }
}

/// One floor's guides, as the shell stores them beside the draft. `floor`
/// is `null` for the implicit floor of a chart without floors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FloorGuides {
    pub floor: Option<u32>,
    pub guides: Vec<UserGuide>,
}

#[derive(Debug, Default)]
pub(crate) struct Guides {
    floors: BTreeMap<Option<u32>, Vec<UserGuide>>,
}

fn check(g: &UserGuide) -> Result<(), CommandError> {
    if g.at.is_finite() {
        Ok(())
    } else {
        Err(CommandError::InvalidValue("guide"))
    }
}

impl Guides {
    pub(crate) fn on(&self, floor: Option<u32>) -> &[UserGuide] {
        self.floors.get(&floor).map_or(&[], Vec::as_slice)
    }

    /// Adds a guide at the end of the floor's list; returns its index.
    pub(crate) fn add(&mut self, floor: Option<u32>, g: UserGuide) -> Result<usize, CommandError> {
        check(&g)?;
        let list = self.floors.entry(floor).or_default();
        if list.len() >= MAX_GUIDES {
            return Err(CommandError::Limit("guides"));
        }
        list.push(g);
        Ok(list.len() - 1)
    }

    pub(crate) fn set(
        &mut self,
        floor: Option<u32>,
        i: usize,
        at: f64,
    ) -> Result<(), CommandError> {
        let g = self.floors.get_mut(&floor).and_then(|l| l.get_mut(i));
        let g = g.ok_or(CommandError::IndexOutOfRange)?;
        check(&UserGuide { axis: g.axis, at })?;
        g.at = at;
        Ok(())
    }

    pub(crate) fn remove(&mut self, floor: Option<u32>, i: usize) -> bool {
        match self.floors.get_mut(&floor) {
            Some(l) if i < l.len() => {
                l.remove(i);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn clear(&mut self, floor: Option<u32>) {
        self.floors.remove(&floor);
    }

    /// Every floor's guides, for `listed` floors, by floor id.
    pub(crate) fn saved(&self, listed: impl Fn(Option<u32>) -> bool) -> Vec<FloorGuides> {
        self.floors
            .iter()
            .filter(|(f, l)| listed(**f) && !l.is_empty())
            .map(|(&floor, l)| FloorGuides {
                floor,
                guides: l.clone(),
            })
            .collect()
    }

    /// Replaces every guide with saved ones, all or nothing: refused if any
    /// is not finite, or a floor has more than [`MAX_GUIDES`].
    pub(crate) fn restore(&mut self, saved: Vec<FloorGuides>) -> Result<(), CommandError> {
        let mut floors: BTreeMap<Option<u32>, Vec<UserGuide>> = BTreeMap::new();
        for f in saved {
            f.guides.iter().try_for_each(check)?;
            let list = floors.entry(f.floor).or_default();
            list.extend(f.guides);
            if list.len() > MAX_GUIDES {
                return Err(CommandError::Limit("guides"));
            }
        }
        self.floors = floors;
        Ok(())
    }
}
