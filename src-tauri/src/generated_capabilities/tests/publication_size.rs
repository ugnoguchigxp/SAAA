//! P05: the provider-facing definition array, adapter wrappers included, is bounded by 32 KiB.

use crate::generated_capabilities::contracts::{ContractField, FieldKind, WasmContract};
use crate::generated_capabilities::publication::{
    GeneratedToolSnapshot, MAX_DEFINITIONS_BYTES, MAX_PUBLISHED_TOOLS,
};

fn contract(names: &[&str]) -> WasmContract {
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

fn resolved(
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
fn p05b_the_provider_wrapper_is_counted_in_the_definition_limit() {
    // Regression for F1: the neutral definition array can fit while the OpenAI
    // `{"type":"function","function":...}` wrapper pushes the real request array over 32 KiB.
    // Field-name lengths step the neutral size by 8 tools * 8 fields * 3 bytes = 192, which is
    // smaller than the ~248-byte wrapper window, so one length must land inside it.
    for length in 1..=256usize {
        let long: Vec<String> = (0..8)
            .map(|index| format!("f{index}{}", "x".repeat(length)))
            .collect();
        let names: Vec<&str> = long.iter().map(String::as_str).collect();
        let per_tool = contract(&names);
        let Ok(snapshot) = GeneratedToolSnapshot::build(
            (0..MAX_PUBLISHED_TOOLS)
                .map(|index| resolved(index, per_tool.clone()))
                .collect::<Vec<_>>(),
        ) else {
            continue;
        };
        let neutral = serde_json::to_vec(&snapshot.definitions()).unwrap().len();
        let provider = crate::generated_capabilities::tools::openai_tool_definitions(&snapshot);
        let provider_bytes = serde_json::to_vec(&provider).unwrap().len();
        if neutral <= MAX_DEFINITIONS_BYTES && provider_bytes > MAX_DEFINITIONS_BYTES {
            assert!(
                !crate::generated_capabilities::tools::provider_definitions_fit(&provider),
                "the wrapper bytes must count against the limit"
            );
            return;
        }
    }
    panic!("no field length lands inside the provider-wrapper window");
}
