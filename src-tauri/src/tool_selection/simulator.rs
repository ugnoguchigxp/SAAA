//! Headless tool-chain scenarios. The runner calls the production offer, search, and invoke
//! path against an isolated database and a scripted WebView port.

use std::sync::Arc;

use serde_json::{json, Value};
use tempfile::TempDir;

use super::backends::router::BackendRouter;
use super::backends::{FixtureBackend, TechnicalStatus};
use super::contracts::RequestContext;
use super::extraction::UnconfiguredExtractor;
use super::inference::{HashEmbedding, HashReranker};
use super::service::ToolSelectionService;
use crate::artifact_preview::webview_catalog;
use crate::artifact_preview::webview_ops::{AckScript, WebviewHub, WebviewSession};
use crate::persistence::SqliteWriter;
use crate::RunCancellation;

struct Stage {
    name: &'static str,
    passed: bool,
    detail: String,
}

struct ScenarioReport {
    name: String,
    stages: Vec<Stage>,
}

impl ScenarioReport {
    fn pass(&mut self, name: &'static str, detail: impl Into<String>) {
        self.stages.push(Stage {
            name,
            passed: true,
            detail: detail.into(),
        });
    }

    fn fail(&mut self, name: &'static str, detail: impl Into<String>) {
        self.stages.push(Stage {
            name,
            passed: false,
            detail: detail.into(),
        });
    }

    fn ok(&self) -> bool {
        self.stages.iter().all(|stage| stage.passed)
    }
}

struct Harness {
    writer: Arc<SqliteWriter>,
    service: ToolSelectionService,
    hub: Arc<WebviewHub>,
    _dir: TempDir,
}

pub fn run(only: Option<&str>, json_output: bool) -> i32 {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("toolchain simulator runtime failed: {error}");
            return 1;
        }
    };
    let reports = runtime.block_on(run_all(only));
    let failed = reports.iter().any(|report| !report.ok());
    if json_output {
        println!("{}", render_json(&reports));
    } else {
        println!("{}", render_text(&reports));
    }
    crate::artifact_preview::webview_ops::clear_hub_override();
    i32::from(failed)
}

async fn run_all(only: Option<&str>) -> Vec<ScenarioReport> {
    let mut reports = Vec::new();
    for (name, run_one) in catalog() {
        if only.is_some_and(|wanted| wanted != name) {
            continue;
        }
        reports.push(run_one().await);
    }
    if reports.is_empty() {
        let mut report = ScenarioReport {
            name: only.unwrap_or("all").to_string(),
            stages: Vec::new(),
        };
        report.fail("select", "no matching scenario");
        reports.push(report);
    }
    reports
}

fn catalog() -> Vec<(&'static str, ScenarioFn)> {
    vec![
        ("webview-absent", Box::new(|| Box::pin(webview_absent()))),
        ("next-tab-two", Box::new(|| Box::pin(next_tab_two()))),
        ("next-tab-one", Box::new(|| Box::pin(next_tab_one()))),
        ("scroll", Box::new(|| Box::pin(scroll()))),
        ("scroll-loading", Box::new(|| Box::pin(scroll_loading()))),
        ("close-all", Box::new(|| Box::pin(close_all()))),
        ("stale-target", Box::new(|| Box::pin(stale_target()))),
        (
            "conversation-switch",
            Box::new(|| Box::pin(conversation_switch())),
        ),
        (
            "revoked-authority",
            Box::new(|| Box::pin(revoked_authority())),
        ),
        ("rejected-input", Box::new(|| Box::pin(rejected_input()))),
        ("ack-races", Box::new(|| Box::pin(ack_races()))),
        (
            "migration-reregister",
            Box::new(|| Box::pin(migration_reregister())),
        ),
    ]
}

type ScenarioFn = Box<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ScenarioReport> + Send>>
        + Send
        + Sync,
>;

fn open_harness() -> Result<Harness, String> {
    let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let writer = Arc::new(
        crate::open_database_writer(&dir.path().join("catalog.sqlite"))
            .map_err(|error| error.to_string())?,
    );
    writer.write(|connection| {
        connection
            .execute(
                "INSERT INTO conversations(id,title,task_mode,created_at,updated_at) VALUES('conversation',NULL,'conversation','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
                [],
            )
            .map_err(|error| error.to_string())?;
        webview_catalog::ensure_registered(connection, "principal")?;
        Ok(())
    })?;
    let backend = Arc::new(BackendRouter::new(
        Arc::new(FixtureBackend::new()),
        Arc::new(FixtureBackend::new()),
        Arc::new(FixtureBackend::new()),
    ));
    let service = ToolSelectionService::new(
        writer.clone(),
        Arc::new(HashEmbedding::new(384)),
        Arc::new(HashReranker::new()),
        Arc::new(UnconfiguredExtractor),
        backend,
        f64::NEG_INFINITY,
    );
    let hub = Arc::new(WebviewHub::new());
    crate::artifact_preview::webview_ops::install_hub(hub.clone());
    Ok(Harness {
        writer,
        service,
        hub,
        _dir: dir,
    })
}

fn session(conversation_id: &str, labels: &[&str], scrollable: bool) -> WebviewSession {
    WebviewSession {
        conversation_id: conversation_id.to_string(),
        generation: 1,
        labels: labels.iter().map(|label| (*label).to_string()).collect(),
        selected: (!labels.is_empty()).then_some(0),
        scrollable,
        mounted: true,
        other_artifacts: Vec::new(),
    }
}

fn context() -> RequestContext {
    RequestContext::new("principal", "conversation").with_run(Some("run-1".into()))
}

fn note_catalog(report: &mut ScenarioReport, writer: &SqliteWriter) {
    let summary = writer.write(|connection| {
        let (search_bytes, usage_bytes, schema_ok, grants, source_enabled): (
            i64,
            i64,
            i64,
            i64,
            i64,
        ) = connection
            .query_row(
                "SELECT length(r.search_text), length(COALESCE(u.text,'')), \
                        json_valid(r.input_schema_json), \
                        (SELECT COUNT(*) FROM tool_selection_grants g WHERE g.tool_id=c.id), \
                        s.enabled \
                 FROM tool_selection_catalog c \
                 JOIN tool_selection_sources s ON s.id=c.source_id \
                 JOIN tool_selection_revisions r ON r.id=c.current_revision_id \
                 LEFT JOIN tool_selection_usage_pages u ON u.revision_id=r.id AND u.section='usage' AND u.page=1 \
                 WHERE c.id='artifact_webview'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .map_err(|error| error.to_string())?;
        if search_bytes > 0 && usage_bytes > 0 && schema_ok == 1 && grants >= 1 && source_enabled == 1
        {
            Ok(format!(
                "search_text={search_bytes} bytes, usage=present, schema=object, grants={grants}, source=enabled"
            ))
        } else {
            Err("catalog registration is incomplete".into())
        }
    });
    match summary {
        Ok(summary) => report.pass("register", summary),
        Err(error) => report.fail("register", error),
    }
}

fn invocations(writer: &SqliteWriter) -> Result<Vec<(String, String)>, String> {
    writer.write(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT technical_status, COALESCE(error_code, '') FROM tool_selection_invocations ORDER BY started_at",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(rows)
    })
}

async fn webview_absent() -> ScenarioReport {
    let mut report = ScenarioReport {
        name: "webview-absent".into(),
        stages: Vec::new(),
    };
    let Ok(harness) = open_harness() else {
        report.fail("database", "isolated database did not open");
        return report;
    };
    note_catalog(&mut report, &harness.writer);
    match harness.service.offer_direct(&context(), "artifact_webview") {
        Ok(None) => report.pass("offer", "no definition while the webview is closed"),
        Ok(Some(_)) => report.fail("offer", "a closed webview was offered"),
        Err(error) => report.fail("offer", error.message),
    }
    match harness.service.search(&context(), "website tab", 5).await {
        Ok(response) => {
            let leaked = response
                .candidates
                .iter()
                .any(|candidate| candidate.tool_id == "artifact_webview");
            if leaked {
                report.fail("search", "inactive webview appeared in search");
            } else {
                report.pass("search", "search omitted the inactive webview");
            }
        }
        Err(error) => report.fail("search", error.message),
    }
    let rows = invocations(&harness.writer).unwrap_or_default();
    if rows.is_empty() && harness.hub.execute_calls() == 0 {
        report.pass("audit", "no invocation and no backend call");
    } else {
        report.fail("audit", "work was recorded without an offer");
    }
    report
}

async fn next_tab_two() -> ScenarioReport {
    operate(
        "next-tab-two",
        &["one", "two"],
        true,
        AckScript::Apply,
        json!({"operation": "next_tab"}),
        Expect {
            status: Some("succeeded"),
            error: "",
            selected: Some(1),
            labels: Some(&["one", "two"]),
            ui_commands: Some(1),
            execute_calls: Some(1),
            reloads: Some(1),
            invocations: 1,
        },
    )
    .await
}

async fn next_tab_one() -> ScenarioReport {
    operate(
        "next-tab-one",
        &["only"],
        true,
        AckScript::Apply,
        json!({"operation": "next_tab"}),
        Expect {
            status: Some("succeeded"),
            error: "",
            selected: Some(0),
            labels: Some(&["only"]),
            ui_commands: Some(1),
            execute_calls: Some(1),
            reloads: Some(0),
            invocations: 1,
        },
    )
    .await
}

async fn scroll() -> ScenarioReport {
    operate(
        "scroll",
        &["page"],
        true,
        AckScript::Apply,
        json!({"operation": "scroll"}),
        Expect {
            status: Some("succeeded"),
            error: "",
            selected: Some(0),
            labels: Some(&["page"]),
            ui_commands: Some(1),
            execute_calls: Some(1),
            reloads: None,
            invocations: 1,
        },
    )
    .await
}

async fn scroll_loading() -> ScenarioReport {
    operate(
        "scroll-loading",
        &["page"],
        false,
        AckScript::Apply,
        json!({"operation": "scroll"}),
        Expect {
            status: Some("failed"),
            error: "webview-not-scrollable",
            selected: Some(0),
            labels: Some(&["page"]),
            ui_commands: Some(0),
            execute_calls: Some(1),
            reloads: None,
            invocations: 1,
        },
    )
    .await
}

async fn close_all() -> ScenarioReport {
    let mut report = ScenarioReport {
        name: "close-all".into(),
        stages: Vec::new(),
    };
    let Ok(harness) = open_harness() else {
        report.fail("database", "isolated database did not open");
        return report;
    };
    let mut current = session("conversation", &["one", "two"], true);
    current.other_artifacts = vec!["note".into()];
    harness.hub.report_session(current);
    harness
        .hub
        .report_session(session("other", &["kept"], true));
    harness.hub.set_script(Some(AckScript::Apply));
    let Some(execution_ref) = offered(&mut report, &harness) else {
        return report;
    };
    finish_invoke(
        &mut report,
        &harness,
        &execution_ref,
        json!({"operation": "close_all_tabs"}),
        Expect {
            status: Some("succeeded"),
            error: "",
            selected: None,
            labels: Some(&[]),
            ui_commands: Some(1),
            execute_calls: Some(1),
            reloads: None,
            invocations: 1,
        },
    )
    .await;
    if harness.hub.labels("other") == ["kept".to_string()]
        && harness.hub.other_artifacts("conversation") == ["note".to_string()]
    {
        report.pass(
            "other-conversation",
            "other artifacts and the other conversation stayed",
        );
    } else {
        report.fail(
            "other-conversation",
            "a non-website artifact or another conversation was modified",
        );
    }
    report
}

async fn stale_target() -> ScenarioReport {
    let mut report = ScenarioReport {
        name: "stale-target".into(),
        stages: Vec::new(),
    };
    let Ok(harness) = open_harness() else {
        report.fail("database", "isolated database did not open");
        return report;
    };
    harness
        .hub
        .report_session(session("conversation", &["one"], true));
    harness
        .hub
        .report_session(session("other", &["kept"], true));
    let Ok(Some(offer)) = harness.service.offer_direct(&context(), "artifact_webview") else {
        report.fail("offer", "expected an offer before the tab closed");
        return report;
    };
    harness.hub.report_session(WebviewSession {
        conversation_id: "conversation".into(),
        generation: 2,
        mounted: false,
        ..WebviewSession::default()
    });
    harness.hub.reset_counts();
    let error = harness
        .service
        .invoke(
            &context(),
            &offer.execution_ref,
            &json!({"operation": "next_tab"}),
            &RunCancellation::default(),
        )
        .await;
    match error {
        Err(error) if error.code.as_str() == "unavailable" => {
            report.pass("closed", "closed tab rejected before a backend call");
        }
        Ok(_) => report.fail("closed", "closed tab still ran"),
        Err(error) => report.fail("closed", error.message),
    }
    if harness.hub.execute_calls() == 0 && harness.hub.labels("other") == ["kept".to_string()] {
        report.pass(
            "isolation",
            "no side effect on the backend or the other conversation",
        );
    } else {
        report.fail("isolation", "a rejected call changed state");
    }
    let rows = invocations(&harness.writer).unwrap_or_default();
    if rows.is_empty() {
        report.pass("audit", "admission failure wrote no invocation");
    } else {
        report.fail("audit", format!("{} invocation rows", rows.len()));
    }
    report
}

async fn revoked_authority() -> ScenarioReport {
    let mut report = ScenarioReport {
        name: "revoked-authority".into(),
        stages: Vec::new(),
    };
    let Ok(harness) = open_harness() else {
        report.fail("database", "isolated database did not open");
        return report;
    };
    harness
        .hub
        .report_session(session("conversation", &["one"], true));
    let cases = [
        (
            "grant",
            "DELETE FROM tool_selection_grants WHERE tool_id='artifact_webview'",
            "not-authorized",
        ),
        (
            "source",
            "UPDATE tool_selection_sources SET enabled=0 WHERE kind='artifact_webview'",
            "stale-reference",
        ),
        ("revision", "", "stale-reference"),
    ];
    for (name, sql, expected) in cases {
        let _ = harness.writer.write(|connection| {
            webview_catalog::ensure_registered(connection, "principal")?;
            connection
                .execute(
                    "UPDATE tool_selection_sources SET enabled=1 WHERE kind='artifact_webview'",
                    [],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        });
        harness
            .hub
            .report_session(session("conversation", &["one"], true));
        let Ok(Some(offer)) = harness.service.offer_direct(&context(), "artifact_webview") else {
            report.fail(name, "offer failed before revocation");
            continue;
        };
        if name == "revision" {
            let _ = harness.writer.write(|connection| {
                webview_catalog::ensure_registered_with_note(connection, "principal", " revised")
            });
        } else if let Err(error) = harness.writer.write(|connection| {
            connection
                .execute(sql, [])
                .map_err(|error| error.to_string())
                .map(|_| ())
        }) {
            report.fail(name, error);
            continue;
        }
        harness.hub.reset_counts();
        let before = invocations(&harness.writer)
            .map(|rows| rows.len())
            .unwrap_or(0);
        let outcome = harness
            .service
            .invoke(
                &context(),
                &offer.execution_ref,
                &json!({"operation": "next_tab"}),
                &RunCancellation::default(),
            )
            .await;
        let code = match &outcome {
            Err(error) => error.code.as_str(),
            Ok(response) => response.status.as_str(),
        };
        let after = invocations(&harness.writer)
            .map(|rows| rows.len())
            .unwrap_or(0);
        if code == expected && harness.hub.execute_calls() == 0 && after == before {
            report.pass(
                name,
                format!("{expected}, backend not called, invocation unchanged"),
            );
        } else {
            report.fail(
                name,
                format!(
                    "code={code} expected={expected} backend={} invocations={after}",
                    harness.hub.execute_calls()
                ),
            );
        }
    }
    report
}

async fn rejected_input() -> ScenarioReport {
    let mut report = ScenarioReport {
        name: "rejected-input".into(),
        stages: Vec::new(),
    };
    let Ok(harness) = open_harness() else {
        report.fail("database", "isolated database did not open");
        return report;
    };
    harness
        .hub
        .report_session(session("conversation", &["one"], true));
    let Ok(Some(offer)) = harness.service.offer_direct(&context(), "artifact_webview") else {
        report.fail("offer", "expected an offer");
        return report;
    };
    let cases = [
        ("schema", json!({"index": 0}), "invalid-input"),
        (
            "index",
            json!({"operation": "select_tab", "index": 4}),
            "invalid-input",
        ),
        ("unoffered", json!({"operation": "next_tab"}), "not-found"),
    ];
    for (name, arguments, expected) in cases {
        harness.hub.reset_counts();
        let reference = if name == "unoffered" {
            "missing-ref"
        } else {
            offer.execution_ref.as_str()
        };
        let before = invocations(&harness.writer)
            .map(|rows| rows.len())
            .unwrap_or(0);
        let outcome = harness
            .service
            .invoke(
                &context(),
                reference,
                &arguments,
                &RunCancellation::default(),
            )
            .await;
        let code = match outcome {
            Err(error) => error.code.as_str().to_string(),
            Ok(_) => "succeeded".to_string(),
        };
        let after = invocations(&harness.writer)
            .map(|rows| rows.len())
            .unwrap_or(0);
        if code == expected && harness.hub.execute_calls() == 0 && after == before {
            report.pass(name, format!("stopped at {expected} before the backend"));
        } else {
            report.fail(
                name,
                format!("code={code} backend={}", harness.hub.execute_calls()),
            );
        }
    }
    report
}

async fn ack_races() -> ScenarioReport {
    let mut report = ScenarioReport {
        name: "ack-races".into(),
        stages: Vec::new(),
    };
    let Ok(harness) = open_harness() else {
        report.fail("database", "isolated database did not open");
        return report;
    };
    harness
        .hub
        .report_session(session("conversation", &["one", "two"], true));
    harness.hub.set_script(Some(AckScript::Timeout));
    let Ok(Some(offer)) = harness.service.offer_direct(&context(), "artifact_webview") else {
        report.fail("offer", "expected an offer");
        return report;
    };
    let timed = harness
        .service
        .invoke(
            &context(),
            &offer.execution_ref,
            &json!({"operation": "next_tab"}),
            &RunCancellation::default(),
        )
        .await;
    let timeout_ok = matches!(
        &timed,
        Ok(response) if response.status == TechnicalStatus::Failed && response.error_code == Some("webview-timeout")
    );
    if timeout_ok && harness.hub.selected("conversation") == Some(0) {
        report.pass(
            "timeout",
            "timeout is a failed invocation and does not move the tab",
        );
    } else {
        report.fail(
            "timeout",
            "timeout was not recorded as a failed unchanged tab",
        );
    }
    if let Some(request_id) = harness.hub.last_request_id() {
        harness.hub.complete_request(&request_id, true);
    }
    if harness.hub.selected("conversation") == Some(0) {
        report.pass("late-ack", "an ack after timeout does not move the tab");
    } else {
        report.fail("late-ack", "a late ack changed the selection");
    }
    harness.hub.set_script(Some(AckScript::DuplicateApply));
    let Ok(Some(again)) = harness.service.offer_direct(&context(), "artifact_webview") else {
        report.fail("reoffer", "could not re-offer after timeout");
        return report;
    };
    let duplicated = harness
        .service
        .invoke(
            &context(),
            &again.execution_ref,
            &json!({"operation": "next_tab"}),
            &RunCancellation::default(),
        )
        .await;
    if matches!(&duplicated, Ok(response) if response.status == TechnicalStatus::Succeeded)
        && harness.hub.selected("conversation") == Some(1)
    {
        report.pass(
            "duplicate-ack",
            "a repeated ack did not advance an extra tab",
        );
    } else {
        report.fail(
            "duplicate-ack",
            "duplicate ack changed the following operation",
        );
    }
    harness.hub.set_script(Some(AckScript::DelayedApply));
    let before = harness.hub.selected("conversation");
    let Ok(Some(delayed)) = harness.service.offer_direct(&context(), "artifact_webview") else {
        report.fail("delay", "could not offer after the duplicate ack");
        return report;
    };
    let delayed_result = harness
        .service
        .invoke(
            &context(),
            &delayed.execution_ref,
            &json!({"operation": "next_tab"}),
            &RunCancellation::default(),
        )
        .await;
    let moved_once = harness.hub.selected("conversation") != before;
    if matches!(&delayed_result, Ok(response) if response.status == TechnicalStatus::Succeeded)
        && moved_once
        && harness.hub.virtual_elapsed_ms() == 200
    {
        report.pass(
            "delayed-ack",
            "virtual clock 200ms; one ack applied and the extra ack was ignored",
        );
    } else {
        report.fail("delayed-ack", "delayed ack did not apply exactly once");
    }
    report
}

async fn conversation_switch() -> ScenarioReport {
    let mut report = ScenarioReport {
        name: "conversation-switch".into(),
        stages: Vec::new(),
    };
    let Ok(harness) = open_harness() else {
        report.fail("database", "isolated database did not open");
        return report;
    };
    harness
        .hub
        .report_session(session("conversation", &["one"], true));
    harness
        .hub
        .report_session(session("other", &["kept"], true));
    let Ok(Some(offer)) = harness.service.offer_direct(&context(), "artifact_webview") else {
        report.fail("offer", "expected an offer");
        return report;
    };
    let switched = RequestContext::new("principal", "other").with_run(Some("run-1".into()));
    harness.hub.reset_counts();
    let outcome = harness
        .service
        .invoke(
            &switched,
            &offer.execution_ref,
            &json!({"operation": "next_tab"}),
            &RunCancellation::default(),
        )
        .await;
    if matches!(&outcome, Err(error) if error.code.as_str() == "unavailable")
        && harness.hub.execute_calls() == 0
        && harness.hub.selected("other") == Some(0)
        && harness.hub.selected("conversation") == Some(0)
    {
        report.pass(
            "switch",
            "the old reference does not touch either conversation",
        );
    } else {
        report.fail(
            "switch",
            "switching conversation still ran the old reference",
        );
    }
    harness.hub.report_session(WebviewSession {
        generation: 4,
        ..session("conversation", &["one"], true)
    });
    let bumped = harness
        .service
        .invoke(
            &context(),
            &offer.execution_ref,
            &json!({"operation": "next_tab"}),
            &RunCancellation::default(),
        )
        .await;
    if matches!(&bumped, Err(error) if error.message == "webview-generation-changed")
        && harness.hub.execute_calls() == 0
    {
        report.pass(
            "generation",
            "a moved generation is rejected before the backend",
        );
    } else {
        report.fail("generation", "a new generation was still executed");
    }
    report
}

async fn migration_reregister() -> ScenarioReport {
    let mut report = ScenarioReport {
        name: "migration-reregister".into(),
        stages: Vec::new(),
    };
    let Ok(dir) = tempfile::tempdir() else {
        report.fail("database", "temp dir failed");
        return report;
    };
    let path = dir.path().join("migrated.sqlite");
    let writer = match crate::open_database_writer(&path) {
        Ok(writer) => writer,
        Err(_) => {
            report.fail("fixture", "could not build the pre-migration database");
            return report;
        }
    };
    if let Err(error) = writer.write(|connection| {
        connection
            .pragma_update(None, "user_version", 40)
            .map_err(|error| error.to_string())
    }) {
        report.fail("fixture", error.to_string());
        return report;
    }
    drop(writer);
    let writer = match crate::open_database_writer(&path) {
        Ok(writer) => writer,
        Err(error) => {
            report.fail("migrate", error);
            return report;
        }
    };
    let registered = writer.write(|connection| {
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if version <= 0 {
            return Err("schema version was not migrated".into());
        }
        webview_catalog::ensure_registered(connection, "principal")?;
        let first: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM tool_selection_revisions WHERE tool_id='artifact_webview'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        webview_catalog::ensure_registered(connection, "principal")?;
        let second: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM tool_selection_revisions WHERE tool_id='artifact_webview'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let definition = webview_catalog::offered_definition(connection, "principal")?;
        if first == 1 && second == 1 && definition.is_some() {
            Ok(())
        } else {
            Err(format!("revisions {first} -> {second}"))
        }
    });
    match registered {
        Ok(()) => report.pass("reregister", "migration kept one current revision"),
        Err(error) => report.fail("reregister", error),
    }
    report
}

struct Expect {
    status: Option<&'static str>,
    error: &'static str,
    selected: Option<usize>,
    labels: Option<&'static [&'static str]>,
    ui_commands: Option<usize>,
    execute_calls: Option<usize>,
    invocations: usize,
    reloads: Option<usize>,
}

async fn operate(
    name: &'static str,
    labels: &[&str],
    scrollable: bool,
    script: AckScript,
    arguments: Value,
    expect: Expect,
) -> ScenarioReport {
    let mut report = ScenarioReport {
        name: name.into(),
        stages: Vec::new(),
    };
    let Ok(harness) = open_harness() else {
        report.fail("database", "isolated database did not open");
        return report;
    };
    harness
        .hub
        .report_session(session("conversation", labels, scrollable));
    note_catalog(&mut report, &harness.writer);
    harness.hub.set_script(Some(script));
    let Some(execution_ref) = offered(&mut report, &harness) else {
        return report;
    };
    searched(&mut report, &harness).await;
    finish_invoke(&mut report, &harness, &execution_ref, arguments, expect).await;
    report
}

fn offered(report: &mut ScenarioReport, harness: &Harness) -> Option<String> {
    match harness.service.offer_direct(&context(), "artifact_webview") {
        Ok(Some(offer)) => {
            report.pass("offer", "current revision offered");
            Some(offer.execution_ref)
        }
        Ok(None) => {
            report.fail("offer", "tool was hidden");
            None
        }
        Err(error) => {
            report.fail("offer", error.message);
            None
        }
    }
}

async fn searched(report: &mut ScenarioReport, harness: &Harness) {
    match harness.service.search(&context(), "website tab", 5).await {
        Ok(response) => {
            if let Some(candidate) = response
                .candidates
                .iter()
                .find(|candidate| candidate.tool_id == "artifact_webview")
            {
                report.pass("search", "current webview revision is searchable");
                match harness
                    .service
                    .describe(&context(), &candidate.reference, "contract", None)
                {
                    Ok(described) if described.execution_ref.is_some() => {
                        report.pass("describe", "contract matches the current revision");
                    }
                    Ok(_) => report.fail("describe", "usage did not issue an execution reference"),
                    Err(error) => report.fail("describe", error.message),
                }
            } else {
                report.fail("search", "offered tool was missing from search");
            }
        }
        Err(error) => report.fail("search", error.message),
    }
}

async fn finish_invoke(
    report: &mut ScenarioReport,
    harness: &Harness,
    execution_ref: &str,
    arguments: Value,
    expect: Expect,
) {
    let outcome = harness
        .service
        .invoke(
            &context(),
            execution_ref,
            &arguments,
            &RunCancellation::default(),
        )
        .await;
    let (status, error) = match &outcome {
        Ok(response) => (
            response.status.as_str().to_string(),
            response.error_code.unwrap_or("").to_string(),
        ),
        Err(error) => ("error".to_string(), error.code.as_str().to_string()),
    };
    let status_ok = expect.status.is_none_or(|expected| expected == status);
    let error_ok = error == expect.error;
    if status_ok && error_ok {
        report.pass("invoke", format!("{status} {error}"));
    } else {
        report.fail("invoke", format!("status={status} error={error}"));
    }
    if harness.hub.selected("conversation") == expect.selected {
        report.pass("tabs", "selection matches");
    } else {
        report.fail("tabs", "selection diverged");
    }
    if let Some(labels) = expect.labels {
        let actual = harness.hub.labels("conversation");
        let wanted: Vec<String> = labels.iter().map(|label| (*label).to_string()).collect();
        if actual == wanted {
            report.pass("labels", "website tabs match");
        } else {
            report.fail("labels", "website tabs diverged");
        }
    }
    if harness.hub.ui_commands() == expect.ui_commands.unwrap_or(0)
        && harness.hub.execute_calls() == expect.execute_calls.unwrap_or(0)
    {
        report.pass("backend", "ui and backend counts match");
    } else {
        report.fail(
            "backend",
            format!(
                "ui={} execute={}",
                harness.hub.ui_commands(),
                harness.hub.execute_calls()
            ),
        );
    }
    if let Some(reloads) = expect.reloads {
        if harness.hub.reloads() == reloads {
            report.pass("reload", "reload count matches");
        } else {
            report.fail("reload", format!("reloads={}", harness.hub.reloads()));
        }
    }
    match invocations(&harness.writer) {
        Ok(rows) if rows.len() == expect.invocations => {
            report.pass("audit", format!("{} invocation", rows.len()));
        }
        Ok(rows) => report.fail("audit", format!("{} invocations", rows.len())),
        Err(error) => report.fail("audit", error),
    }
    let _ = outcome;
}

fn render_text(reports: &[ScenarioReport]) -> String {
    let mut lines = Vec::new();
    for report in reports {
        let mark = if report.ok() { "pass" } else { "fail" };
        lines.push(format!("{mark} {}", report.name));
        for stage in &report.stages {
            let mark = if stage.passed { "pass" } else { "fail" };
            lines.push(format!("  {mark} {} — {}", stage.name, stage.detail));
        }
    }
    lines.join("\n")
}

fn render_json(reports: &[ScenarioReport]) -> String {
    let body = reports
        .iter()
        .map(|report| {
            json!({
                "scenario": report.name,
                "passed": report.ok(),
                "stages": report.stages.iter().map(|stage| json!({
                    "name": stage.name,
                    "passed": stage.passed,
                    "detail": stage.detail,
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    json!({"passed": reports.iter().all(ScenarioReport::ok), "scenarios": body}).to_string()
}
