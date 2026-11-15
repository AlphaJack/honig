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
    let to = e2e::v32_catalog::api::Email::try_from("ann@example.com")?;
    assert!(e2e::v32_catalog::api::Email::try_from("ann").is_err());
    println!("{}", to.as_str());
    // ANCHOR_END: email
    Ok(())
}
