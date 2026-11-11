use super::*;

/// The examples of `docs/03-chart-format.md` §2 in one document.
const SPEC_EXAMPLE: &str = r##"{
  "schema": 1,
  "name": "Royal Theatre",
  "settings": {
    "seatSize": 44, "seatShape": "circle",
    "labelTemplate": "{section}-{row}-{seat}",
    "tableLabelTemplate": "{section}-{table}-{seat}",
    "rowLabelDisplay": "both", "alphaOverflow": "double"
  },
  "nextOid": 20431,
  "categories": [{ "key": "vip", "label": "VIP", "color": "#B45309", "accessible": false }],
  "floors": [{ "id": 1, "name": "Stalls", "level": 0, "focalPoint": [0, -800] }],
  "nodes": [
    { "id": 10, "type": "section", "floor": 1, "label": "Orchestra", "outline": "auto",
      "category": "standard", "priority": 1.0, "entrance": "Door 3",
      "children": [
        { "id": 11, "type": "rowBlock", "origin": [0, 0], "rotation": 0,
          "seatPitch": 55, "rowPitch": 95,
          "curve": { "kind": "arc", "radius": 2400 },
          "align": "center", "stagger": 0, "skew": 0, "category": "standard",
          "rowLabels":  { "scheme": "alpha", "start": "A", "skip": ["I", "O"], "direction": "front-to-back" },
          "seatLabels": { "scheme": "numeric", "start": 1, "step": 1, "direction": "ltr", "continuous": false },
          "rows": [
            { "rid": 12, "seats": 30, "offset": 0, "oids": [[100, 30]] },
            { "rid": 13, "seats": 32, "offset": 0, "oids": [[130, 32]], "label": "BB", "removed": [15, 16] }
          ],
          "seatOverrides": {
            "104": { "flags": ["accessible"], "category": "access" },
            "105": { "flags": ["companion"] },
            "131": { "label": "W1", "nudge": [0, -10] }
          } }
      ] },
    { "id": 20, "type": "section", "outline": { "points": [[0, 0], [10, 0], [10, 10]] } },
    { "id": 40, "type": "row", "path": { "kind": "polyline", "points": [[0,0],[500,40]] },
      "seatPitch": 55, "seats": 9, "oids": [[400, 9]], "label": "A",
      "seatLabels": { "scheme": "odd", "direction": "center-out" } },
    { "id": 50, "type": "table", "shape": "round", "radius": 90, "seats": 8, "bookAs": "seats",
      "label": "T1", "oids": [[500, 8]], "tableOid": 508,
      "seatLabels": { "scheme": "numeric", "start": 1, "direction": "clockwise" } },
    { "id": 60, "type": "booth", "size": [200, 150], "label": "Box 1", "oid": 600, "category": "vip" },
    { "id": 70, "type": "area", "shape": { "points": [[0,0],[900,0],[900,600],[0,600]] },
      "label": "Standing", "capacity": 500, "oid": 700, "category": "ga" },
    { "id": 80, "type": "shape", "kind": "rect", "size": [1200, 300], "cornerRadius": 20,
      "style": { "fill": "#1F2937", "stroke": null, "strokeWidth": 0, "opacity": 1 }, "layer": "below" },
    { "id": 81, "type": "text", "text": "STAGE", "size": 120, "weight": 600,
      "color": "#F9FAFB", "align": "center", "minZoom": 0 },
    { "id": 82, "type": "icon", "icon": "entrance", "size": 80 },
    { "id": 83, "type": "image", "asset": "b3a1", "size": [4000, 3000], "opacity": 0.5,
      "role": "reference" }
  ]
}"##;

#[test]
fn spec_example_round_trips() {
    let doc = ChartDocument::from_json(SPEC_EXAMPLE.as_bytes()).unwrap();
    let again = ChartDocument::from_json(&doc.to_json().unwrap()).unwrap();
    assert_eq!(doc, again);

    let [Node::Section(section), Node::Section(explicit), ..] = doc.nodes.as_slice() else {
        panic!("sections")
    };
    assert!(matches!(&explicit.outline, Outline::Points(p) if p.points.len() == 3));
    assert_eq!(section.outline, Outline::Auto);
    let [Node::RowBlock(block)] = section.children.as_slice() else {
        panic!("row block")
    };
    assert_eq!(block.curve, Curve::Arc { radius: 2400.0 });
    assert_eq!(block.row_labels.start, Some(LabelStart::Text("A".into())));
    assert_eq!(block.seat_labels.start, Some(LabelStart::Number(1)));
    assert_eq!(block.seat_overrides[&131].nudge, Some([0.0, -10.0]));
    assert_eq!(block.rows[1].removed, [15, 16]);
}

#[test]
fn floats_survive_a_save() {
    let json = br#"{"schema":1,"name":"","nextOid":1,"nodes":[
        {"id":1,"type":"booth","origin":[0.1,-123456.78901234567],"rotation":33.333333333333336,
         "size":[1e-7,2.5],"oid":2}]}"#;
    let doc = ChartDocument::from_json(json).unwrap();
    let saved = doc.to_json().unwrap();
    assert_eq!(ChartDocument::from_json(&saved).unwrap(), doc);
    let Some(Node::Booth(booth)) = doc.nodes.first() else {
        panic!()
    };
    assert_eq!(booth.origin, [0.1, -123456.78901234567]);
}

#[test]
fn rejects_unknown_and_malformed_input() {
    let base = |node: &str| format!(r#"{{"schema":1,"name":"","nextOid":1,"nodes":[{node}]}}"#);
    let bad = [
        r#"{"schema":1,"name":"","nextOid":1,"extra":true}"#.to_string(),
        base(r#"{"id":1,"type":"booth","size":[1,1],"oid":2,"colour":"red"}"#),
        base(r#"{"id":1,"type":"teapot"}"#),
        base(r#"{"id":1,"type":"rowBlock","rows":[],"curve":{"kind":"arc","radius":1,"sweep":2}}"#),
        base(r#"{"id":1,"type":"rowBlock","rows":[{"rid":2,"seats":1,"oids":[[1,1]],"seatz":3}]}"#),
        base(r#"{"id":1,"type":"section","outline":"manual"}"#),
        base(r#"{"id":1,"type":"booth","size":[1e999,1],"oid":2}"#),
        base(r#"{"id":-1,"type":"booth","size":[1,1],"oid":2}"#),
        base(r#"{"id":1,"type":"rowBlock","rows":[],"seatOverrides":{"+7":{}}}"#),
    ];
    for json in &bad {
        assert!(ChartDocument::from_json(json.as_bytes()).is_err(), "{json}");
    }
}

#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    let depth = 10_000;
    let mut json = String::from(r#"{"schema":1,"name":"","nextOid":1,"nodes":["#);
    for i in 0..depth {
        json.push_str(&format!(r#"{{"id":{i},"type":"section","children":["#));
    }
    for _ in 0..depth {
        json.push_str("]}");
    }
    json.push_str("]}");
    assert!(ChartDocument::from_json(json.as_bytes()).is_err());
}
