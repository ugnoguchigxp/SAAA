//! WM-01/WM-02 contract tests: types, normalization, semantic keys, byte limits.
use saaa_personal_state_core::world::*;

#[test]
fn t01_rejects_bad_payloads_and_measures_bytes_not_chars() {
    // Unknown field rejected.
    let bad = serde_json::json!({
        "type": "entity", "schema_version": 1, "entity_id": "e1",
        "entity_kind": "concept", "name": "n", "aliases": [], "extra": 1
    });
    assert!(WorldPayload::decode("world_entity", &bad).is_err());
    // Bad enum rejected.
    let bad_enum = serde_json::json!({
        "type": "entity", "schema_version": 1, "entity_id": "e1",
        "entity_kind": "person", "name": "n", "aliases": []
    });
    assert!(WorldPayload::decode("world_entity", &bad_enum).is_err());
    // Kind/tag mismatch rejected.
    let ok_entity = serde_json::json!({
        "type": "entity", "schema_version": 1, "entity_id": "e1",
        "entity_kind": "concept", "name": "n", "aliases": []
    });
    assert!(WorldPayload::decode("world_relation", &ok_entity).is_err());
    // 2,000 bytes measured on UTF-8 encoding, not char count.
    // 700 CJK chars ~ 2100 bytes must exceed.
    let long = "あ".repeat(700);
    let big = serde_json::json!({
        "type": "entity", "schema_version": 1, "entity_id": "e1",
        "entity_kind": "concept", "name": long, "aliases": []
    });
    assert!(WorldPayload::encoded_len(&big).unwrap() > 2000);
    // Round trip ok.
    let p = WorldPayload::decode("world_entity", &ok_entity).unwrap();
    assert!(matches!(p, WorldPayload::Entity(_)));
    // Relation/ focus round trips.
    let rel = serde_json::json!({
        "type": "relation", "schema_version": 1,
        "from_entity_id": "a", "to_entity_id": "b", "relation_type": "related_to",
        "effect_input": null, "conditions": [],
        "basis": "user_statement",
        "evidence_stances": [{"source": {"id": "s", "version": 1, "start": 0, "end": 1}, "stance": "supports"}]
    });
    assert!(WorldPayload::decode("world_relation", &rel).is_ok());
    let focus = serde_json::json!({
        "type": "focus", "schema_version": 1, "entity_id": "e1",
        "reason": "explicit_interest", "objective_assertion_id": null
    });
    assert!(WorldPayload::decode("world_focus", &focus).is_ok());
}

#[test]
fn t02_normalization_alias_semantic_key() {
    assert_eq!(normalize_name("  Hello   WORLD\t"), "hello world");
    assert_eq!(
        normalize_name("投機的デコード  の導入"),
        "投機的デコード の導入"
    );
    // Only ASCII case folding: full-width stays.
    assert_ne!(normalize_name("Ａ"), "a");
    // related_to endpoint flip => same key; directed differs.
    let (a, b) = order_undirected("e2", "e1");
    assert_eq!((a.as_str(), b.as_str()), ("e1", "e2"));
    let key = |from: &str, to: &str, rt: &str, effect: Option<&str>| {
        relation_key(&RelationKeyInput {
            project_scope: "project:x",
            from,
            to,
            relation_type: rt,
            effect_input: effect,
            sorted_conditions: &[],
            valid_from: 1,
            valid_until: None,
        })
    };
    let k1 = key("e1", "e2", "related_to", None);
    let k2 = key("e2", "e1", "related_to", None);
    assert_ne!(k1, k2); // raw order differs; caller must sort for related_to
    let (s1, s2) = order_undirected("e2", "e1");
    let (t1, t2) = order_undirected("e1", "e2");
    assert_eq!(
        key(&s1, &s2, "related_to", None),
        key(&t1, &t2, "related_to", None)
    );
    let kd1 = key("e1", "e2", "increases", Some("quantity_increase"));
    let kd2 = key("e2", "e1", "increases", Some("quantity_increase"));
    assert_ne!(kd1, kd2);
    assert!(kd1.starts_with("wm1:"));
    assert_eq!(entity_key("project:x", "e1").len(), 4 + 64);
}
