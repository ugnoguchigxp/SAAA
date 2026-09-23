use super::*;
#[test]
pub(super) fn world_g1_08_policy_carries_the_static_world_reading_rules() {
    let connection = database();
    insert(
        &connection,
        0,
        "user",
        "「Speculative Decoding」は今の目標にどう関係しますか？",
    );

    let window = build(&connection, "primary", "message-0").expect("context builds");
    let system: Vec<_> = window
        .messages
        .iter()
        .filter(|message| message.role == "system")
        .collect();
    assert_eq!(system.len(), 1, "exactly one trusted system template");
    let policy = &system[0].content;
    for phrase in [
        "World model policy",
        "instructionAuthority=none",
        "correlates_with",
        "unknown_seed",
        "ambiguous_seed",
        "stale, pending and capacity-omitted",
    ] {
        assert!(policy.contains(phrase), "policy is missing {phrase}");
    }
    // The user question is data, never promoted into the trusted system template.
    assert!(!policy.contains("Speculative Decoding"));
}
