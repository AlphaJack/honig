//! Hand-written checks for the `coverage` fixture.

use e2e::v32_coverage::api::types;
use serde_json::json;

#[test]
fn const_properties_are_unit_types() {
    // `kind: {const: image}` is required. It is a unit struct and not a `String`.
    let note = types::ImageNote::new("https://x".to_owned());
    let filled = json!({"kind": "image", "url": "https://x", "width": 64, "frame": {"color": "white", "width": 2}, "sizes": [320, 640]});
    assert_eq!(serde_json::to_value(&note).unwrap(), filled);
    assert_eq!(types::ImageNoteKind::VALUE, "image");
    let wrong = json!({"kind": "text", "url": "https://x"});
    assert!(serde_json::from_value::<types::ImageNote>(wrong).is_err(), "only `image` decodes");
}

#[test]
fn optional_const_properties_hold_only_their_value() {
    let t: types::TextNote = serde_json::from_value(json!({"kind": "text", "text": "hi"})).unwrap();
    assert_eq!(t.kind, Some(types::TextNoteKind));
    let none: types::TextNote = serde_json::from_value(json!({"text": "hi"})).unwrap();
    assert_eq!(none.kind, None);
}

#[test]
fn fixed_length_arrays_are_rust_arrays() {
    let t: types::TextNote = serde_json::from_value(json!({"text": "hi"})).unwrap();
    assert_eq!(t.point, None);
    let full: types::TextNote =
        serde_json::from_value(json!({"text": "hi", "point": [1.5, 2.5], "rgb": [1, 2, 3]})).unwrap();
    let (point, rgb): (Option<[f64; 2]>, Option<[u8; 3]>) = (full.point, full.rgb.clone().map(Into::into));
    assert_eq!((point, rgb), (Some([1.5, 2.5]), Some([1, 2, 3])));
    let short = json!({"text": "hi", "point": [1.5]});
    assert!(serde_json::from_value::<types::TextNote>(short).is_err(), "a point has exactly 2 numbers");
}

#[test]
fn a_default_next_to_a_ref_fills_the_field() {
    // `width: {$ref: Pixels, default: 64}`. `Pixels` is an integer alias declared after `ImageNote`.
    let note: types::ImageNote = serde_json::from_value(json!({"kind": "image", "url": "https://x"})).unwrap();
    let width: types::Pixels = note.width;
    assert_eq!(width, 64);
}

#[test]
fn an_object_or_list_default_fills_the_field() {
    let note: types::ImageNote = serde_json::from_value(json!({"kind": "image", "url": "https://x"})).unwrap();
    assert_eq!((note.frame.color.as_deref(), note.frame.width), (Some("white"), Some(2)));
    assert_eq!(note.sizes, [320, 640]);
}

#[test]
fn nested_one_of_variants_are_pascal_case() {
    let point: types::Crop = serde_json::from_value(json!({"x": 1, "y": 2})).unwrap();
    let types::CropVariants::Variant1(types::CropVariant1::Variant2(inner)) = &*point else { panic!("{point:?}") };
    assert_eq!((inner.x, inner.y), (Some(1), Some(2)));
}

/// A value that does not write out as JSON (a `NaN`) cannot be checked: building it fails, and the checks never run on
/// `null` in its place.
#[test]
fn a_value_that_does_not_write_out_does_not_build() {
    let fields = types::TextNoteFields { point: Some([f64::NAN, 0.0]), ..types::TextNoteFields::new("hi".to_owned()) };
    let err = types::TextNote::try_from(fields).expect_err("NaN is no JSON number");
    assert!(err.to_string().contains("NaN") || err.to_string().contains("finite"), "{err}");
}

/// A value that matches both object branches of an `anyOf` decodes as the one that keeps the most of its members, so
/// it encodes back whole. A value that only one branch's properties fill decodes as that one.
#[test]
fn an_any_of_keeps_every_member_that_a_matching_branch_declares() {
    let lamp = json!({"name": "Desk lamp", "price": 900});
    let decoded: types::Overlap = serde_json::from_value(lamp.clone()).unwrap();
    assert_eq!(serde_json::to_value(&decoded).unwrap(), lamp);
    let named = json!({"name": "Desk lamp"});
    assert_eq!(serde_json::to_value(serde_json::from_value::<types::Overlap>(named.clone()).unwrap()).unwrap(), named);
}

/// A branch with a decimal decodes from the JSON text: the ranking holds there too, and every digit stays.
#[test]
fn an_any_of_with_a_decimal_branch_keeps_its_members_and_digits() {
    let text = r#"{"name":"Desk lamp","amount":12345678901234567.89}"#;
    let decoded: types::PricedOverlap = serde_json::from_str(text).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), text);
}

/// A `oneOf` without a discriminator refuses a value that two branches match: it never decodes it as the first.
#[test]
fn a_one_of_refuses_a_value_that_two_branches_match() {
    let refused = serde_json::from_value::<types::OneOfOverlap>(json!({"name": "Desk lamp", "price": 900}));
    assert!(refused.unwrap_err().to_string().contains("matches 2 of `oneOf`"));
}
