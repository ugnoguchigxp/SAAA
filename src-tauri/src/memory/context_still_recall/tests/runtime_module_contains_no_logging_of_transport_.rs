use super::*;
#[test]
    pub(super) fn runtime_module_contains_no_logging_of_transport_data() {
        let source = include_str!("../../context_still_recall.rs");
        let forbidden = [
            ["print", "ln!"].concat(),
            ["eprint", "ln!"].concat(),
            ["db", "g!"].concat(),
            ["tracing", "::"].concat(),
            ["log", "::"].concat(),
        ];
        for forbidden in forbidden {
            assert!(
                !source.contains(&forbidden),
                "forbidden logger: {forbidden}"
            );
        }
    }
#[cfg(not(coverage))]
    #[tokio::test]
    #[ignore = "operator-only ContextStill typed-memory MCP compatibility canary"]
    pub(super) async fn live_typed_memory_contract() {
        let client = ContextStillRecallClient::from_environment();
        assert!(client.is_configured());
        let result = client
            .recall(
                RECALL_RULE_TOOL_NAME,
                r#"{"query":"release health check","limit":1}"#,
            )
            .await
            .expect("live typed-memory recall follows memory-recall-v1");
        let value: Value = serde_json::from_str(&result).expect("result is strict JSON");
        assert_eq!(value["contractVersion"], MEMORY_RECALL_CONTRACT_VERSION);
        assert_eq!(value["memoryType"], "rule");
        assert_eq!(value["trust"]["instructionAuthority"], "none");
    }
