//! Overlays: the compliant sets of the Overlay Specification and the worked examples of the 1.2 text (`spec-*`). Each
//! is in `tests/fixtures/overlay/<set>` as a description, an overlay and the description that it must become. Also the
//! generator that loads a description through one.

use crate::diag::Diagnostics;
use crate::parse::load_file;
use serde_json::{Value, json};

fn apply(doc: &mut Value, overlay: &Value) -> Diagnostics {
    let mut d = Diagnostics::default();
    crate::overlay::apply(doc, overlay, "overlay.yaml", &mut d);
    d
}

#[test]
fn compliant_sets_give_their_output() {
    let root = super::common::fixtures().join("overlay");
    let mut sets: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().path()).collect();
    sets.sort();
    assert_eq!(sets.len(), 13, "{sets:?}");
    for set in sets {
        let (mut doc, _) = load_file(&set.join("openapi.yaml")).unwrap();
        let (overlay, _) = load_file(&set.join("overlay.yaml")).unwrap();
        let (output, _) = load_file(&set.join("output.yaml")).unwrap();
        let d = apply(&mut doc, &overlay);
        assert!(d.is_empty(), "{}: {d:?}", set.display());
        assert_eq!(doc, output, "{}", set.display());
    }
}

#[test]
fn copy_then_remove_moves_a_path_item() {
    let mut doc = json!({"paths": {"/items": {"get": {"summary": "list"}}, "/some-items": {}}});
    let overlay = json!({"overlay": "1.1.0", "info": {"title": "move", "version": "1"}, "actions": [
        {"target": "$.paths", "update": {"/new-items": {}}},
        {"target": "$.paths['/new-items']", "copy": "$.paths['/items']"},
        {"target": "$.paths['/items']", "remove": true}
    ]});
    assert!(apply(&mut doc, &overlay).is_empty());
    assert_eq!(doc, json!({"paths": {"/some-items": {}, "/new-items": {"get": {"summary": "list"}}}}));
}

#[test]
fn reusable_actions_take_their_reference_target() {
    let mut doc = json!({"paths": {"/a": {"get": {"responses": {}}}, "/b": {"delete": {"responses": {}}}}});
    let overlay = json!({"overlay": "1.2.0", "info": {"title": "reuse", "version": "1"},
    "components": {"actions": {"notFound": {"fields": {"update": {"404": {"description": "Not Found"}}}}}},
    "actions": [
        {"$ref": "#/components/actions/notFound", "target": "$.paths['/a'].get.responses"},
        {"$ref": "#/components/actions/notFound", "target": "$.paths['/b'].delete.responses"}
    ]});
    assert!(apply(&mut doc, &overlay).is_empty());
    let not_found = json!({"404": {"description": "Not Found"}});
    assert_eq!(doc["paths"]["/a"]["get"]["responses"], not_found);
    assert_eq!(doc["paths"]["/b"]["delete"]["responses"], not_found);
}

/// The learn.openapis.org overlay examples filter with `$.paths.*.*[?...]`. Under
/// RFC 9535 that tests the fields inside each operation. So it selects nothing.
/// The action does nothing (not an error) and says so. `$.paths.*[?...]` applies.
#[test]
fn a_target_that_selects_nothing_is_warned() {
    let doc = json!({"paths": {"/items": {"get": {"x-supports-filters": true, "responses": {}}}}});
    let overlay = |target: &str| {
        json!({"overlay": "1.0.0", "info": {"title": "filters", "version": "1"}, "actions": [
            {"target": target, "update": {"parameters": [{"name": "month", "in": "query", "schema": {"type": "integer"}}]}}
        ]})
    };
    let mut missed = doc.clone();
    let d = apply(&mut missed, &overlay("$.paths.*.*[?(@['x-supports-filters'] == true)]"));
    let out: Vec<String> = d.iter().map(|x| format!("{:?} {}: {}", x.level, x.pointer, x.message)).collect();
    assert_eq!(out, ["Warning overlay.yaml#/actions/0/target: the target selects no nodes: the action does nothing"]);
    assert_eq!(missed, doc);
    let mut hit = doc;
    assert!(apply(&mut hit, &overlay("$.paths.*[?(@['x-supports-filters'] == true)]")).is_empty());
    assert_eq!(hit["paths"]["/items"]["get"]["parameters"][0]["name"], "month");
}

/// The MUSTs of the Overlay Object: at least one action, `info.title` and `info.version`. Also no `target` inside the
/// `fields` of a reusable action (the reference gives it).
#[test]
fn an_overlay_needs_actions_an_info_and_targets_from_references() {
    let errors = |overlay: &Value| -> Vec<String> {
        apply(&mut json!({"info": {}}), overlay).iter().map(|x| format!("{} {}", x.pointer, x.message)).collect()
    };
    let empty = json!({"overlay": "1.1.0", "info": {"title": "t", "version": "1"}, "actions": []});
    assert_eq!(errors(&empty), ["overlay.yaml#/actions `actions` needs at least one action"]);
    let no_info = json!({"overlay": "1.0.0", "actions": [{"target": "$.info", "update": {"x": 1}}]});
    assert_eq!(
        errors(&no_info),
        [
            "overlay.yaml#/info/title an overlay needs `info.title`",
            "overlay.yaml#/info/version an overlay needs `info.version`"
        ]
    );
    let targeted = json!({"overlay": "1.2.0", "info": {"title": "t", "version": "1"},
        "components": {"actions": {"a": {"fields": {"target": "$.info", "update": {"x": 1}}}}},
        "actions": [{"$ref": "#/components/actions/a", "target": "$.info"}]});
    assert_eq!(
        errors(&targeted),
        ["overlay.yaml#/actions/0 a reusable action's `fields` cannot have a `target`: the reference supplies it"]
    );
}

/// Overlay 1.2: `extends` and `$self` identify whole documents.
#[test]
fn extends_and_self_name_whole_documents() {
    let overlay = |extends: &str, own: &str| {
        json!({"overlay": "1.2.0", "$self": own, "extends": extends, "info": {"title": "t", "version": "1"},
        "actions": [{"target": "$.info", "update": {"x-done": true}}]})
    };
    let mut doc = json!({"$self": "https://example.com/api/openapi", "info": {"title": "t", "version": "1"}});
    let messages = |doc: &mut Value, o: &Value| -> Vec<String> {
        apply(doc, o).iter().map(|x| format!("{:?} {}", x.level, x.pointer)).collect()
    };
    let ok = overlay("https://example.com/api/openapi", "https://example.com/overlays/public");
    assert_eq!(messages(&mut doc, &ok), Vec::<String>::new());
    assert_eq!(
        messages(&mut doc, &overlay("../api/openapi", "https://example.com/o#x")),
        ["Error overlay.yaml#/$self"]
    );
    assert_eq!(
        messages(&mut doc, &overlay("https://example.com/other#frag", "https://example.com/o")),
        ["Error overlay.yaml#/extends", "Warning overlay.yaml#/extends"]
    );
}

#[test]
fn arrays_concatenate_from_1_1_and_take_one_entry_in_1_0() {
    let update = |version: &str| {
        let mut doc = json!({"tags": ["a"], "info": {"x": [1]}});
        let overlay = json!({"overlay": version, "info": {"title": "t", "version": "1"}, "actions": [
            {"target": "$.tags", "update": ["b", "c"]},
            {"target": "$.info", "update": {"x": [2]}},
            {"target": "$.tags[?@ == 'a']", "remove": true}
        ]});
        assert!(apply(&mut doc, &overlay).is_empty());
        doc
    };
    assert_eq!(update("1.1.0"), json!({"tags": ["b", "c"], "info": {"x": [1, 2]}}));
    assert_eq!(update("1.0.0"), json!({"tags": [["b", "c"]], "info": {"x": [1, 2]}}));
}

#[test]
fn a_broken_action_is_reported_and_the_rest_still_apply() {
    let mut doc = json!({"info": {"title": "t"}});
    let overlay = json!({"overlay": "1.2.0", "info": {"title": "t", "version": "1"}, "actions": [
        {"target": "$.info[", "update": {}},
        {"target": "$.info", "update": "a string into an object"},
        {"target": "$.info", "update": {"version": "2"}}
    ]});
    let d = apply(&mut doc, &overlay);
    let pointers: Vec<&str> = d.iter().map(|d| d.pointer.as_str()).collect();
    assert_eq!(pointers, ["overlay.yaml#/actions/0", "overlay.yaml#/actions/1"]);
    assert_eq!(doc, json!({"info": {"title": "t", "version": "2"}}));
}

#[test]
fn generation_reads_the_description_through_its_overlays() {
    let dir = std::env::temp_dir().join(format!("honig-overlay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spec = dir.join("openapi.yaml");
    std::fs::write(
        &spec,
        "openapi: 3.1.0\ninfo: {title: t, version: '1'}\npaths:\n  /products:\n    get:\n      responses: {'204': {description: ok}}\n  /internal:\n    get:\n      responses: {'204': {description: ok}}\n",
    )
    .unwrap();
    let overlay = dir.join("public.overlay.yaml");
    std::fs::write(
        &overlay,
        "overlay: 1.2.0\ninfo: {title: public, version: '1'}\nactions:\n  - target: $.paths['/internal']\n    remove: true\n  - target: $.paths['/products'].get\n    update: {operationId: listProducts}\n",
    )
    .unwrap();
    let opts = crate::emit::Options { overlays: vec![overlay], ..Default::default() };
    let g = crate::generate::generate(&spec, &opts).unwrap();
    assert!(g.diagnostics.is_empty(), "{:?}", g.diagnostics);
    assert_eq!(g.operations, 1);
    let ops = g.files.iter().find(|(p, _)| p == "operations.rs").map(|(_, t)| t.as_str()).unwrap();
    assert!(ops.contains("pub mod list_products"), "{ops}");
    std::fs::remove_dir_all(&dir).ok();
}

/// An overlay action that selects nothing fails `generate`: a `security` added to a path that was renamed would
/// leave the operation public.
#[test]
fn an_overlay_action_that_selects_nothing_fails_generation() {
    let dir = std::env::temp_dir().join(format!("honig-overlay-nothing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spec = dir.join("openapi.yaml");
    std::fs::write(
        &spec,
        "openapi: 3.1.0\ninfo: {title: t, version: '1'}\npaths:\n  /a:\n    get:\n      operationId: a\n      responses: {'204': {description: ok}}\n",
    )
    .unwrap();
    let overlay = dir.join("secure.overlay.yaml");
    std::fs::write(
        &overlay,
        "overlay: 1.2.0\ninfo: {title: secure, version: '1'}\nactions:\n  - target: $.paths['/b'].get\n    update: {security: [{local: []}]}\n",
    )
    .unwrap();
    let opts = crate::emit::Options { overlays: vec![overlay], ..Default::default() };
    let err = crate::generate::generate(&spec, &opts).unwrap_err();
    assert!(
        err.iter().any(|d| d.level == crate::diag::Level::Error && d.pointer.ends_with("/actions/0/target")),
        "{:#?}",
        err.0
    );
    std::fs::remove_dir_all(&dir).ok();
}
