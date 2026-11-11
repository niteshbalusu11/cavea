//! Engine events as one JSON array per drain, for `View::takeEvents`. The
//! shapes are `RendererEvent` in `packages/renderer/src/events.ts`. Written by
//! hand: no serialisation crate, and numbers as fixed point with four decimals,
//! so neither float formatting nor library maths reaches the WASM.

use std::fmt::Write;

use cavea_render::protocol::{Event, Lod, ObjectInfo};

/// Appends the events the host is told about (selection, hold and release
/// requests, lost holds, resync, hover, focus, camera) as a JSON array; others are skipped.
pub fn to_json(events: &[Event], out: &mut String) {
    out.push('[');
    let mut first = true;
    for event in events {
        let start = out.len();
        if !first {
            out.push(',');
        }
        if write_event(event, out) {
            first = false;
        } else {
            out.truncate(start);
        }
    }
    out.push(']');
}

/// Writes one event; `false` for an event the host is not told about.
fn write_event(event: &Event, out: &mut String) -> bool {
    let kind = |out: &mut String, name: &str| {
        out.push_str("{\"type\":\"");
        out.push_str(name);
        out.push('"');
    };
    match event {
        Event::HoldRequest { request, labels } | Event::ReleaseRequest { request, labels } => {
            let hold = matches!(event, Event::HoldRequest { .. });
            kind(
                out,
                if hold {
                    "holdRequest"
                } else {
                    "releaseRequest"
                },
            );
            field(out, "request");
            let _ = write!(out, "{request}");
            field(out, "labels");
            strings(out, labels);
        }
        Event::SelectionChanged {
            selected,
            expires_at_ms,
        } => {
            kind(out, "selectionChanged");
            field(out, "expiresAt");
            optional(out, *expires_at_ms);
            field(out, "selected");
            out.push('[');
            for (i, object) in selected.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_object(out, object);
            }
            out.push(']');
        }
        Event::SelectionInvalid { rule, objects } => {
            kind(out, "selectionInvalid");
            field(out, "rule");
            string(out, rule);
            field(out, "objects");
            strings(out, objects);
        }
        Event::SelectionValid => kind(out, "selectionValid"),
        Event::ObjectHover(object) | Event::ObjectFocus(object) => {
            let hover = matches!(event, Event::ObjectHover(_));
            kind(out, if hover { "objectHover" } else { "objectFocus" });
            field(out, "object");
            match object {
                Some((object, at)) => {
                    write_object(out, object);
                    field(out, "at");
                    point(out, *at);
                }
                None => out.push_str("null,\"at\":null"),
            }
        }
        Event::ObjectUnavailable { label } => {
            kind(out, "objectUnavailable");
            field(out, "label");
            string(out, label);
        }
        Event::HoldLost { labels } => {
            kind(out, "holdLost");
            field(out, "labels");
            strings(out, labels);
        }
        Event::Resync { seq } => {
            kind(out, "resync");
            field(out, "seq");
            optional(out, *seq);
        }
        Event::CameraChanged { centre, zoom, lod } => {
            kind(out, "cameraChanged");
            field(out, "centre");
            point(out, *centre);
            field(out, "zoom");
            number(out, *zoom);
            field(out, "lod");
            string(
                out,
                match lod {
                    Lod::Overview => "overview",
                    Lod::CrossFade => "crossFade",
                    Lod::Seats => "seats",
                    Lod::Detail => "detail",
                },
            );
        }
        _ => return false,
    }
    out.push('}');
    true
}

fn field(out: &mut String, name: &str) {
    out.push_str(",\"");
    out.push_str(name);
    out.push_str("\":");
}

fn write_object(out: &mut String, o: &ObjectInfo) {
    out.push_str("{\"label\":");
    string(out, &o.label);
    for (name, value) in [
        ("category", &o.category),
        ("section", &o.section),
        ("row", &o.row),
        ("seat", &o.seat),
    ] {
        field(out, name);
        string(out, value);
    }
    field(out, "flags");
    let _ = write!(out, "{}}}", o.flags);
}

fn optional(out: &mut String, value: Option<u64>) {
    match value {
        Some(v) => {
            let _ = write!(out, "{v}");
        }
        None => out.push_str("null"),
    }
}

fn strings(out: &mut String, values: &[String]) {
    out.push('[');
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        string(out, value);
    }
    out.push(']');
}

fn string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn point(out: &mut String, [x, y]: [f32; 2]) {
    out.push('[');
    number(out, x);
    out.push(',');
    number(out, y);
    out.push(']');
}

/// Four decimals, truncated; 0 for a value that is not finite.
fn number(out: &mut String, value: f32) {
    let scaled = f64::from(value) * 1e4;
    let n = if scaled.is_finite() { scaled as i64 } else { 0 };
    let sign = if n < 0 { "-" } else { "" };
    let n = n.unsigned_abs();
    let _ = write!(out, "{sign}{}.{:04}", n / 10_000, n % 10_000);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_valid_json_with_escaped_text() {
        let object = ObjectInfo {
            label: "A\"1\\\n".into(),
            category: "a".into(),
            section: String::new(),
            row: "B".into(),
            seat: "é".into(),
            flags: 3,
        };
        let events = [
            Event::NeedFrame,
            Event::SelectionChanged {
                selected: vec![object.clone()],
                expires_at_ms: Some(1_800_000_000_000),
            },
            Event::Resync { seq: None },
            Event::NeedFrame,
            Event::ObjectHover(Some((object, [-1.5, f32::NAN]))),
            Event::ObjectFocus(None),
            Event::HoldRequest {
                request: 7,
                labels: vec!["x".into(), "y".into()],
            },
            Event::SelectionValid,
        ];
        let mut out = String::new();
        to_json(&events, &mut out);
        let object = r#"{"label":"A\"1\\\u000a","category":"a","section":"","row":"B","seat":"é","flags":3}"#;
        assert_eq!(
            out,
            format!(
                r#"[{{"type":"selectionChanged","expiresAt":1800000000000,"selected":[{object}]}},{{"type":"resync","seq":null}},{{"type":"objectHover","object":{object},"at":[-1.5000,0.0000]}},{{"type":"objectFocus","object":null,"at":null}},{{"type":"holdRequest","request":7,"labels":["x","y"]}},{{"type":"selectionValid"}}]"#
            )
        );
        let mut out = String::new();
        to_json(&[Event::NeedFrame], &mut out);
        assert_eq!(out, "[]");
    }
}
