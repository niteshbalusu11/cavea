//! Model-based property tests (13 §3.1): generated command sequences run
//! against the reference model and the real `EventState`, compared after
//! every step; a failure shrinks to a small case written in a line-based text
//! format that `cases/*.case` files hold and [`Case::parse`] reads back.
//!
//! The format, one step per line (`#` starts a comment):
//!
//! ```text
//! objects 8
//! s create t1 ttl=900 max=2          # s: secret, p: publishable
//! p hold t1 S-1 S-2 #3 X-9           # label, #idx, an unknown label
//! s book t1 order=o1 detail=S-1:adult:10 key=k1:a
//! s book order=~nul S-4*1001         # ~empty ~long ~nul; ref*count
//! s release t1                       # no objects: all of the token's
//! s unbook S-1 | s status 3 S-2 | s extend t1 ttl=60
//! advance 900000 | advance -5 | seal | reload
//! ```

use std::sync::Arc;

use cavea_engine::{
    Authority, BookDetail, Command, EventState, Idempotency, LoadedEvent, MemoryStore, ObjectRef,
    Request, SeatStore, Status, Timestamp, TokenHash,
};

use crate::model::{Model, Replay};
use crate::{EVENT, MAX_OBJECTS, Rng, Stats, Violation, chart, ensure, poll_now};

/// Where every run's clock starts.
pub const T0: Timestamp = Timestamp(1_000_000_000);

#[derive(Debug, Clone)]
pub enum Step {
    Apply(Request),
    /// Move the clock, then expire what is due. May be negative.
    Advance(i64),
    /// Seal a batch, commit it to the store and check the log against it.
    Seal,
    /// Seal, then replace the engine by one restored from the store.
    Reload,
}

#[derive(Debug, Clone)]
pub struct Case {
    pub objects: u32,
    pub steps: Vec<Step>,
}

#[must_use]
pub fn token(n: u8) -> TokenHash {
    TokenHash([n; 32])
}

/// A failure at a step, with the case that produced it.
#[derive(Debug, Clone)]
pub struct Failed {
    pub step: usize,
    pub violation: Violation,
}

impl Case {
    /// Run against the model and a fresh engine; the first violation.
    pub fn run(&self, stats: &mut Stats) -> Result<(), Failed> {
        let mut run = Run::new(self.objects).map_err(|violation| Failed { step: 0, violation })?;
        for (i, step) in self.steps.iter().enumerate() {
            stats.steps += 1;
            run.step(step, stats)
                .map_err(|violation| Failed { step: i, violation })?;
        }
        run.step(&Step::Reload, stats).map_err(|violation| Failed {
            step: self.steps.len(),
            violation,
        })?;
        stats.runs += 1;
        Ok(())
    }

    /// Run, catching a panic as a failure of its own kind.
    pub fn check(&self, stats: &mut Stats) -> Result<(), Failed> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run(stats))).unwrap_or_else(
            |p| {
                let msg = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_default();
                Err(Failed {
                    step: 0,
                    violation: Violation::new("panic", msg),
                })
            },
        )
    }
}

/// The model and the engine side by side, with a store for sealed batches.
struct Run {
    objects: u32,
    model: Model,
    engine: EventState,
    store: MemoryStore,
    replay: Replay,
    now: Timestamp,
}

fn empty() -> LoadedEvent {
    LoadedEvent {
        epoch: 1,
        seq: 0,
        holds: Vec::new(),
        objects: Vec::new(),
        idempotency: Vec::new(),
    }
}

fn engine_err(e: impl std::fmt::Display) -> Violation {
    Violation::new("engine", e.to_string())
}

impl Run {
    fn new(objects: u32) -> Result<Self, Violation> {
        let objects = objects.clamp(1, MAX_OBJECTS);
        let store = MemoryStore::new();
        store.create_event(EVENT, 1, cavea_engine::ChartVersionId(1));
        let engine = EventState::restore(EVENT, chart(objects)?, empty()).map_err(engine_err)?;
        Ok(Self {
            objects,
            model: Model::new(objects),
            engine,
            store,
            replay: Replay::default(),
            now: T0,
        })
    }

    fn step(&mut self, step: &Step, stats: &mut Stats) -> Result<(), Violation> {
        match step {
            Step::Advance(ms) => {
                self.now = Timestamp(self.now.0.saturating_add_signed(*ms));
                self.model.expire_due(self.now);
                self.engine.expire_due(self.now).map_err(engine_err)?;
                stats.count("time steps");
            }
            Step::Apply(request) => {
                let before: Vec<_> = (0..self.objects)
                    .map(|i| self.engine.booking(i).cloned())
                    .collect();
                let expected = self.model.apply(request, self.now);
                let got = self.engine.apply(request, self.now).map_err(engine_err)?;
                stats.count("replies compared");
                stats.count(reply_kind(&got));
                ensure!(
                    got == expected,
                    "reply",
                    "engine {got:?}, model {expected:?}"
                );
                if request.authority == Authority::Publishable {
                    // INV-3: a publishable key never changes a booked object.
                    for (i, b) in before.iter().enumerate() {
                        let after = self.engine.booking(i as u32);
                        ensure!(
                            b.is_none() || b.as_ref() == after,
                            "INV-3",
                            "object {i} changed"
                        );
                    }
                    stats.count("INV-3 checked");
                }
            }
            Step::Seal => self.seal(stats)?,
            Step::Reload => {
                self.seal(stats)?;
                let loaded = poll_now(self.store.load_event(EVENT))
                    .ok_or_else(|| Violation::new("store", "load waited"))?
                    .map_err(engine_err)?;
                self.engine =
                    EventState::restore(EVENT, chart(self.objects)?, loaded).map_err(engine_err)?;
                stats.count("reloads");
            }
        }
        self.model.diff(&mut self.engine)?;
        stats.count("states compared");
        invariants(&self.engine, self.now)?;
        stats.count("INV-1, INV-2 checked");
        Ok(())
    }

    /// Seal: the batch's log is the model's, contiguous (INV-5), accepted by
    /// the store, and replaying every committed log rebuilds memory and the
    /// store's rows (INV-6, INV-10).
    fn seal(&mut self, stats: &mut Stats) -> Result<(), Violation> {
        let batch = self.engine.take_batch().map_err(engine_err)?;
        let expected = self.model.take_log();
        let Some(batch) = batch else {
            ensure!(
                expected.is_empty(),
                "log",
                "no batch, model logged {expected:?}"
            );
            return Ok(());
        };
        ensure!(
            batch.log == expected,
            "log",
            "engine {:?}, model {expected:?}",
            batch.log
        );
        ensure!(
            batch.seq == self.engine.seq(),
            "INV-5",
            "batch seq {}",
            batch.seq
        );
        poll_now(self.store.commit_batch(batch))
            .ok_or_else(|| Violation::new("store", "commit waited"))?
            .map_err(|e| Violation::new("INV-5", e.to_string()))?;
        for r in &expected {
            self.replay.apply(r)?;
        }
        self.replay.diff_state(&self.engine)?;
        let loaded = poll_now(self.store.load_event(EVENT))
            .ok_or_else(|| Violation::new("store", "load waited"))?
            .map_err(engine_err)?;
        self.replay.diff_loaded(&loaded)?;
        stats.count("INV-5, INV-6, INV-10 checked");
        Ok(())
    }
}

/// A reply's kind, for counting which paths a run reached.
#[must_use]
pub fn reply_kind(reply: &cavea_engine::Reply) -> &'static str {
    use cavea_engine::Outcome as O;
    match reply {
        Ok(O::HoldCreated { .. }) => "ok hold_created",
        Ok(O::Held { .. }) => "ok held",
        Ok(O::Released { .. }) => "ok released",
        Ok(O::Extended { .. }) => "ok extended",
        Ok(O::Booked { .. }) => "ok booked",
        Ok(O::StatusChanged { .. }) => "ok status_changed",
        Err(f) => f.code(),
    }
}

/// INV-1 and INV-2 on the engine's state at `now`: one valid status per
/// object, booking data exactly when booked, held exactly when listed under
/// exactly one unexpired hold, which is its holder; no hold over its limit.
pub fn invariants(s: &EventState, now: Timestamp) -> Result<(), Violation> {
    let holds: Vec<_> = s.holds().collect();
    for h in &holds {
        ensure!(
            h.expires_at > now,
            "INV-2",
            "hold {:?} outlived its expiry",
            h.id
        );
        let over = h.max_objects.is_some_and(|m| h.objects.len() > m as usize);
        ensure!(!over, "INV-2", "hold {:?} over its limit", h.id);
    }
    for i in 0..s.chart().len() as u32 {
        let Some(status) = s.status(i) else {
            return Err(Violation::new("INV-1", format!("object {i} has no status")));
        };
        ensure!(
            s.booking(i).is_some() == (status == Status::BOOKED),
            "INV-1",
            "object {i}: booking"
        );
        let listed: Vec<_> = holds.iter().filter(|h| h.objects.contains(&i)).collect();
        if status == Status::HELD {
            ensure!(
                listed.len() == 1,
                "INV-2",
                "object {i} held, under {} holds",
                listed.len()
            );
            ensure!(
                s.holder(i) == listed.first().map(|h| h.id),
                "INV-2",
                "object {i}: holder"
            );
        } else {
            ensure!(
                listed.is_empty() && s.holder(i).is_none(),
                "INV-2",
                "object {i} listed but {status:?}"
            );
        }
    }
    Ok(())
}

// ---- Text format ----

fn text(spec: &str) -> Arc<str> {
    match spec {
        "~empty" => "".into(),
        "~long" => "x".repeat(256).into(),
        "~nul" => "a\0b".into(),
        s => s.into(),
    }
}

fn spec(text: &str) -> String {
    if text.is_empty() {
        "~empty".into()
    } else if text.contains('\0') {
        "~nul".into()
    } else if text.chars().count() > 128 {
        "~long".into()
    } else {
        text.into()
    }
}

fn write_refs(out: &mut String, refs: &[ObjectRef]) {
    let mut i = 0;
    while let Some(r) = refs.get(i) {
        let run = refs[i..].iter().take_while(|x| *x == r).count();
        let one = match r {
            ObjectRef::Idx(n) => format!("#{n}"),
            ObjectRef::Label(l) => l.to_string(),
        };
        if run > 2 {
            out.push_str(&format!(" {one}*{run}"));
        } else {
            out.push_str(&format!(" {one}").repeat(run));
        }
        i += run;
    }
}

fn parse_ref(s: &str) -> Result<Vec<ObjectRef>, String> {
    let (one, n) = match s.split_once('*') {
        Some((one, n)) => (one, n.parse().map_err(|_| format!("bad count {s}"))?),
        None => (s, 1),
    };
    let r = match one.strip_prefix('#') {
        Some(n) => ObjectRef::Idx(n.parse().map_err(|_| format!("bad idx {s}"))?),
        None => ObjectRef::Label(one.into()),
    };
    Ok(vec![r; n])
}

fn tok_name(t: &TokenHash) -> String {
    format!("t{}", t.0[0])
}

impl std::fmt::Display for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "objects {}", self.objects)?;
        for step in &self.steps {
            writeln!(f, "{}", step_line(step))?;
        }
        Ok(())
    }
}

fn step_line(step: &Step) -> String {
    let r = match step {
        Step::Advance(ms) => return format!("advance {ms}"),
        Step::Seal => return "seal".into(),
        Step::Reload => return "reload".into(),
        Step::Apply(r) => r,
    };
    let mut out = String::from(if r.authority == Authority::Secret {
        "s"
    } else {
        "p"
    });
    let token_and_refs = |out: &mut String, op: &str, token: &TokenHash, refs: &[ObjectRef]| {
        out.push_str(&format!(" {op} {}", tok_name(token)));
        write_refs(out, refs);
    };
    match &r.command {
        Command::CreateHold {
            token,
            ttl_secs,
            max_objects,
        } => {
            let max = max_objects.map(|m| format!(" max={m}")).unwrap_or_default();
            out.push_str(&format!(" create {} ttl={ttl_secs}{max}", tok_name(token)));
        }
        Command::Hold { token, objects } => token_and_refs(&mut out, "hold", token, objects),
        Command::Release { token, objects } => token_and_refs(&mut out, "release", token, objects),
        Command::ExtendHold { token, ttl_secs } => {
            out.push_str(&format!(" extend {} ttl={ttl_secs}", tok_name(token)));
        }
        Command::Book {
            token,
            objects,
            order_ref,
            details,
        } => {
            out.push_str(" book");
            if let Some(t) = token {
                out.push_str(&format!(" {}", tok_name(t)));
            }
            out.push_str(&format!(" order={}", spec(order_ref)));
            write_refs(&mut out, objects);
            for d in details {
                let mut one = String::new();
                write_refs(&mut one, std::slice::from_ref(&d.object));
                let tt = d.ticket_type.as_deref().map_or("-".into(), spec);
                let extra = d.extra.as_ref().map_or("-".into(), |e| e.len().to_string());
                out.push_str(&format!(" detail={}:{tt}:{extra}", one.trim()));
            }
        }
        Command::ReleaseBooked { objects } => {
            out.push_str(" unbook");
            write_refs(&mut out, objects);
        }
        Command::ChangeStatus { objects, status } => {
            out.push_str(&format!(" status {}", status.code()));
            write_refs(&mut out, objects);
        }
    }
    if let Some(k) = &r.idempotency {
        let fp = if k.fingerprint[0] == 0 { "a" } else { "b" };
        out.push_str(&format!(" key={}:{fp}", spec(&k.key)));
    }
    out
}

impl Case {
    /// Read the text format back.
    pub fn parse(src: &str) -> Result<Self, String> {
        let mut objects = 8;
        let mut steps = Vec::new();
        for (n, line) in src.lines().enumerate() {
            let line = line.split(" # ").next().unwrap_or_default().trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            parse_line(line, &mut objects, &mut steps)
                .map_err(|e| format!("line {}: {e}: {line}", n + 1))?;
        }
        Ok(Self { objects, steps })
    }
}

fn parse_line(line: &str, objects: &mut u32, steps: &mut Vec<Step>) -> Result<(), String> {
    let mut words = line.split_whitespace();
    let first = words.next().unwrap_or_default();
    let num = |w: Option<&str>| w.and_then(|w| w.parse::<i64>().ok()).ok_or("bad number");
    let authority = match first {
        "objects" => {
            *objects = u32::try_from(num(words.next())?).map_err(|_| "bad count")?;
            return Ok(());
        }
        "advance" => {
            steps.push(Step::Advance(num(words.next())?));
            return Ok(());
        }
        "seal" | "reload" => {
            steps.push(if first == "seal" {
                Step::Seal
            } else {
                Step::Reload
            });
            return Ok(());
        }
        "s" => Authority::Secret,
        "p" => Authority::Publishable,
        _ => return Err("unknown step".into()),
    };
    let op = words.next().unwrap_or_default();
    let (mut tok, mut ttl, mut max, mut order, mut key, mut code) =
        (None, 900, None, None, None, None);
    let (mut refs, mut details) = (Vec::new(), Vec::new());
    for w in words {
        let (k, v) = w.split_once('=').unwrap_or((w, ""));
        let int = || v.parse::<u32>().map_err(|_| format!("bad value {w}"));
        match k {
            "ttl" => ttl = int()?,
            "max" => max = Some(int()?),
            "order" => order = Some(text(v)),
            "key" => {
                let (name, fp) = v.split_once(':').ok_or("key needs :a or :b")?;
                key = Some(Idempotency {
                    key: text(name),
                    fingerprint: [u8::from(fp == "b"); 32],
                    checked_at: Timestamp(0),
                });
            }
            "detail" => {
                let mut parts = v.splitn(3, ':');
                let object = parse_ref(parts.next().unwrap_or_default())?.remove(0);
                let tt = parts.next().filter(|t| *t != "-").map(text);
                let extra = match parts.next() {
                    Some("-") | None => None,
                    Some(n) => Some("x".repeat(n.parse().map_err(|_| "bad extra")?).into()),
                };
                details.push(BookDetail {
                    object,
                    ticket_type: tt,
                    extra,
                });
            }
            _ if k.len() > 1
                && k.starts_with('t')
                && tok.is_none()
                && refs.is_empty()
                && v.is_empty() =>
            {
                tok = Some(token(k[1..].parse().map_err(|_| format!("bad token {k}"))?));
            }
            _ if op == "status" && code.is_none() => {
                code = Some(k.parse::<u8>().map_err(|_| format!("bad status {k}"))?);
            }
            _ => refs.extend(parse_ref(w)?),
        }
    }
    let need_tok = || tok.ok_or_else(|| "missing token".to_string());
    let command = match op {
        "create" => Command::CreateHold {
            token: need_tok()?,
            ttl_secs: ttl,
            max_objects: max,
        },
        "hold" => Command::Hold {
            token: need_tok()?,
            objects: refs,
        },
        "release" => Command::Release {
            token: need_tok()?,
            objects: refs,
        },
        "extend" => Command::ExtendHold {
            token: need_tok()?,
            ttl_secs: ttl,
        },
        "book" => Command::Book {
            token: tok,
            objects: refs,
            order_ref: order.unwrap_or_else(|| "o".into()),
            details,
        },
        "unbook" => Command::ReleaseBooked { objects: refs },
        "status" => Command::ChangeStatus {
            objects: refs,
            status: code.and_then(Status::from_code).ok_or("bad status")?,
        },
        _ => return Err(format!("unknown command {op}")),
    };
    steps.push(Step::Apply(Request {
        actor: "a".into(),
        authority,
        command,
        idempotency: key,
    }));
    Ok(())
}

// ---- Generation ----

/// A generated sequence of `len` steps, weighted towards contention: a few
/// objects and tokens, several authorities, tokens near their limits and their
/// expiry, reused idempotency keys, and hostile input.
#[must_use]
pub fn generate(seed: u64, len: usize) -> Case {
    let mut rng = Rng::new(seed);
    let objects = [3, 6, 8, 12][rng.below(4) as usize];
    let mut model = Model::new(objects);
    let mut now = T0;
    let mut steps = Vec::with_capacity(len);
    let mut keyed: Vec<Request> = Vec::new();
    while steps.len() < len {
        let step = match rng.weighted(&[60, 22, 8, 3]) {
            0 => {
                let mut request = gen_request(&mut rng, objects);
                // Mostly use tokens that exist: create a missing one first.
                if let Some(t) = token_of(&request.command)
                    && !model.holds().any(|(live, _)| *live == t)
                    && rng.chance(60)
                {
                    request.command = Command::CreateHold {
                        token: t,
                        ttl_secs: 900,
                        max_objects: None,
                    };
                }
                if rng.chance(25) {
                    match keyed
                        .get(rng.below(keyed.len() as u64 + 2) as usize)
                        .cloned()
                    {
                        // A duplicate delivery, or the same key on another request.
                        Some(dup) if rng.chance(70) => request = dup,
                        Some(other) => {
                            let k = other.idempotency.clone().map(|mut k| {
                                k.fingerprint = [1 - k.fingerprint[0].min(1); 32];
                                k
                            });
                            request.idempotency = k;
                        }
                        None => {
                            let name = match rng.below(20) {
                                0 => "~empty".into(),
                                1 => "~nul".into(),
                                2 => "~long".into(),
                                _ => format!("k{}", rng.below(6)),
                            };
                            request.idempotency = Some(Idempotency {
                                key: text(&name),
                                fingerprint: [0; 32],
                                checked_at: Timestamp(0),
                            });
                            keyed.push(request.clone());
                        }
                    }
                }
                let _ = model.apply(&request, now);
                Step::Apply(request)
            }
            1 => {
                let mut ms = gen_advance(&mut rng, now, model.next_expiry());
                let back = Timestamp(now.0.saturating_add_signed(ms));
                if ms < 0 && model.record_expires_within(back, now) {
                    ms = 0;
                }
                now = Timestamp(now.0.saturating_add_signed(ms));
                model.expire_due(now);
                Step::Advance(ms)
            }
            2 => Step::Seal,
            _ => Step::Reload,
        };
        steps.push(step);
    }
    Case { objects, steps }
}

fn gen_advance(rng: &mut Rng, now: Timestamp, next: Option<Timestamp>) -> i64 {
    let to = |t: u64| i64::try_from(t).unwrap_or(0) - i64::try_from(now.0).unwrap_or(0);
    match (rng.below(16), next) {
        // Onto a hold's expiry instant, a millisecond either side of it.
        (0..=2, Some(at)) => to(at.0) + [-1, 0, 0, 1][rng.below(4) as usize],
        (3, _) => 0,
        (4, _) => -(rng.below(5_000) as i64),
        (5, _) => 15 * 60_000,
        (6, _) if rng.chance(20) => 24 * 3_600_000,
        _ => [1, 999, 1_000, 30_000][rng.below(4) as usize],
    }
}

pub(crate) fn token_of(c: &Command) -> Option<TokenHash> {
    match c {
        Command::Hold { token, .. }
        | Command::Release { token, .. }
        | Command::ExtendHold { token, .. } => Some(*token),
        Command::Book { token, .. } => *token,
        _ => None,
    }
}

fn gen_refs(rng: &mut Rng, objects: u32, min: u64) -> Vec<ObjectRef> {
    let n = min + rng.below(3);
    let mut refs: Vec<ObjectRef> = (0..n)
        .map(|_| {
            let i = rng.below(u64::from(objects)) as u32;
            match rng.below(40) {
                0 => ObjectRef::Label("X-1".into()),
                1 => ObjectRef::Idx(objects),
                2..=20 => ObjectRef::Idx(i),
                _ => ObjectRef::Label(format!("S-{i}").into()),
            }
        })
        .collect();
    if let (Some(first), true) = (refs.first().cloned(), rng.chance(10)) {
        refs.push(first); // the same object twice
    }
    if rng.chance(1) {
        refs = vec![ObjectRef::Idx(0); 1_001];
    }
    refs
}

pub(crate) fn gen_request(rng: &mut Rng, objects: u32) -> Request {
    let t = token(rng.below(4) as u8 + 1);
    let command = match rng.weighted(&[14, 26, 10, 7, 16, 6, 11]) {
        0 => Command::CreateHold {
            token: t,
            ttl_secs: rng
                .pick(&[59, 60, 61, 300, 900, 900, 7_200, 7_201])
                .unwrap_or(900),
            max_objects: rng
                .pick(&[None, None, Some(0), Some(1), Some(2), Some(3)])
                .flatten(),
        },
        1 => {
            let min = u64::from(!rng.chance(3));
            Command::Hold {
                token: t,
                objects: gen_refs(rng, objects, min),
            }
        }
        2 => Command::Release {
            token: t,
            objects: gen_refs(rng, objects, 0),
        },
        3 => Command::ExtendHold {
            token: t,
            ttl_secs: rng.pick(&[0, 60, 120, 900, 7_201]).unwrap_or(60),
        },
        4 => {
            let with_token = rng.chance(60);
            let min = u64::from(!with_token || rng.chance(50));
            let objects_named = gen_refs(rng, objects, min);
            let details = (0..rng.below(3))
                .map(|_| BookDetail {
                    object: objects_named
                        .get(rng.below(objects_named.len() as u64 + 1) as usize)
                        .cloned()
                        .unwrap_or(ObjectRef::Idx(rng.below(u64::from(objects)) as u32)),
                    ticket_type: [
                        None,
                        Some("adult".into()),
                        Some(text("~long")),
                        Some(text("~nul")),
                    ][rng.weighted(&[3, 8, 1, 1])]
                    .clone(),
                    extra: rng
                        .chance(20)
                        .then(|| "x".repeat(if rng.chance(20) { 2_049 } else { 10 }).into()),
                })
                .collect();
            Command::Book {
                token: with_token.then_some(t),
                objects: objects_named,
                order_ref: text(["o1", "o2", "~empty", "~nul"][rng.weighted(&[10, 10, 1, 1])]),
                details,
            }
        }
        5 => Command::ReleaseBooked {
            objects: gen_refs(rng, objects, 1),
        },
        _ => Command::ChangeStatus {
            objects: gen_refs(rng, objects, 1),
            status: Status::from_code(rng.pick(&[0, 0, 3, 3, 4, 9, 1, 2]).unwrap_or(0))
                .unwrap_or(Status::FREE),
        },
    };
    Request {
        actor: "a".into(),
        authority: if rng.chance(30) {
            Authority::Publishable
        } else {
            Authority::Secret
        },
        command,
        idempotency: None,
    }
}

// ---- Shrinking ----

fn refs_mut(c: &mut Command) -> Option<&mut Vec<ObjectRef>> {
    match c {
        Command::Hold { objects, .. }
        | Command::Release { objects, .. }
        | Command::Book { objects, .. }
        | Command::ReleaseBooked { objects }
        | Command::ChangeStatus { objects, .. } => Some(objects),
        Command::CreateHold { .. } | Command::ExtendHold { .. } => None,
    }
}

/// Simpler variants of one step, simplest first.
fn simpler(step: &Step) -> Vec<Step> {
    let mut out = Vec::new();
    match step {
        Step::Advance(ms) if *ms != 0 => {
            out.extend(
                [0, ms / 2, ms.signum()]
                    .into_iter()
                    .filter(|m| m != ms)
                    .map(Step::Advance),
            );
        }
        Step::Reload => out.push(Step::Seal),
        Step::Apply(r) => {
            let mut push = |f: &dyn Fn(&mut Request)| {
                let mut r = r.clone();
                f(&mut r);
                out.push(Step::Apply(r));
            };
            if r.idempotency.is_some() {
                push(&|r| r.idempotency = None);
            }
            if r.authority == Authority::Publishable {
                push(&|r| r.authority = Authority::Secret);
            }
            if let Command::Book { details, .. } = &r.command {
                for i in 0..details.len() {
                    push(&|r| {
                        if let Command::Book { details, .. } = &mut r.command {
                            details.remove(i);
                        }
                    });
                }
            }
            let mut c = r.command.clone();
            let n = refs_mut(&mut c).map_or(0, |v| v.len());
            if n > 8 {
                push(&|r| {
                    refs_mut(&mut r.command)
                        .into_iter()
                        .for_each(|v| v.truncate(v.len() / 2))
                });
            }
            for i in 0..n.min(8) {
                push(&|r| {
                    if let Some(v) = refs_mut(&mut r.command) {
                        v.remove(i);
                    }
                });
            }
        }
        _ => {}
    }
    out
}

/// Shrink a failing case while it still fails with the same kind: drop runs
/// of steps, then simplify each step (drop objects, details and keys, shorten
/// time steps), until nothing more helps.
pub fn shrink(case: &Case, kind: &str) -> Case {
    let fails =
        |c: &Case| matches!(c.check(&mut Stats::default()), Err(f) if f.violation.kind == kind);
    let mut best = case.clone();
    // Nothing after the failing step matters.
    if let Err(f) = best.check(&mut Stats::default()) {
        best.steps.truncate(f.step + 1);
    }
    loop {
        let mut progressed = false;
        let mut size = best.steps.len() / 2;
        while size >= 1 {
            let mut start = 0;
            while start + size <= best.steps.len() {
                let mut c = best.clone();
                c.steps.drain(start..start + size);
                if fails(&c) {
                    best = c;
                    progressed = true;
                } else {
                    start += size;
                }
            }
            size /= 2;
        }
        for i in 0..best.steps.len() {
            let Some(step) = best.steps.get(i) else { break };
            for s in simpler(step) {
                let mut c = best.clone();
                if let Some(slot) = c.steps.get_mut(i) {
                    *slot = s;
                }
                if fails(&c) {
                    best = c;
                    progressed = true;
                    break;
                }
            }
        }
        if !progressed {
            return best;
        }
    }
}

/// Generate and run `count` cases from `first_seed`; on the first failure,
/// shrink it and return it ready to save under `crates/cavea-sim/cases/`.
pub fn explore(first_seed: u64, count: u64, len: usize, stats: &mut Stats) -> Result<(), String> {
    for seed in first_seed..first_seed.saturating_add(count) {
        let case = generate(seed, len);
        if let Err(f) = case.check(stats) {
            let small = shrink(&case, f.violation.kind);
            let why = small
                .check(&mut Stats::default())
                .err()
                .map_or(f.violation.to_string(), |g| g.violation.to_string());
            return Err(format!(
                "seed {seed} failed at step {}: {}\nshrunk from {} to {} steps; save as crates/cavea-sim/cases/<name>.case:\n\n# {why}\n{small}",
                f.step,
                f.violation,
                case.steps.len(),
                small.steps.len()
            ));
        }
    }
    Ok(())
}
