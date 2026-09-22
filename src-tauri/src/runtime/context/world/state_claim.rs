//! Typed claims are data. Only matching, authorized source payloads produce display text.
use saaa_personal_state_core::world::{frame_sources::*, runtime_frame::WorldFrame};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StateClaim {
    pub kind: WorldSourceKind,
    pub source_ref: String,
    pub source_version_or_digest: String,
    pub value: WorldSourcePayload,
    pub as_of_ms: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    claims: Vec<StateClaim>,
}
pub(crate) const INSTRUCTION: &str = r#"This turn asks only for current state. Return JSON only: {"claims":[{"kind":"situation/coding/delegation/schedule","source_ref":"source_id from the current World Frame","source_version_or_digest":"digest from that entry","value":the exact payload object from that entry,"as_of_ms":that entry's as_of_ms}]}. Select only entries relevant to the user's question, with availability:available and an authorized owner scope. Do not invoke tools or add prose. Do not interpret settled as success, correlation as causation, or an inferred meeting as confirmed fact. If no entry supports an answer return {"claims":[]}. Never invent values or source references."#;
pub(crate) fn render_for_query(
    raw: &str,
    frame: &WorldFrame,
    question: &str,
) -> Result<String, String> {
    if raw.len() > 16384 {
        return Err("state-claim-budget".into());
    }
    let answer: Answer = serde_json::from_str(raw).map_err(|_| "state-claim-schema")?;
    for claim in &answer.claims {
        if !super::host_answer::matches_query(question, claim.kind) {
            return Err("state-claim-wrong-subject".into());
        }
        if claim.kind == WorldSourceKind::Schedule {
            let earliest = frame
                .sources
                .iter()
                .filter(|g| {
                    g.kind == WorldSourceKind::Schedule
                        && g.availability == WorldSourceAvailability::Available
                })
                .flat_map(|g| &g.entries)
                .filter(|e| e.availability == WorldSourceAvailability::Available)
                .filter_map(|e| match &e.payload {
                    Some(WorldSourcePayload::Schedule {
                        due_at_ms: Some(due),
                        ..
                    }) => Some((*due, e.source_id.as_str())),
                    _ => None,
                })
                .min();
            if earliest.map(|(_, id)| id) != Some(claim.source_ref.as_str()) {
                return Err("state-claim-not-next-deadline".into());
            }
        }
    }
    render(raw, frame)
}
fn display_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S %:z")
                .to_string()
        })
        .unwrap_or_else(|| "不明".into())
}
pub(crate) fn render(raw: &str, frame: &WorldFrame) -> Result<String, String> {
    if raw.len() > 16384 {
        return Err("state-claim-budget".into());
    }
    let answer: Answer = serde_json::from_str(raw).map_err(|_| "state-claim-schema")?;
    if answer.claims.is_empty() || answer.claims.len() > 8 {
        return Err("state-claim-unavailable".into());
    }
    let mut lines = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for claim in answer.claims {
        if !seen.insert((claim.kind, claim.source_ref.clone())) {
            return Err("state-claim-duplicate".into());
        }
        let source = frame
            .sources
            .iter()
            .filter(|g| g.availability == WorldSourceAvailability::Available)
            .flat_map(|g| &g.entries)
            .find(|e| e.kind == claim.kind && e.source_id == claim.source_ref)
            .ok_or("state-claim-source")?;
        if source.availability != WorldSourceAvailability::Available
            || !frame
                .scope
                .allowed_scope_keys
                .contains(&source.owner_scope_key)
            || source.digest != claim.source_version_or_digest
            || source.as_of_ms != claim.as_of_ms
            || source.payload.as_ref() != Some(&claim.value)
        {
            return Err("state-claim-mismatch".into());
        }
        let text=match &claim.value {
            WorldSourcePayload::Coding { job_id,owner_state,.. } => format!("タスク {job_id} の記録上の状態は {owner_state} です。"),
            WorldSourcePayload::Delegation { task_id,status,delegation_status,goal_status,.. } => format!("委任タスク {task_id} の状態は {status}、委任は {delegation_status}、目標は {goal_status} です。"),
            WorldSourcePayload::Schedule {entry_id,status,due_at_ms,..} => format!("期限 {entry_id} の予定時刻は {}、状態は {status} です。",due_at_ms.map(display_time).unwrap_or_else(||"不明".into())),
            WorldSourcePayload::Situation {scene,hold,..} => format!("観測から推定した現在の状況は {}、発話保留は {} です。",scene.as_deref().unwrap_or("不明"),hold.as_deref().unwrap_or("不明")),
        };
        lines.push(format!(
            "{text}\n確認時刻: {}\n根拠: {}@{}",
            display_time(source.as_of_ms),
            source.source_id,
            source.version.as_deref().unwrap_or(&source.digest)
        ));
    }
    Ok(lines.join("\n\n"))
}

/// Suppress unverified provider deltas, including the speech fan-out. Ordinary turns never use it.
pub(crate) struct ClaimEvents(pub Box<dyn crate::runtime::event_hub::RuntimeEventSender>);
impl crate::runtime::event_hub::RuntimeEventSender for ClaimEvents {
    fn send(&self, event: crate::ipc_contract::RuntimeEvent) -> tauri::Result<()> {
        if matches!(event, crate::ipc_contract::RuntimeEvent::Delta { .. }) {
            Ok(())
        } else {
            self.0.send(event)
        }
    }
    fn clone_box(&self) -> Box<dyn crate::runtime::event_hub::RuntimeEventSender> {
        Box::new(Self(self.0.clone_box()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> (WorldFrame, serde_json::Value) {
        let mut frame = WorldFrame::empty("run", "project:p", 100, 1000);
        let payload = WorldSourcePayload::Coding {
            job_id: "job".into(),
            owner_state: "settled".into(),
            phase: "terminal".into(),
            revision: Some(2),
        };
        frame.scope.allowed_scope_keys.push("task:job".into());
        frame.sources.push(WorldSourceGroup {
            kind: WorldSourceKind::Coding,
            availability: WorldSourceAvailability::Available,
            entries: vec![WorldSourceEntry {
                kind: WorldSourceKind::Coding,
                source_id: "job".into(),
                owner_scope_key: "task:job".into(),
                availability: WorldSourceAvailability::Available,
                observed_at_ms: 100,
                as_of_ms: 100,
                version: Some("2".into()),
                digest: "digest".into(),
                payload: Some(payload.clone()),
                reason_code: None,
            }],
            omission_reason: None,
        });
        let answer = json!({"claims":[{"kind":"coding","source_ref":"job","source_version_or_digest":"digest","value":payload,"as_of_ms":100}]});
        (frame, answer)
    }
    #[test]
    fn wr_t17_only_exact_authorized_source_can_render() {
        let (mut frame, answer) = fixture();
        let rendered = render(&answer.to_string(), &frame).unwrap();
        assert!(rendered.contains("settled"));
        assert!(!rendered.contains("成功"));
        assert!(!rendered.contains("対象:"));
        assert!(!rendered.contains("task:job"));
        for key in ["source_ref", "source_version_or_digest"] {
            let mut fake = answer.clone();
            fake["claims"][0][key] = json!("fake");
            assert!(render(&fake.to_string(), &frame).is_err());
        }
        let mut fake = answer.clone();
        fake["claims"][0]["value"]["value"]["owner_state"] = json!("succeeded");
        assert!(render(&fake.to_string(), &frame).is_err());
        frame.scope.allowed_scope_keys.clear();
        assert!(render(&answer.to_string(), &frame).is_err());
    }
    #[test]
    fn wr_t18_unverified_deltas_do_not_reach_ui() {
        use crate::runtime::event_hub::RuntimeEventSender;
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let received = count.clone();
        let sink = tauri::ipc::Channel::new(move |_| {
            received.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
        let guard = ClaimEvents(Box::new(sink));
        guard
            .send(crate::ipc_contract::RuntimeEvent::Delta {
                run_id: "run".into(),
                text: "forged success".into(),
            })
            .unwrap();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!guard.voice_response_enabled());
        assert!(!crate::runtime::context::state_answer::is_state_query(
            "今のタスクをキャンセルしてください"
        ));
        assert!(crate::runtime::context::state_answer::is_state_query(
            "現在のタスクは？"
        ));
    }
}
