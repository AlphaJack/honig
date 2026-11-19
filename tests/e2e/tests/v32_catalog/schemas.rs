//! Types: structs built from their required properties and strings that only a valid value can be.

use e2e::v32_catalog::api::types;

#[test]
fn a_struct_starts_from_its_required_properties() {
    // ANCHOR: new
    let lamp = types::Physical { fragile: Some(true), ..types::Physical::new(types::PhysicalKind, "Desk lamp".into()) };
    // ANCHOR_END: new
    assert_eq!(lamp.name, "Desk lamp");
}

#[test]
fn try_into_builds_a_checked_field() -> Result<(), e2e::v32_catalog::api::BuildError> {
    // ANCHOR: try-into
    let lamp = types::Physical {
        sku: Some("LMP-0042".try_into()?),
        ..types::Physical::new(types::PhysicalKind, "Desk lamp".into())
    };
    // ANCHOR_END: try-into
    assert_eq!(lamp.sku.as_deref(), Some("LMP-0042"));
    Ok(())
}

/// `try_into`, `parse` and `build` fail with one error type: a function that mixes them needs one `?` each and no
/// `Box<dyn Error>`.
#[test]
fn every_way_to_build_a_value_fails_with_build_error() {
    fn lamp(sku: &str) -> Result<types::Physical, e2e::v32_catalog::api::BuildError> {
        let sku: types::PhysicalSku = sku.parse()?;
        let checked: types::PhysicalSku = sku.as_str().try_into()?;
        types::Physical::builder(types::PhysicalKind, "Desk lamp").sku(checked).build()
    }
    assert!(lamp("LMP-0042").is_ok());
    let e = lamp("lamp").expect_err("not the pattern");
    assert!(e.to_string().contains("does not match"), "{e}");
}

#[test]
fn a_pattern_string_is_checked_when_it_is_made() -> Result<(), Box<dyn std::error::Error>> {
    // ANCHOR: pattern
    let sku = types::PhysicalSku::try_from("LMP-0042")?; // checks `^[A-Z]{3}-[0-9]{4}$`
    assert!(types::PhysicalSku::try_from("lamp").is_err());
    println!("{}", sku.as_str());
    // ANCHOR_END: pattern
    Ok(())
}

#[test]
fn an_email_is_checked_when_it_is_made() -> Result<(), Box<dyn std::error::Error>> {
    // ANCHOR: email
    let to = e2e::v32_catalog::api::EmailText::try_from("ann@example.com")?;
    assert!(e2e::v32_catalog::api::EmailText::try_from("ann").is_err());
    println!("{}", to.as_str());
    // ANCHOR_END: email
    // It reads as a `str`, as every checked text does.
    assert_eq!(to.len(), "ann@example.com".len());
    assert_eq!(to, "ann@example.com");
    Ok(())
}
