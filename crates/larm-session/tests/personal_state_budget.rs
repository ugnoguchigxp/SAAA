use saaa_larm_session::personal_state::Capability;

fn capability(context: u64, output: u64, margin: u64) -> Capability {
    Capability {
        contract_version: "larm-personal-state.v1".into(),
        boot_epoch: uuid::Uuid::new_v4().to_string(),
        subject_digest: "a".repeat(64),
        allocation_id: "allocation".into(),
        runtime: "qwen-general".into(),
        release: "release".into(),
        lease_epoch: 1,
        lease_expires_at: "2099-01-01T00:00:00Z".into(),
        credential_expires_at: "2099-01-01T00:00:00Z".into(),
        tokenizer_digest: "b".repeat(64),
        chat_template_digest: "c".repeat(64),
        context_limit_tokens: context,
        output_reserve_tokens: output,
        safety_margin_tokens: margin,
        source_token_limit: 20_000_000,
        max_source_bytes: 262_144,
        max_total_source_bytes: 1_048_576,
        max_materialized_bytes: 262_144,
        scopes: [
            "context.source.provision",
            "context.measure",
            "context.view.create",
            "context.generate",
            "context.attempt.cancel",
            "context.forget",
            "context.operation.read",
        ]
        .map(str::to_string)
        .into(),
    }
}

#[test]
fn qwen38_budget_is_derived_from_the_api_contract() {
    let capability = capability(230_400, 4_096, 1_976);
    assert_eq!(capability.input_limit(), Ok(224_328));
    assert_eq!(capability.output_limit(), Ok(4_096));
}

#[test]
fn provider_budget_is_not_replaced_by_a_local_cap() {
    let capability = capability(262_144, 32_768, 4_096);
    assert_eq!(capability.input_limit(), Ok(225_280));
    assert_eq!(capability.output_limit(), Ok(32_768));
}

#[test]
fn smaller_provider_limits_remain_fail_closed() {
    let smaller = capability(64_000, 2_000, 1_000);
    assert_eq!(smaller.input_limit(), Ok(61_000));
    assert_eq!(smaller.output_limit(), Ok(2_000));

    let invalid = capability(4_096, 4_096, 0);
    assert!(invalid.input_limit().is_err());
}
