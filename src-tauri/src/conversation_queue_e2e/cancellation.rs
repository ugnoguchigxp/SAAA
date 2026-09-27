//! Delayed-provider fixtures exercise cancellation through the real queue workers.
use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Notify;

const SLOW: &str = "fixture: slow request";
const REPLACE: &str = "fixture: replace request";
const CANCEL: &str = "fixture: cancel request";

#[derive(Default)]
pub(super) struct Control {
    routing_started: AtomicBool,
    release_routing: Notify,
    thinking_started: AtomicBool,
    thinking_count: AtomicUsize,
    thinking_released: AtomicUsize,
}

pub(super) async fn respond(fixture: &Fixture, path: &str, body: &Value) -> Option<String> {
    let text = body["messages"].as_array()?.last()?["content"].as_str()?;
    if ![SLOW, REPLACE, CANCEL].contains(&text) {
        return None;
    }
    let control = &fixture.cancellation;
    if path.starts_with("/llm/") {
        if text == SLOW {
            let generation = control.thinking_count.fetch_add(1, Ordering::SeqCst) + 1;
            control.thinking_started.store(true, Ordering::SeqCst);
            while control.thinking_released.load(Ordering::SeqCst) < generation {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            // A cancelled request must never execute this follow-up tool.
            return Some(json!({"action":"web_search","query":"must-not-run"}).to_string());
        }
        return Some(json!({"action":"answer","content":"置換後の結果です。"}).to_string());
    }
    if body.to_string().contains("会話の入口") {
        if text == SLOW {
            control.routing_started.store(true, Ordering::SeqCst);
            control.release_routing.notified().await;
            return Some(json!({"route":"think","reply":null}).to_string());
        }
        // Keep A active while B decides to replace/cancel it.
        wait_until(|| Ok(control.thinking_started.load(Ordering::SeqCst)))
            .await
            .expect("old request reaches Ornith");
        assert!(
            body.to_string().contains("処理中の前の依頼"),
            "late-created downstream jobs must still identify the prior input"
        );
        return Some(
            if text == REPLACE {
                json!({"route":"replace","reply":null})
            } else {
                json!({"route":"cancel","reply":"中止しました。"})
            }
            .to_string(),
        );
    }
    Some("置換後の回答です。".into())
}

async fn wait_until(mut ready: impl FnMut() -> Result<bool, String>) -> Result<(), String> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !ready()? {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        Ok(())
    })
    .await
    .map_err(|_| "cancelled request blocked the next job".to_string())?
}

pub(super) async fn verify(state: &AppState, fixture: &Fixture) -> Result<(), String> {
    for (index, action) in [REPLACE, CANCEL, "explicit"].iter().enumerate() {
        let old_key = format!("cancel-old-{index}");
        let new_key = format!("cancel-new-{index}");
        let control = &fixture.cancellation;
        control.routing_started.store(false, Ordering::SeqCst);
        control.thinking_started.store(false, Ordering::SeqCst);
        conversation_check::queue_runtime::enqueue_text(state, old_key.clone(), SLOW.into())?;
        state.conversation_queue_wake.notify_waiters();
        wait_until(|| Ok(control.routing_started.load(Ordering::SeqCst))).await?;
        if *action != "explicit" {
            // B arrives while A is still routing: Ornith(A) will have a later rowid.
            conversation_check::queue_runtime::enqueue_text(
                state,
                new_key.clone(),
                (*action).into(),
            )?;
        }
        control.release_routing.notify_one();
        if *action == "explicit" {
            wait_until(|| Ok(control.thinking_started.load(Ordering::SeqCst))).await?;
            conversation_check::queue_runtime::cancel_input(state, &old_key)?;
            conversation_check::queue_runtime::enqueue_text(
                state,
                new_key.clone(),
                "こんにちは".into(),
            )?;
        }
        state.conversation_queue_wake.notify_waiters();
        let finished = wait_until(|| state.sqlite_readers.read(|db| {
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE job_key=?1 AND kind='speech' AND state='completed')",
                [&new_key], |row| row.get(0),
            ).map_err(crate::database_error)
        })).await;
        // Always let the local server handler exit, even if the regression failed.
        control.thinking_released.store(index + 1, Ordering::SeqCst);
        finished?;
        // A response arriving after cancellation must not run another tool or save an answer.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        state.sqlite_readers.read(|db| {
            let status: String = db.query_row("SELECT status FROM runtime_runs WHERE id=?1",
                [format!("run_{old_key}")], |row| row.get(0)).map_err(crate::database_error)?;
            if status != "cancelled" { return Err(format!("old run is still {status}")); }
            let count: i64 = db.query_row(
                "SELECT count(*) FROM conversation_messages WHERE id=?1", [format!("reply_{old_key}")],
                |row| row.get(0),
            ).map_err(crate::database_error)?;
            if count != 0 { return Err("cancelled request published an answer".into()); }
            let active: i64 = db.query_row(
                "SELECT count(*) FROM task_queue_jobs WHERE job_key=?1 AND state IN ('queued','running')",
                [&old_key], |row| row.get(0),
            ).map_err(crate::database_error)?;
            if active != 0 { return Err("cancelled request still has active jobs".into()); }
            Ok(())
        })?;
        if fixture
            .searches
            .lock()
            .map_err(|_| "search lock")?
            .iter()
            .any(|query| query == "must-not-run")
        {
            return Err("cancelled request executed a tool".into());
        }
    }
    Ok(())
}
