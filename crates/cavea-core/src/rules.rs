//! Selection rules (`docs/02-domain-model.md` §12). The renderer runs them for
//! instant feedback and the server runs the same functions as the authority.
//! They take plain counts, so both callers pass what they hold: the renderer
//! its buyer's selection, the server a hold token's objects.
//!
//! Built so far: maximum and minimum selection. The orphan and consecutive-seat
//! rules (ENG-020, ENG-021) need the seats themselves; they will be further
//! fields of [`Rules`] and further [`Violation`]s, checked by functions that
//! also take the row segments and seat state.

use core::fmt;

/// The rules a selection must keep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    /// At most this many objects; `None` for no limit.
    max: Option<u32>,
    /// At least this many objects before checkout. The renderer only reports
    /// it: it never refuses a selection or a hold.
    min: u32,
}

/// A rule that a selection breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    /// More objects than [`Rules::max`].
    Max,
    /// Fewer objects than [`Rules::min`].
    Min,
}

impl Violation {
    /// The rule's stable name, as hosts see it.
    pub fn code(self) -> &'static str {
        match self {
            Self::Max => "max",
            Self::Min => "min",
        }
    }
}

/// Rules that cannot hold together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulesError {
    /// A maximum of zero: nothing could ever be selected.
    ZeroMax,
    /// A minimum above the maximum: checkout could never be allowed.
    MinAboveMax,
}

impl fmt::Display for RulesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ZeroMax => "maximum selection is zero",
            Self::MinAboveMax => "minimum selection is above the maximum",
        })
    }
}

impl std::error::Error for RulesError {}

impl Rules {
    pub fn new(max: Option<u32>, min: u32) -> Result<Self, RulesError> {
        match max {
            Some(0) => Err(RulesError::ZeroMax),
            Some(max) if min > max => Err(RulesError::MinAboveMax),
            _ => Ok(Self { max, min }),
        }
    }

    pub fn max(&self) -> Option<u32> {
        self.max
    }

    pub fn min(&self) -> u32 {
        self.min
    }

    /// Whether a selection of `count` objects may take `adding` more.
    pub fn check_add(&self, count: u32, adding: u32) -> Result<(), Violation> {
        match self.max {
            Some(max) if u64::from(count) + u64::from(adding) > u64::from(max) => {
                Err(Violation::Max)
            }
            _ => Ok(()),
        }
    }

    /// Whether a selection of `count` objects may go to checkout.
    pub fn check_checkout(&self, count: u32) -> Result<(), Violation> {
        if count < self.min {
            return Err(Violation::Min);
        }
        self.check_add(count, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximum_and_minimum() {
        assert_eq!(Rules::new(Some(0), 0), Err(RulesError::ZeroMax));
        assert_eq!(Rules::new(Some(2), 3), Err(RulesError::MinAboveMax));
        let rules = Rules::new(Some(4), 2).unwrap();
        assert_eq!(rules.check_add(3, 1), Ok(()));
        assert_eq!(rules.check_add(3, 2), Err(Violation::Max));
        assert_eq!(rules.check_add(u32::MAX, u32::MAX), Err(Violation::Max));
        assert_eq!(rules.check_checkout(1), Err(Violation::Min));
        assert_eq!(rules.check_checkout(2), Ok(()));
        assert_eq!(rules.check_checkout(5), Err(Violation::Max));
        let open = Rules::default();
        assert_eq!(open.check_add(u32::MAX, u32::MAX), Ok(()));
        assert_eq!(open.check_checkout(0), Ok(()));
    }
}
