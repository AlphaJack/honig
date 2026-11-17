//! Phase 1 check: parsing into the typed model and writing it back as JSON must give the input JSON.

use std::path::Path;

use serde_json::Value;

fn assert_lossless(path: &Path) {
    let loaded = crate::load::load_path(path);
    let errors: Vec<_> = loaded.diagnostics.iter().map(ToString::to_string).collect();
    let model = loaded.model.unwrap_or_else(|| panic!("{} failed to parse: {errors:#?}", path.display()));
    let input = loaded.value.expect("value present when model is");
    let output = crate::spec::to_value(&model);
    assert!(input == output, "{} is not lossless:\n{}", path.display(), first_difference(&input, &output, ""));
}

/// Human-readable pointer to the first place two JSON values differ.
fn first_difference(left: &Value, right: &Value, at: &str) -> String {
    match (left, right) {
        (Value::Object(lo), Value::Object(ro)) => {
            for (k, v) in lo {
                let here = format!("{at}/{k}");
                match ro.get(k) {
                    None => return format!("{here}: dropped (was {v})"),
                    Some(w) if v != w => return first_difference(v, w, &here),
                    _ => {}
                }
            }
            if let Some(k) = ro.keys().find(|k| !lo.contains_key(*k)) {
                return format!("{at}/{k}: added ({})", ro[k]);
            }
            format!("{at}: differs")
        }
        (Value::Array(la), Value::Array(ra)) if la.len() == ra.len() => {
            for (i, (v, w)) in la.iter().zip(ra).enumerate() {
                if v != w {
                    return first_difference(v, w, &format!("{at}/{i}"));
                }
            }
            format!("{at}: differs")
        }
        _ => format!("{at}: {left} != {right}"),
    }
}

#[test]
fn every_v32_fixture_is_lossless() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("v32");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if matches!(path.extension().and_then(|e| e.to_str()), Some("yaml" | "yml" | "json")) {
            assert_lossless(&path);
            n += 1;
        }
    }
    assert!(n > 0, "no fixtures in {}", dir.display());
}

#[test]
fn kitchen_sink_models_every_32_feature() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("v32").join("kitchen-sink.yaml");
    let doc = crate::load::load_path(&path).model.unwrap();
    assert_eq!(doc.self_uri.as_deref(), Some("https://example.com/apis/kitchen-sink.yaml"));
    let products = doc.paths.as_ref().unwrap().get("/products").unwrap();
    let methods: Vec<_> = products.operations().iter().map(|(m, _)| *m).collect();
    assert_eq!(methods, ["GET", "POST", "QUERY", "COPY"]);
    let comps = doc.components.as_ref().unwrap();
    assert!(comps.media_types.as_ref().unwrap().contains_key("ProductArray"));
    let oauth = comps.security_schemes.as_ref().unwrap()["oauth"].as_item().unwrap();
    assert!(oauth.flows.as_ref().unwrap().device_authorization.is_some());
    let product = comps.schemas.as_ref().unwrap()["Product"].as_object().unwrap();
    assert_eq!(
        product.discriminator.as_ref().unwrap().default_mapping.as_deref(),
        Some("#/components/schemas/Product")
    );
    // Unmodelled keys are kept and not dropped.
    assert!(product.extra.contains_key("x-rust-type"));
    assert!(product.properties.as_ref().unwrap()["nickname"].as_object().unwrap().default.is_some());
}
