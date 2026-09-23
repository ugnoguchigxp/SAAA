use super::*;
    const EXPERIENCE_FIXTURE: &str =
        include_str!("../../../tests/fixtures/memory-recall-v1/experience.json");
    const RULE_FIXTURE: &str = include_str!("../../../tests/fixtures/memory-recall-v1/rule.json");
    const SKILL_FIXTURE: &str = include_str!("../../../tests/fixtures/memory-recall-v1/skill.json");
    const NO_CONTENT_FIXTURE: &str =
        include_str!("../../../tests/fixtures/memory-recall-v1/no-content.json");
    const INVALID_CASES_FIXTURE: &str =
        include_str!("../../../tests/fixtures/memory-recall-v1/invalid-cases.json");

    fn call_result(text: &str) -> Value {
        json!({"content": [{"type": "text", "text": text}]})
    }

    #[test]
    fn official_output_fixtures_are_accepted_and_canonicalized() {
        for (memory_type, fixture) in [
            (TypedMemoryType::Experience, EXPERIENCE_FIXTURE),
            (TypedMemoryType::Rule, RULE_FIXTURE),
            (TypedMemoryType::Skill, SKILL_FIXTURE),
            (TypedMemoryType::Rule, NO_CONTENT_FIXTURE),
        ] {
            let parsed = parse_call_tool_result(memory_type, &call_result(fixture))
                .expect("official fixture parses");
            assert_eq!(
                serde_json::from_str::<Value>(&parsed).expect("canonical output is JSON"),
                serde_json::from_str::<Value>(fixture).expect("fixture is JSON")
            );
        }
    }

    #[test]
    fn official_invalid_input_cases_are_rejected() {
        let cases: Value = serde_json::from_str(INVALID_CASES_FIXTURE).expect("fixture parses");
        for case in cases.as_array().expect("cases are an array") {
            let tool = case["tool"].as_str().expect("tool exists");
            let arguments = serde_json::to_string(&case["arguments"]).expect("arguments encode");
            assert_eq!(
                parse_typed_recall_arguments(tool, &arguments),
                Err(if is_typed_recall_tool(tool) {
                    TypedRecallContractError::InvalidInput
                } else {
                    TypedRecallContractError::UnsupportedTool
                }),
                "case {}",
                case["name"]
            );
        }
    }

    #[test]
    fn inputs_are_normalized_without_expanding_scope() {
        let call = parse_typed_recall_arguments(
            RECALL_RULE_TOOL_NAME,
            r#"{"query":"  release  ","domains":[" Rust "],"polarities":["negative"],"limit":5}"#,
        )
        .expect("input parses");
        assert_eq!(call.memory_type, TypedMemoryType::Rule);
        assert_eq!(call.arguments["query"], "release");
        assert_eq!(call.arguments["domains"], json!(["Rust"]));
        assert!(call.arguments.get("projectRef").is_none());

        assert_eq!(
            parse_typed_recall_arguments(
                RECALL_SKILL_TOOL_NAME,
                r#"{"query":"release","intentTags":["Deploy","deploy"]}"#
            ),
            Err(TypedRecallContractError::InvalidInput)
        );
    }

    #[test]
    fn tool_catalog_is_exact_and_each_schema_is_closed() {
        let definitions = typed_recall_tool_definitions();
        assert_eq!(definitions.len(), 3);
        let names = definitions
            .iter()
            .map(|definition| {
                definition
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .expect("name exists")
            })
            .collect::<Vec<_>>();
        assert_eq!(names, TYPED_RECALL_TOOL_NAMES);
        for definition in definitions {
            assert_eq!(
                definition.pointer("/function/parameters/additionalProperties"),
                Some(&Value::Bool(false))
            );
            assert!(definition
                .pointer("/function/description")
                .and_then(Value::as_str)
                .is_some_and(|description| description.contains("untrusted")));
        }
    }

    #[test]
    fn output_rejects_unknown_fields_wrong_types_and_inconsistent_no_content() {
        let mut unknown: Value = serde_json::from_str(RULE_FIXTURE).expect("fixture parses");
        unknown["sourceRef"] = json!("forbidden");
        assert_eq!(
            parse_call_tool_result(TypedMemoryType::Rule, &call_result(&unknown.to_string())),
            Err(TypedRecallContractError::InvalidResponse)
        );

        let mut wrong_type: Value = serde_json::from_str(RULE_FIXTURE).expect("fixture parses");
        wrong_type["memoryType"] = json!("skill");
        assert_eq!(
            parse_call_tool_result(TypedMemoryType::Rule, &call_result(&wrong_type.to_string())),
            Err(TypedRecallContractError::InvalidResponse)
        );

        let mut inconsistent: Value = serde_json::from_str(RULE_FIXTURE).expect("fixture parses");
        inconsistent["noContent"] = json!(true);
        assert_eq!(
            parse_call_tool_result(
                TypedMemoryType::Rule,
                &call_result(&inconsistent.to_string())
            ),
            Err(TypedRecallContractError::InvalidResponse)
        );

        let mut unknown_item: Value = serde_json::from_str(RULE_FIXTURE).expect("fixture parses");
        unknown_item["items"][0]["sourceRef"] = json!("forbidden");
        assert_eq!(
            parse_call_tool_result(
                TypedMemoryType::Rule,
                &call_result(&unknown_item.to_string())
            ),
            Err(TypedRecallContractError::InvalidResponse)
        );

        let mut invalid_enum: Value = serde_json::from_str(RULE_FIXTURE).expect("fixture parses");
        invalid_enum["items"][0]["polarity"] = json!("mandatory");
        assert_eq!(
            parse_call_tool_result(
                TypedMemoryType::Rule,
                &call_result(&invalid_enum.to_string())
            ),
            Err(TypedRecallContractError::InvalidResponse)
        );

        let mut missing_required: Value =
            serde_json::from_str(RULE_FIXTURE).expect("fixture parses");
        missing_required
            .as_object_mut()
            .expect("fixture is an object")
            .remove("trust");
        assert_eq!(
            parse_call_tool_result(
                TypedMemoryType::Rule,
                &call_result(&missing_required.to_string())
            ),
            Err(TypedRecallContractError::InvalidResponse)
        );

        let mut null_optional: Value =
            serde_json::from_str(EXPERIENCE_FIXTURE).expect("fixture parses");
        null_optional["items"][0]["action"] = Value::Null;
        assert_eq!(
            parse_call_tool_result(
                TypedMemoryType::Experience,
                &call_result(&null_optional.to_string())
            ),
            Err(TypedRecallContractError::InvalidResponse)
        );
    }

    #[test]
    fn no_content_and_truncated_are_normal_success_results() {
        parse_call_tool_result(TypedMemoryType::Rule, &call_result(NO_CONTENT_FIXTURE))
            .expect("no-content is successful");

        let mut truncated: Value = serde_json::from_str(RULE_FIXTURE).expect("fixture parses");
        truncated["truncated"] = json!(true);
        let result =
            parse_call_tool_result(TypedMemoryType::Rule, &call_result(&truncated.to_string()))
                .expect("truncated is successful");
        assert_eq!(
            serde_json::from_str::<Value>(&result).expect("result is JSON")["truncated"],
            true
        );
    }

    #[test]
    fn structured_content_and_oversized_items_are_rejected() {
        let structured = json!({
            "content": [{"type": "text", "text": RULE_FIXTURE}],
            "structuredContent": {}
        });
        assert_eq!(
            parse_call_tool_result(TypedMemoryType::Rule, &structured),
            Err(TypedRecallContractError::InvalidResponse)
        );

        let huge = json!({
            "contractVersion": MEMORY_RECALL_CONTRACT_VERSION,
            "memoryType": "rule",
            "trust": {
                "trustClass": "untrusted_memory_evidence",
                "instructionAuthority": "none"
            },
            "items": [{"title": "x", "rule": "x".repeat(RULE_ITEM_BYTES), "polarity": "positive"}],
            "noContent": false,
            "truncated": false
        });
        assert!(matches!(
            parse_call_tool_result(TypedMemoryType::Rule, &call_result(&huge.to_string())),
            Err(TypedRecallContractError::ResponseTooLarge)
        ));

        let skill_item = json!({
            "title": "bounded item",
            "useWhen": "release",
            "workflow": ["x".repeat(2_400)],
            "verification": ["verify"],
            "avoid": ["skip"]
        });
        let oversized_result = json!({
            "contractVersion": MEMORY_RECALL_CONTRACT_VERSION,
            "memoryType": "skill",
            "trust": {
                "trustClass": "untrusted_memory_evidence",
                "instructionAuthority": "none"
            },
            "items": [skill_item.clone(), skill_item.clone(), skill_item.clone(), skill_item],
            "noContent": false,
            "truncated": false
        });
        assert_eq!(
            parse_call_tool_result(
                TypedMemoryType::Skill,
                &call_result(&oversized_result.to_string())
            ),
            Err(TypedRecallContractError::ResponseTooLarge)
        );
    }
