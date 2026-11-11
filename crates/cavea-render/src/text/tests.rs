use super::*;

/// The committed atlas image, whose size the metrics must match.
const PNG: &[u8] = include_bytes!("../../../../packages/renderer/assets/glyphs.png");

#[test]
fn metrics_cover_the_glyph_set_and_match_the_atlas() {
    let set = b" -./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let chars: Vec<u8> = GLYPHS.iter().map(|g| g.ch).collect();
    assert_eq!(chars, set, "sorted, one entry per character");
    for c in set {
        assert!(glyph_code(char::from(*c)).is_some());
    }
    // The PNG header (IHDR) holds the size, big-endian, at bytes 16 to 24.
    let size = |at: usize| u32::from_be_bytes(PNG[at..at + 4].try_into().unwrap());
    assert_eq!([size(16), size(20)], ATLAS_SIZE);
    // Each glyph's atlas box is inside the atlas and is its plane box at the
    // atlas's scale: 3 px of range over RANGE_EM.
    let px_per_em = 3.0 / RANGE_EM;
    for g in GLYPHS.iter().filter(|g| g.ch != b' ') {
        let [l, t, r, b] = g.atlas;
        assert!(0.0 <= l && l < r && r <= ATLAS_SIZE[0] as f32, "{}", g.ch);
        assert!(0.0 <= t && t < b && b <= ATLAS_SIZE[1] as f32, "{}", g.ch);
        let [pl, pt, pr, pb] = g.plane;
        assert!(((pr - pl) * px_per_em - (r - l)).abs() < 1e-3, "{}", g.ch);
        assert!(((pb - pt) * px_per_em - (b - t)).abs() < 1e-3, "{}", g.ch);
    }
    // Tabular digits, which the size rule assumes.
    let zero = GLYPHS[usize::from(glyph_code('0').unwrap()) - 1].advance;
    assert_eq!(zero, NOMINAL_ADVANCE_EM);
    assert!(
        GLYPHS
            .iter()
            .filter(|g| g.ch.is_ascii_digit())
            .all(|g| g.advance == zero)
    );
}

#[test]
fn the_size_rule_fits_the_seat_and_shows_short_labels_from_the_detail_level() {
    let fits = |em: f32, width: f32| {
        let box_diagonal = em * (width * width + CAP_HEIGHT_EM * CAP_HEIGHT_EM).sqrt();
        box_diagonal / LABEL_FIT
    };
    // One and two digits are the same size; two digits fill the seat exactly.
    assert_eq!(label_em(44.0, 1, 0.5), label_em(44.0, 2, 1.0));
    assert!((fits(label_em(44.0, 2, 1.0), 1.0) - 44.0).abs() < 1e-3);
    // Wider than its digits (capital W): sized by its own width, still fits.
    let wide = label_em(44.0, 2, 1.5);
    assert!(wide < label_em(44.0, 2, 1.0));
    assert!((fits(wide, 1.5) - 44.0).abs() < 1e-3);
    // Proportional to the seat.
    assert!((label_em(88.0, 3, 1.5) - 2.0 * label_em(44.0, 3, 1.5)).abs() < 1e-4);

    // Shown on screen from LABEL_MIN_EM_PX: at the detail level's 14 px,
    // labels of up to two digits; three from 15 px, four from 19 px.
    let em_px = |seat_px: f32, n: usize| label_em(seat_px, n, NOMINAL_ADVANCE_EM * n as f32);
    let detail = crate::tuning::LOD_DETAIL_SEAT_PX as f32;
    assert!(em_px(detail, 2) >= LABEL_MIN_EM_PX);
    assert!(em_px(detail, 3) < LABEL_MIN_EM_PX && em_px(15.0, 3) >= LABEL_MIN_EM_PX);
    assert!(em_px(18.0, 4) < LABEL_MIN_EM_PX && em_px(19.0, 4) >= LABEL_MIN_EM_PX);
}

#[test]
fn a_label_instance_holds_its_glyphs_centred_or_nothing() {
    let bytes = label_instance("12", [10.0, -4.5], 44.0).unwrap();
    let f = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    assert_eq!([f(0), f(4)], [10.0, -4.5]);
    assert_eq!(f(8), label_em(44.0, 2, 1.0));
    let code = |c| glyph_code(c).unwrap();
    assert_eq!(bytes[12..16], [code('1'), code('2'), 0, 0]);
    // Two digits 0.5 em wide: origins at −0.5 and 0 em from the centre.
    assert_eq!(
        bytes[16..20].iter().map(|&b| b as i8).collect::<Vec<_>>(),
        [-32, 0, 0, 0]
    );

    let four = label_instance("A-10", [0.0; 2], 44.0).unwrap();
    assert!(four[12..16].iter().all(|&c| c != 0));
    // Skipped whole: a character outside the atlas, too many glyphs, empty.
    for label in ["W1♿", "a1", "12345", ""] {
        assert_eq!(label_instance(label, [0.0; 2], 44.0), None, "{label:?}");
    }
}
