//! A buyer's view of availability, built only from the frames it receives
//! (08 §8), and how it picks seats from it.

use cavea_format::wire::{ServerFrame, Snapshot, WireError};

/// SplitMix64: small, seeded, reproducible.
#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`; 0 when `n` is 0.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p && p > 0.0
    }
}

/// Why a frame could not be applied: each is a protocol violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewError {
    Wire(WireError),
    /// A DELTA that does not continue the view's `seq`, outside a RESYNC.
    Gap {
        have: u64,
        from: u64,
    },
    /// A DELTA between RESYNC and the SNAPSHOT that must follow it.
    DeltaDuringResync,
    /// A HELLO or SNAPSHOT for another number of objects.
    Objects {
        have: u32,
        got: u32,
    },
    /// A SNAPSHOT behind the `seq` the view had already reached: a state it
    /// was shown has been lost (published, so committed, 08 §4.5).
    Regressed {
        have: u64,
        got: u64,
    },
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wire(e) => write!(f, "undecodable frame: {e}"),
            Self::Gap { have, from } => write!(f, "DELTA from seq {from} applied at seq {have}"),
            Self::DeltaDuringResync => f.write_str("DELTA after RESYNC before its SNAPSHOT"),
            Self::Objects { have, got } => write!(f, "frame for {got} objects, chart has {have}"),
            Self::Regressed { have, got } => {
                write!(f, "SNAPSHOT at seq {got} after the view reached seq {have}")
            }
        }
    }
}

/// What the receiver must do after a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    Nothing,
    Hello,
    /// A DELTA moved the view from `from` to `to`.
    Delta {
        from: u64,
        to: u64,
    },
    Snapshot,
    Resync,
    Pong(u64),
    Close(u8),
}

/// Public codes, one bit per object (1: unavailable), and the `seq` they are at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatView {
    bits: Vec<u64>,
    objects: u32,
    pub seq: u64,
    resyncing: bool,
}

impl SeatView {
    pub fn new(objects: u32) -> Self {
        Self {
            bits: vec![0; (objects as usize).div_ceil(64)],
            objects,
            seq: 0,
            resyncing: false,
        }
    }

    pub fn available(&self, idx: u32) -> bool {
        idx < self.objects
            && self
                .bits
                .get(idx as usize / 64)
                .is_some_and(|w| w >> (idx % 64) & 1 == 0)
    }

    fn set(&mut self, idx: u32, unavailable: bool) {
        if let Some(w) = self.bits.get_mut(idx as usize / 64) {
            let bit = 1u64 << (idx % 64);
            if unavailable { *w |= bit } else { *w &= !bit }
        }
    }

    pub fn load(&mut self, s: &Snapshot<'_>) -> Result<(), ViewError> {
        if s.objects != self.objects {
            return Err(ViewError::Objects {
                have: self.objects,
                got: s.objects,
            });
        }
        for (range, code) in s.runs(self.objects).map_err(ViewError::Wire)? {
            for idx in range {
                self.set(idx, code != 0);
            }
        }
        self.seq = s.seq;
        self.resyncing = false;
        Ok(())
    }

    /// Apply one decoded server frame.
    pub fn apply(&mut self, frame: &ServerFrame<'_>) -> Result<Applied, ViewError> {
        Ok(match frame {
            ServerFrame::Hello(h) if h.objects != self.objects => {
                return Err(ViewError::Objects {
                    have: self.objects,
                    got: h.objects,
                });
            }
            ServerFrame::Hello(_) => Applied::Hello,
            ServerFrame::Private(_) => Applied::Nothing,
            ServerFrame::Snapshot(s) => {
                if s.seq < self.seq {
                    return Err(ViewError::Regressed {
                        have: self.seq,
                        got: s.seq,
                    });
                }
                self.load(s)?;
                Applied::Snapshot
            }
            ServerFrame::Delta(d) => {
                if self.resyncing {
                    return Err(ViewError::DeltaDuringResync);
                }
                if d.from_seq != self.seq {
                    return Err(ViewError::Gap {
                        have: self.seq,
                        from: d.from_seq,
                    });
                }
                for c in d.changes(self.objects).map_err(ViewError::Wire)? {
                    self.set(c.idx, c.code != 0);
                }
                self.seq = d.to_seq;
                Applied::Delta {
                    from: d.from_seq,
                    to: d.to_seq,
                }
            }
            ServerFrame::Resync => {
                self.resyncing = true;
                Applied::Resync
            }
            ServerFrame::Ping(n) => Applied::Pong(*n),
            ServerFrame::Close(r) => Applied::Close(r.0),
        })
    }

    /// The connection was lost: the next one resumes from `seq`, whose state
    /// the view still has, so a RESYNC on the old one is moot.
    pub fn reconnecting(&mut self) {
        self.resyncing = false;
    }

    /// Indices that differ from `other`, at most `limit` of them.
    pub fn differences(&self, other: &SeatView, limit: usize) -> Vec<u32> {
        (0..self.objects)
            .filter(|&i| self.available(i) != other.available(i))
            .take(limit)
            .collect()
    }

    /// `k` seats this view shows available: adjacent in one segment of a row
    /// (`clustered`, what real buyers ask for), or anywhere (`random`). `None`
    /// when a few tries find nothing.
    pub fn pick(
        &self,
        segments: &[(u32, u32)],
        k: u32,
        clustered: bool,
        rng: &mut Rng,
    ) -> Option<Vec<u32>> {
        if k == 0 {
            return None;
        }
        if !clustered {
            let mut out: Vec<u32> = Vec::new();
            for _ in 0..64 * k {
                let idx = rng.below(u64::from(self.objects)) as u32;
                if self.available(idx) && !out.contains(&idx) {
                    out.push(idx);
                    if out.len() == k as usize {
                        out.sort_unstable();
                        return Some(out);
                    }
                }
            }
            return None;
        }
        let fits = |first: u32| (first..first + k).all(|i| self.available(i));
        let wide: Vec<&(u32, u32)> = segments.iter().filter(|s| s.1 >= k).collect();
        for _ in 0..32 {
            let (first, count) = **wide.get(rng.below(wide.len() as u64) as usize)?;
            let start = first + rng.below(u64::from(count - k + 1)) as u32;
            if fits(start) {
                return Some((start..start + k).collect());
            }
        }
        // Crowded: walk every segment from a random one.
        let from = rng.below(wide.len() as u64) as usize;
        for &&(first, count) in wide[from..].iter().chain(wide[..from].iter()) {
            if let Some(start) = (first..=first + count - k).find(|&s| fits(s)) {
                return Some((start..start + k).collect());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use cavea_format::wire::{self, Change, View};

    use super::*;

    fn frame(bytes: &[u8]) -> ServerFrame<'_> {
        wire::decode_server(bytes, View::Public).unwrap()
    }

    /// A snapshot and the deltas after it leave the view equal to the public
    /// reduction of the domain statuses applied directly; a delta that skips a
    /// `seq`, or comes between RESYNC and SNAPSHOT, is refused.
    #[test]
    fn snapshot_plus_deltas_equals_direct_application() {
        let n = 300u32;
        let mut rng = Rng(5);
        let mut statuses: Vec<u8> = (0..n)
            .map(|_| [0, 0, 1, 2, 3, 7][rng.below(6) as usize])
            .collect();
        let mut view = SeatView::new(n);
        let snap = wire::encode_snapshot(View::Public, 10, &statuses, &[]).unwrap();
        assert_eq!(view.apply(&frame(&snap)), Ok(Applied::Snapshot));
        let mut seq = 10;
        for _ in 0..50 {
            let mut changes: Vec<Change> = (0..rng.below(20))
                .map(|_| Change {
                    idx: rng.below(u64::from(n)) as u32,
                    code: [0, 1, 2, 3][rng.below(4) as usize],
                })
                .collect();
            for c in &changes {
                statuses[c.idx as usize] = c.code;
            }
            wire::coalesce(&mut changes);
            let to = seq + 1 + rng.below(3);
            let delta = wire::encode_delta(View::Public, seq, to, &changes, n, &[]).unwrap();
            assert_eq!(
                view.apply(&frame(&delta)),
                Ok(Applied::Delta { from: seq, to })
            );
            seq = to;
            for (i, s) in statuses.iter().enumerate() {
                assert_eq!(view.available(i as u32), *s == 0, "idx {i}");
            }
        }
        let skip = wire::encode_delta(View::Public, seq + 1, seq + 2, &[], n, &[]).unwrap();
        assert_eq!(
            view.apply(&frame(&skip)),
            Err(ViewError::Gap {
                have: seq,
                from: seq + 1
            })
        );
        assert_eq!(
            view.apply(&frame(&wire::encode_resync())),
            Ok(Applied::Resync)
        );
        let next = wire::encode_delta(View::Public, seq, seq + 1, &[], n, &[]).unwrap();
        assert_eq!(view.apply(&frame(&next)), Err(ViewError::DeltaDuringResync));
        let snap = wire::encode_snapshot(View::Public, seq + 5, &vec![0; n as usize], &[]).unwrap();
        view.apply(&frame(&snap)).unwrap();
        assert_eq!(
            (view.seq, (0..n).all(|i| view.available(i))),
            (seq + 5, true)
        );
        // A lost connection resumes from `seq`: DELTAs from it apply even
        // after a RESYNC on the old one; a SNAPSHOT behind it is refused.
        view.apply(&frame(&wire::encode_resync())).unwrap();
        view.reconnecting();
        let next = wire::encode_delta(View::Public, seq + 5, seq + 6, &[], n, &[]).unwrap();
        assert!(view.apply(&frame(&next)).is_ok());
        let old = wire::encode_snapshot(View::Public, seq + 4, &vec![0; n as usize], &[]).unwrap();
        assert_eq!(
            view.apply(&frame(&old)),
            Err(ViewError::Regressed {
                have: seq + 6,
                got: seq + 4
            })
        );
    }

    /// Clustered picks are `k` adjacent available seats inside one segment;
    /// random picks are available; a full view gives nothing.
    #[test]
    fn picks_only_available_seats() {
        let segments = [(0, 10), (10, 3), (13, 20)];
        let mut statuses = vec![0u8; 33];
        for i in [1, 4, 8, 11, 14, 20, 25, 30] {
            statuses[i] = 2;
        }
        let mut view = SeatView::new(33);
        view.apply(&frame(
            &wire::encode_snapshot(View::Public, 1, &statuses, &[]).unwrap(),
        ))
        .unwrap();
        let mut rng = Rng(9);
        for k in 2..=4 {
            for _ in 0..200 {
                let p = view.pick(&segments, k, true, &mut rng).unwrap();
                assert_eq!(p.len(), k as usize);
                assert!(p.windows(2).all(|w| w[1] == w[0] + 1), "{p:?}");
                assert!(
                    segments
                        .iter()
                        .any(|&(f, c)| p[0] >= f && p[k as usize - 1] < f + c),
                    "{p:?}"
                );
                assert!(p.iter().all(|&i| statuses[i as usize] == 0), "{p:?}");
                let r = view.pick(&segments, k, false, &mut rng).unwrap();
                assert!(r.iter().all(|&i| statuses[i as usize] == 0), "{r:?}");
            }
        }
        // Only seats 15 to 19 have four in a row.
        let only = [(13, 7)];
        assert_eq!(
            view.pick(&only, 4, true, &mut rng).map(|p| p[0] >= 15),
            Some(true)
        );
        assert_eq!(view.pick(&only, 6, true, &mut rng), None);
        let full = SeatView {
            bits: vec![u64::MAX],
            ..view
        };
        assert_eq!(full.pick(&segments, 2, true, &mut rng), None);
        assert_eq!(full.pick(&segments, 2, false, &mut rng), None);
    }
}
