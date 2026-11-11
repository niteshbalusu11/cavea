//! The engine's [`Reply`] as stored in `idempotency.response` (08 §5, §6.1):
//! JSON that decodes to exactly the reply that was encoded, so a retry after a
//! reload returns the original result.

use cavea_engine::{
    Failure, HoldId, Idx, InvalidReason, Missing, ObjectFailure, ObjectReason, ObjectRef, Outcome,
    Reply, Timestamp,
};
use serde_json::{Map, Value, json};

/// `idempotency.status`: the reply's HTTP status, for reports. Loading reads
/// only `response`.
pub(super) fn status(reply: &Reply) -> i32 {
    match reply {
        Ok(_) => 200,
        Err(failure) => i32::from(failure.http_status()),
    }
}

pub(super) fn encode(reply: &Reply) -> String {
    let value = match reply {
        Ok(outcome) => match outcome {
            Outcome::HoldCreated {
                seq,
                hold,
                expires_at,
            } => {
                json!({"ok": "hold_created", "seq": seq, "hold": hold.0, "expires_at": expires_at.0})
            }
            Outcome::Held {
                seq,
                expires_at,
                objects,
            } => json!({"ok": "held", "seq": seq, "expires_at": expires_at.0, "objects": objects}),
            Outcome::Released { seq, objects } => {
                json!({"ok": "released", "seq": seq, "objects": objects})
            }
            Outcome::Extended { seq, expires_at } => {
                json!({"ok": "extended", "seq": seq, "expires_at": expires_at.0})
            }
            Outcome::Booked { seq, objects } => {
                json!({"ok": "booked", "seq": seq, "objects": objects})
            }
            Outcome::StatusChanged { seq, objects } => {
                json!({"ok": "status_changed", "seq": seq, "objects": objects})
            }
        },
        Err(failure) => {
            let mut map = Map::new();
            map.insert("error".into(), failure.code().into());
            match failure {
                Failure::InvalidRequest(reason) => {
                    map.insert("reason".into(), invalid_code(*reason).into());
                }
                Failure::NotFound(Missing::HoldToken) => {
                    map.insert("missing".into(), "hold_token".into());
                }
                Failure::NotFound(Missing::Objects(refs)) => {
                    let refs = refs.iter().map(|r| match r {
                        ObjectRef::Label(label) => json!({"label": &**label}),
                        ObjectRef::Idx(idx) => json!({"idx": idx}),
                    });
                    map.insert("objects".into(), refs.collect());
                }
                Failure::ObjectNotFree(objects)
                | Failure::ObjectNotHeldByToken(objects)
                | Failure::ObjectStatusConflict(objects) => {
                    let objects = objects.iter().map(|o| json!([o.idx, o.reason.code()]));
                    map.insert("objects".into(), objects.collect());
                }
                Failure::HoldLimitExceeded { limit, requested } => {
                    map.insert("limit".into(), (*limit).into());
                    map.insert("requested".into(), (*requested).into());
                }
                Failure::Forbidden
                | Failure::IdempotencyMismatch
                | Failure::HoldTokenExpired
                | Failure::Overloaded
                | Failure::Unavailable => {}
            }
            Value::Object(map)
        }
    };
    value.to_string()
}

/// `None` if the text is not a reply this module wrote.
pub fn decode(text: &str) -> Option<Reply> {
    let v: Value = serde_json::from_str(text).ok()?;
    let u64_of = |key: &str| v.get(key)?.as_u64();
    let seq = || u64_of("seq");
    let expires_at = || u64_of("expires_at").map(Timestamp);
    let idxs = || -> Option<Vec<Idx>> {
        v.get("objects")?
            .as_array()?
            .iter()
            .map(|i| i.as_u64().and_then(|i| Idx::try_from(i).ok()))
            .collect()
    };
    if let Some(ok) = v.get("ok") {
        let outcome = match ok.as_str()? {
            "hold_created" => Outcome::HoldCreated {
                seq: seq()?,
                hold: HoldId(u64_of("hold")?),
                expires_at: expires_at()?,
            },
            "held" => Outcome::Held {
                seq: seq()?,
                expires_at: expires_at()?,
                objects: idxs()?,
            },
            "released" => Outcome::Released {
                seq: seq()?,
                objects: idxs()?,
            },
            "extended" => Outcome::Extended {
                seq: seq()?,
                expires_at: expires_at()?,
            },
            "booked" => Outcome::Booked {
                seq: seq()?,
                objects: idxs()?,
            },
            "status_changed" => Outcome::StatusChanged {
                seq: seq()?,
                objects: idxs()?,
            },
            _ => return None,
        };
        return Some(Ok(outcome));
    }
    let object_failures = || -> Option<Vec<ObjectFailure>> {
        v.get("objects")?
            .as_array()?
            .iter()
            .map(|pair| {
                let idx = Idx::try_from(pair.get(0)?.as_u64()?).ok()?;
                let code = pair.get(1)?.as_str()?;
                let reason = [
                    ObjectReason::NotFree,
                    ObjectReason::NotHeldByToken,
                    ObjectReason::NotBooked,
                    ObjectReason::NotBlocked,
                ]
                .into_iter()
                .find(|r| r.code() == code)?;
                Some(ObjectFailure { idx, reason })
            })
            .collect()
    };
    let failure = match v.get("error")?.as_str()? {
        "invalid_request" => Failure::InvalidRequest(invalid_reason(v.get("reason")?.as_str()?)?),
        "forbidden" => Failure::Forbidden,
        "not_found" => match v.get("missing") {
            Some(m) if m.as_str()? == "hold_token" => Failure::NotFound(Missing::HoldToken),
            Some(_) => return None,
            None => {
                let refs = v.get("objects")?.as_array()?.iter().map(|r| {
                    if let Some(label) = r.get("label") {
                        Some(ObjectRef::Label(label.as_str()?.into()))
                    } else {
                        Some(ObjectRef::Idx(Idx::try_from(r.get("idx")?.as_u64()?).ok()?))
                    }
                });
                Failure::NotFound(Missing::Objects(refs.collect::<Option<_>>()?))
            }
        },
        "object_not_free" => Failure::ObjectNotFree(object_failures()?),
        "object_not_held_by_token" => Failure::ObjectNotHeldByToken(object_failures()?),
        "object_status_conflict" => Failure::ObjectStatusConflict(object_failures()?),
        "idempotency_mismatch" => Failure::IdempotencyMismatch,
        "hold_token_expired" => Failure::HoldTokenExpired,
        "hold_limit_exceeded" => Failure::HoldLimitExceeded {
            limit: u32::try_from(u64_of("limit")?).ok()?,
            requested: u64_of("requested")?,
        },
        "overloaded" => Failure::Overloaded,
        "unavailable" => Failure::Unavailable,
        _ => return None,
    };
    Some(Err(failure))
}

const INVALID_REASONS: [InvalidReason; 11] = [
    InvalidReason::NoObjects,
    InvalidReason::TooManyObjects,
    InvalidReason::TtlOutOfRange,
    InvalidReason::ZeroObjectLimit,
    InvalidReason::TokenAlreadyExists,
    InvalidReason::OrderRefLength,
    InvalidReason::TicketTypeLength,
    InvalidReason::ExtraTooLarge,
    InvalidReason::DetailMismatch,
    InvalidReason::BadStatus,
    InvalidReason::IdempotencyKeyLength,
];

/// Exhaustive, so a reason added to the engine fails to compile here until it
/// is also added to [`INVALID_REASONS`], which decoding searches.
fn invalid_code(reason: InvalidReason) -> &'static str {
    match reason {
        InvalidReason::NoObjects
        | InvalidReason::TooManyObjects
        | InvalidReason::TtlOutOfRange
        | InvalidReason::ZeroObjectLimit
        | InvalidReason::TokenAlreadyExists
        | InvalidReason::OrderRefLength
        | InvalidReason::TicketTypeLength
        | InvalidReason::ExtraTooLarge
        | InvalidReason::DetailMismatch
        | InvalidReason::BadStatus
        | InvalidReason::IdempotencyKeyLength => reason.code(),
    }
}

fn invalid_reason(code: &str) -> Option<InvalidReason> {
    INVALID_REASONS.into_iter().find(|r| r.code() == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every reply shape survives the round trip unchanged.
    #[test]
    fn replies_round_trip() {
        let f = |idx, reason| ObjectFailure { idx, reason };
        let mut replies: Vec<Reply> = vec![
            Ok(Outcome::HoldCreated {
                seq: 1,
                hold: HoldId(1),
                expires_at: Timestamp(u64::MAX),
            }),
            Ok(Outcome::Held {
                seq: 2,
                expires_at: Timestamp(5),
                objects: vec![0, u32::MAX],
            }),
            Ok(Outcome::Released {
                seq: 3,
                objects: vec![],
            }),
            Ok(Outcome::Extended {
                seq: 4,
                expires_at: Timestamp(6),
            }),
            Ok(Outcome::Booked {
                seq: 5,
                objects: vec![7],
            }),
            Ok(Outcome::StatusChanged {
                seq: 6,
                objects: vec![8, 9],
            }),
            Err(Failure::Forbidden),
            Err(Failure::NotFound(Missing::HoldToken)),
            Err(Failure::NotFound(Missing::Objects(vec![
                ObjectRef::Label("Orchestra-C-12 \"é\"".into()),
                ObjectRef::Idx(3),
            ]))),
            Err(Failure::ObjectNotFree(vec![f(1, ObjectReason::NotFree)])),
            Err(Failure::ObjectNotHeldByToken(vec![f(
                2,
                ObjectReason::NotHeldByToken,
            )])),
            Err(Failure::ObjectStatusConflict(vec![
                f(3, ObjectReason::NotBooked),
                f(4, ObjectReason::NotBlocked),
            ])),
            Err(Failure::IdempotencyMismatch),
            Err(Failure::HoldTokenExpired),
            Err(Failure::HoldLimitExceeded {
                limit: 10,
                requested: 11,
            }),
            Err(Failure::Overloaded),
            Err(Failure::Unavailable),
        ];
        replies.extend(INVALID_REASONS.map(|r| Err(Failure::InvalidRequest(r))));
        for reply in replies {
            assert_eq!(decode(&encode(&reply)), Some(reply));
        }
    }
}
