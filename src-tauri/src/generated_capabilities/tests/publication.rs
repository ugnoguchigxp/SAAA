//! P01-P05: publication config limits and the offer snapshot.

use super::*;
use crate::generated_capabilities::contracts::{ContractField, FieldKind, WasmContract};
use crate::generated_capabilities::publication::{
    self, GeneratedToolSnapshot, GeneratedToolsConfig, MAX_DEFINITIONS_BYTES, MAX_PUBLISHED_TOOLS,
};

pub(super) fn write_config(value: Value) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("generated-tools.json");
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    (directory, path)
}

pub(super) fn enabled(ids: Vec<&str>) -> Value {
    json!({ "formatVersion": 1, "enabled": true, "capabilityIds": ids })
}

pub(super) fn contract(names: &[&str]) -> WasmContract {
    WasmContract {
        version: 1,
        fields: names
            .iter()
            .map(|name| ContractField {
                name: (*name).to_string(),
                kind: FieldKind::Boolean,
                values: Vec::new(),
                nullable: false,
                undefinable: false,
                optional: false,
            })
            .collect(),
    }
}

pub(super) fn resolved(
    index: usize,
    contract: WasmContract,
) -> crate::generated_capabilities::contracts::ResolvedCapability {
    crate::generated_capabilities::contracts::ResolvedCapability {
        capability_id: format!("cap_{index}"),
        revision_id: uuid::Uuid::from_u128(index as u128 + 1).to_string(),
        package_hash: "0".repeat(64),
        contract_hash: "1".repeat(64),
        catalog_epoch: 1,
        contract,
    }
}

#[test]
pub(super) fn p01_no_config_disabled_or_empty_list_publishes_nothing() {
    assert!(!GeneratedToolsConfig::from_path(None).enabled);
    let (_directory, disabled) = write_config(json!({
        "formatVersion": 1,
        "enabled": false,
        "capabilityIds": ["cap_a"]
    }));
    assert!(!GeneratedToolsConfig::from_path(Some(&disabled)).enabled);

    let (_directory, empty) = write_config(enabled(vec![]));
    let config = GeneratedToolsConfig::from_path(Some(&empty));
    assert!(config.enabled, "enabled with an empty list is valid");
    assert!(config.capability_ids.is_empty());
    assert!(GeneratedToolSnapshot::build(Vec::new()).unwrap().is_empty());
}

#[test]
pub(super) fn p02_malformed_configs_disable_publication_without_failing_startup() {
    let cases = [
        json!({ "formatVersion": 2, "enabled": true, "capabilityIds": [] }),
        json!({ "formatVersion": 1, "enabled": true, "capabilityIds": [], "extra": 1 }),
        json!({ "formatVersion": 1, "enabled": true, "capabilityIds": ["cap_a", "cap_a"] }),
        json!({ "formatVersion": 1, "enabled": true, "capabilityIds": [""] }),
        json!({
            "formatVersion": 1,
            "enabled": true,
            "capabilityIds": (0..=MAX_PUBLISHED_TOOLS).map(|i| format!("cap_{i}")).collect::<Vec<_>>()
        }),
    ];
    for value in cases {
        let (_directory, path) = write_config(value.clone());
        let config = GeneratedToolsConfig::from_path(Some(&path));
        assert!(!config.enabled, "must reject {value}");
        assert!(config.diagnostic.is_some(), "must keep a safe diagnostic");
    }

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("too-large.json");
    fs::write(&path, vec![b'x'; publication::MAX_CONFIG_BYTES + 1]).unwrap();
    let config = GeneratedToolsConfig::from_path(Some(&path));
    assert!(!config.enabled);
    assert!(config.diagnostic.is_some());

    // An unreadable path and invalid JSON are rejected the same way.
    assert!(!GeneratedToolsConfig::from_path(Some(&directory.path().join("missing.json"))).enabled);
}

#[test]
pub(super) fn p02b_relative_config_path_is_rejected_before_read() {
    // A relative path would resolve against the launch directory, so it must
    // never enable publication even if a matching file exists in the cwd.
    let config =
        GeneratedToolsConfig::from_path(Some(std::path::Path::new("generated-tools.json")));
    assert!(!config.enabled);
    assert!(config.diagnostic.is_some());
}

#[test]
pub(super) fn p04_names_come_from_the_revision_and_schema_is_the_strict_boolean_subset() {
    let snapshot = GeneratedToolSnapshot::build(vec![resolved(0, contract(&["alpha", "beta"]))])
        .expect("snapshot builds");
    let descriptor = &snapshot.descriptors()[0];
    assert!(descriptor.tool_name.starts_with("gc_"));
    assert_eq!(descriptor.tool_name.len(), "gc_".len() + 32);
    assert!(!descriptor.tool_name.contains('-'));
    assert!(snapshot.resolve(&descriptor.tool_name).is_some());
    assert_eq!(descriptor.input_schema["type"], "object");
    assert_eq!(descriptor.input_schema["additionalProperties"], false);
    assert_eq!(
        descriptor.input_schema["properties"]["alpha"]["type"],
        "boolean"
    );
    assert_eq!(
        descriptor.input_schema["required"],
        json!(["alpha", "beta"])
    );

    // A revision id that is not a UUID is rejected, never repaired by hashing.
    let mut broken = resolved(0, contract(&["alpha"]));
    broken.revision_id = "not-a-uuid".into();
    assert_eq!(
        GeneratedToolSnapshot::build(vec![broken]).unwrap_err().code,
        CapabilityErrorCode::IntegrityError
    );
}

#[test]
pub(super) fn p05_definition_limit_rejects_the_whole_offer_without_truncation() {
    let long: Vec<String> = (0..8)
        .map(|index| format!("field{index}_{}", "x".repeat(240)))
        .collect();
    let names: Vec<&str> = long.iter().map(String::as_str).collect();

    let single = GeneratedToolSnapshot::build(vec![resolved(0, contract(&names))])
        .expect("one large tool fits");
    assert_eq!(single.descriptors().len(), 1);

    let many = (0..MAX_PUBLISHED_TOOLS)
        .map(|index| resolved(index, contract(&names)))
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&single.definitions()).unwrap().len();
    assert!(bytes <= MAX_DEFINITIONS_BYTES);
    assert!(
        bytes * MAX_PUBLISHED_TOOLS > MAX_DEFINITIONS_BYTES,
        "the fixture must actually cross the limit"
    );
    assert_eq!(
        GeneratedToolSnapshot::build(many).unwrap_err().code,
        CapabilityErrorCode::OutputLimit
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn p03_and_p06_only_the_allowlisted_active_revision_is_resolved() {
    // P03: an active capability that is not on the allowlist is never resolved.
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let unlisted = env
        .service
        .resolve_publication(&["cap_not_on_the_allowlist".to_string()])
        .expect("read succeeds");
    assert!(unlisted.is_empty());
    let listed = env
        .service
        .resolve_publication(std::slice::from_ref(&revision.capability_id))
        .expect("read succeeds");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].revision_id, revision.revision_id);

    // P06: imported-but-never-activated and unknown ids are skipped in the same read.
    let env = TestEnv::start(false);
    let candidate = env.import_a().await;
    let resolved = env
        .service
        .resolve_publication(&[candidate.capability_id.clone(), "cap_missing".into()])
        .expect("read succeeds");
    assert!(resolved.is_empty(), "inactive and unknown ids are skipped");
}
