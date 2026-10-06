use super::support::*;
use crate::worker_agents::contracts::*;
use crate::worker_agents::web_search::audit::{
    terminal_audit_attributes, TaskStats, TerminalAuditInput,
};
use crate::worker_agents::web_search::checker::{
    parse_batch, parse_single, Checker, CHECKER_CONTEXT, MAX_CHECK_CHARS,
};
use crate::worker_agents::web_search::profile::web_search_draft;
use crate::worker_agents::web_search::redact::redact_excerpt;
use crate::worker_agents::web_search::sources::{
    host_of, normalize_url, url_hash, SourceBook, SourceEntry, SourceKind, SourceStatus,
    HOST_EXCLUDE_THRESHOLD,
};

#[test]
fn redaction_masks_emails_tokens_and_long_digits() {
    let text = "mail alice@example.com token sk-abcdEFGH1234 hdr Bearer abc.def-ghi card 4111 1111 1111 1111 id 123456789 short 12345678 year 2026";
    let out = redact_excerpt(text);
    for secret in [
        "alice@example.com",
        "sk-abcdEFGH1234",
        "abc.def-ghi",
        "4111",
        "123456789",
    ] {
        assert!(!out.contains(secret), "{out}");
    }
    assert!(out.contains("[redacted]"));
    assert!(out.contains("12345678") && out.contains("2026"));
    assert!(redact_excerpt(&"a ".repeat(500)).chars().count() <= 240);
    assert!(!redact_excerpt("line\nbreak\u{0007}").contains('\n'));
}

#[test]
fn checker_output_is_parsed_strictly_and_fails_closed() {
    let ok = parse_single(r#"{"suspected":false,"categories":[],"excerpt":""}"#);
    assert!(!ok.suspected && !ok.failed_closed);
    // Unknown categories are dropped but suspicion stays.
    let flagged = parse_single(
        r#"{"suspected":true,"categories":["role_redefinition","made_up"],"excerpt":"x"}"#,
    );
    assert!(flagged.suspected);
    assert_eq!(flagged.categories, vec!["role_redefinition".to_string()]);
    // Long excerpt is cut to 160 chars.
    let long = format!(
        r#"{{"suspected":true,"categories":[],"excerpt":"{}"}}"#,
        "e".repeat(400)
    );
    assert_eq!(parse_single(&long).excerpt.chars().count(), 160);
    for bad in [
        "",
        "not json",
        r#"{"suspected":"yes","categories":[]}"#,
        r#"{"suspected":false,"categories":[],"excerpt":"","extra":1}"#,
        r#"{"flagged":[0]}"#,
    ] {
        let verdict = parse_single(bad);
        assert!(verdict.suspected && verdict.failed_closed, "{bad}");
    }
    assert_eq!(
        parse_batch(r#"{"flagged":[1],"categories":["tool_invocation"]}"#, 3).flagged,
        vec![1]
    );
    for bad in [
        "junk",
        r#"{"flagged":[5],"categories":[]}"#,
        r#"{"suspected":true}"#,
    ] {
        let verdict = parse_batch(bad, 3);
        assert!(
            verdict.failed_closed && verdict.flagged == vec![0, 1, 2],
            "{bad}"
        );
    }
}

#[tokio::test]
async fn checker_wraps_text_in_a_nonce_boundary_and_truncation_is_flagged() {
    let model = FakeModel::new(vec![], clean_checker());
    let route = route();
    let cancellation = crate::RunCancellation::default();
    let checker = Checker::new(&model, &route, &cancellation);
    let verdict = checker.check_text("hello world").await;
    assert!(!verdict.suspected);
    let input = model.checker_inputs().remove(0);
    let boundary: Vec<&str> = input
        .lines()
        .filter(|line| line.starts_with("===== UNTRUSTED "))
        .collect();
    assert_eq!(boundary.len(), 2);
    assert_eq!(boundary[0], boundary[1]);
    let again = checker.check_text("hello world").await;
    assert!(!again.suspected);
    let second = model.checker_inputs().remove(1);
    assert_ne!(input, second, "the nonce is fresh per call");

    let long = "y".repeat(MAX_CHECK_CHARS + 1);
    let verdict = checker.check_text(&long).await;
    assert!(verdict.suspected, "truncated input fails closed");
    assert!(model
        .checker_inputs()
        .last()
        .unwrap()
        .contains("[truncated]"));
    let exact = checker.check_text(&"y".repeat(MAX_CHECK_CHARS)).await;
    assert!(!exact.suspected);
    assert_eq!(model.calls.lock().unwrap()[0].0, CHECKER_CONTEXT);
}

#[test]
fn url_helpers_normalize_hosts_and_hashes() {
    assert_eq!(
        host_of("https://WWW.Example.COM/a").as_deref(),
        Some("example.com")
    );
    assert_eq!(
        normalize_url("https://a.example/x#frag").as_deref(),
        Some("https://a.example/x")
    );
    assert_eq!(
        url_hash("https://a.example/x#1"),
        url_hash("https://a.example/x#2")
    );
    assert!(normalize_url("ftp://a.example/x").is_none());
    assert!(normalize_url("https://user:pw@a.example/").is_none());
    assert!(normalize_url(&format!("https://a.example/{}", "p".repeat(2100))).is_none());
    assert_eq!(url_hash("https://a.example/x").unwrap().len(), 64);
}

#[test]
fn host_exclusion_threshold_is_two_and_task_local() {
    assert_eq!(HOST_EXCLUDE_THRESHOLD, 2);
    let mut book = SourceBook::default();
    book.insert(
        SourceKind::SearchHit,
        "https://a.example/1",
        SourceEntry {
            status: SourceStatus::Usable,
            host: "a.example".into(),
            date: None,
        },
    );
    assert!(book.fetch_allowed("https://a.example/1"));
    book.flag_host("a.example");
    assert!(!book.host_excluded("a.example") && book.fetch_allowed("https://a.example/1"));
    book.flag_host("a.example");
    assert!(book.host_excluded("a.example") && !book.fetch_allowed("https://a.example/1"));
}

#[test]
fn profile_draft_matches_the_spec() {
    let draft = web_search_draft();
    assert_eq!(draft.profile_id, WEB_SEARCH_PROFILE_ID);
    assert!(draft.purpose.len() <= 2000 && draft.system_context.len() <= 16384);
    assert_eq!(draft.output_kind, OutputKind::WebClaimsV1);
    assert_eq!(
        draft.completion,
        CompletionCriteria {
            min_items: 1,
            sources_must_be_host_recorded: true
        }
    );
    assert_eq!(draft.tier_policy.max_tier, Tier::Local);
    assert_eq!(draft.tier_policy.cloud, CloudPolicy::Never);
    let keys: Vec<&str> = draft.tools.iter().map(|tool| tool.key.as_str()).collect();
    assert_eq!(keys, ["web_search", "fetch_content"]);
    assert!(draft.purpose.contains("検索") && draft.purpose.contains("latest"));
    assert!(draft.output_schema.is_none());
}

#[test]
fn terminal_audit_attributes_stay_within_two_kib() {
    let stats = TaskStats {
        usable: 2,
        failed: 1,
        excluded: 3,
        categories: (0..200)
            .map(|index| format!("category_{index}_{}", "z".repeat(100)))
            .collect(),
        searches: 2,
        fetches: 3,
    };
    let value = terminal_audit_attributes(&TerminalAuditInput {
        profile_id: "web_search",
        revision: 1,
        tier: "local",
        attempts: 2,
        steps: 5,
        failure_code: Some("no_safe_sources"),
        elapsed_ms: 1234,
        stats: &stats,
    });
    assert!(value.to_string().len() <= 2048);
    assert_eq!(value["failureCode"], "no_safe_sources");
    assert_eq!(value["usable"], 2);
}

#[tokio::test]
async fn task_stats_read_the_ledger_and_the_blocklist_is_never_deleted() {
    use crate::worker_agents::web_search::audit::task_stats;
    let harness = Harness::new("rust release");
    let bad = "https://bad.example/page";
    let tools = FakeTools::default()
        .on("web_search:rust", search_reply(&[(bad, "Bad", "s")]))
        .on(
            &format!("fetch_content:{bad}"),
            fetch_reply(bad, "deny", &["instruction_override"], "blocked", ""),
        );
    let model = FakeModel::new(
        vec![act_search("rust"), act_fetch(bad), act_give_up()],
        clean_checker(),
    );
    let _ = harness.run(&model, &tools).await;
    let stats = task_stats(&harness.writer, "wtask_1").unwrap();
    assert_eq!((stats.usable, stats.failed, stats.excluded), (0, 1, 1));
    assert_eq!(stats.categories, vec!["instruction_override".to_string()]);
    // A second task and a repeated flag leave the permanent row in place.
    harness.insert_task("wtask_2");
    let again = FakeModel::new(vec![act_search("rust"), act_give_up()], clean_checker());
    let _ = harness
        .run_task(
            "wtask_2",
            &again,
            &tools,
            serde_json::json!({"query": "rust"}),
        )
        .await;
    assert_eq!(harness.blocklist_count(), 1);
    // No code path of this module deletes blocklist rows (only the user's IPC removal does).
    let needle = ["DELETE FROM ", "worker_url_blocklist"].concat();
    for source in [
        include_str!("../audit.rs"),
        include_str!("../runner.rs"),
        include_str!("../sources.rs"),
        include_str!("../checker.rs"),
        include_str!("../claims.rs"),
    ] {
        assert!(!source.to_lowercase().contains(&needle.to_lowercase()));
    }
}
