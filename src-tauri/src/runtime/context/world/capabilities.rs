//! Provider contracts and metadata-only delivery diagnostics. No prompts or source quotations.
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use ts_rs::TS;
#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorldCapabilities {
    pub state_input: bool,
    pub graph: bool,
    pub fresh_tool_continuation: bool,
    pub verified_state_answer: bool,
    pub answer_mode: String,
}
#[derive(Clone, Serialize, TS)]
pub(crate) struct ScopeChoice {
    pub key: String,
    pub label: String,
    pub refs: Vec<ScopeRef>,
}
#[derive(Clone, Serialize, TS)]
pub(crate) struct ScopeRef {
    pub kind: String,
    pub id: String,
    pub relation: String,
}
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorldContextStatus {
    pub choices: Vec<ScopeChoice>,
    pub message_scopes: std::collections::BTreeMap<String, Vec<String>>,
    pub latest_scope_keys: Vec<String>,
    pub latest_provider: Option<String>,
    pub delivery: Option<String>,
    pub omission_reason: Option<String>,
}
#[tauri::command]
pub(crate) fn world_provider_capabilities(
    state: tauri::State<'_, crate::AppState>,
    provider_id: String,
) -> Result<WorldCapabilities, String> {
    state.sqlite_readers.read(|c| {
        let settings = crate::persistence::load_model_providers(c)?;
        let p = settings
            .providers
            .iter()
            .find(|p| p.id() == provider_id)
            .ok_or("World provider is not configured")?;
        Ok(for_kind(p.kind()))
    })
}
pub(crate) fn for_kind(kind: &str) -> WorldCapabilities {
    let supported = matches!(kind, "openai-compatible" | "agent-session" | "dynamic-lan");
    WorldCapabilities {
        state_input: supported,
        graph: supported,
        fresh_tool_continuation: supported,
        verified_state_answer: supported,
        answer_mode: if supported {
            "validated-claims"
        } else {
            "host-card"
        }
        .into(),
    }
}
#[tauri::command]
pub(crate) fn world_context_status(
    state: tauri::State<'_, crate::AppState>,
    conversation_id: String,
) -> Result<WorldContextStatus, String> {
    crate::validate_identifier(&conversation_id, "conversation id")?;
    state.sqlite_readers.read(|c| status(c, &conversation_id))
}
pub(crate) fn status(c: &Connection, conversation: &str) -> Result<WorldContextStatus, String> {
    let principal: String = c
        .query_row(
            "SELECT principal FROM personal_scope WHERE id='primary'",
            [],
            |r| r.get(0),
        )
        .map_err(crate::database_error)?;
    let mut choices = vec![ScopeChoice {
        key: format!("user:{principal}"),
        label: "個人（Projectなし）".into(),
        refs: vec![ScopeRef {
            kind: "user".into(),
            id: principal,
            relation: "focus".into(),
        }],
    }];
    let mut q=c.prepare("SELECT scope_key,opaque_id FROM context_scopes WHERE kind='project' AND state='active' ORDER BY scope_key LIMIT 64").map_err(crate::database_error)?;
    let rows = q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(crate::database_error)?;
    for row in rows {
        let (key, id) = row.map_err(crate::database_error)?;
        let mut refs = vec![ScopeRef {
            kind: "project".into(),
            id: id.clone(),
            relation: "focus".into(),
        }];
        let mut linked=c.prepare("SELECT s.kind,s.opaque_id FROM context_scope_links l JOIN context_scopes s ON s.scope_key=l.child_scope_key WHERE l.parent_scope_key=?1 AND s.state='active' AND s.kind='resource' ORDER BY s.scope_key LIMIT 8").map_err(crate::database_error)?;
        for row in linked
            .query_map([&key], |r| {
                Ok(ScopeRef {
                    kind: r.get(0)?,
                    id: r.get(1)?,
                    relation: "parent".into(),
                })
            })
            .map_err(crate::database_error)?
        {
            refs.push(row.map_err(crate::database_error)?);
        }
        choices.push(ScopeChoice {
            key: key.clone(),
            label: format!("Project {id}"),
            refs: refs.clone(),
        });
        let mut tasks=c.prepare("SELECT s.scope_key,s.opaque_id FROM context_scope_links l JOIN context_scopes s ON s.scope_key=l.child_scope_key WHERE l.parent_scope_key=?1 AND s.state='active' AND s.kind='task' ORDER BY s.scope_key LIMIT 32").map_err(crate::database_error)?;
        for row in tasks
            .query_map([&key], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(crate::database_error)?
        {
            let (task_key, task) = row.map_err(crate::database_error)?;
            let mut task_refs = refs.clone();
            task_refs.push(ScopeRef {
                kind: "task".into(),
                id: task.clone(),
                relation: "current".into(),
            });
            choices.push(ScopeChoice {
                key: task_key,
                label: format!("Project {id} / Task {task}"),
                refs: task_refs,
            });
        }
    }
    let latest:Option<(String,String,String)>=c.query_row("SELECT g.id,g.run_id,g.provider_id FROM context_generations g JOIN runtime_runs r ON r.id=g.run_id WHERE r.conversation_id=?1 ORDER BY g.started_at DESC,g.ordinal DESC LIMIT 1",[conversation],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(crate::database_error)?;
    let mut result = WorldContextStatus {
        choices,
        message_scopes: message_scopes(c, conversation)?,
        latest_scope_keys: vec![],
        latest_provider: None,
        delivery: None,
        omission_reason: None,
    };
    if let Some((generation, run, provider)) = latest {
        result.latest_provider = Some(provider);
        let mut q=c.prepare("SELECT scope_key FROM runtime_run_scopes WHERE run_id=?1 AND relation='focus' ORDER BY scope_key").map_err(crate::database_error)?;
        result.latest_scope_keys = q
            .query_map([run], |r| r.get(0))
            .map_err(crate::database_error)?
            .collect::<Result<_, _>>()
            .map_err(crate::database_error)?;
        let receipt:Option<(bool,Option<String>)>=c.query_row("SELECT selected,omission_reason FROM context_generation_inputs WHERE generation_id=?1 AND source_kind IN ('world-model','world-source-snapshot') LIMIT 1",[generation],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(crate::database_error)?;
        match receipt {
            Some((true, _)) => result.delivery = Some("sent".into()),
            Some((false, reason)) => {
                result.delivery = Some("omitted".into());
                result.omission_reason = reason
            }
            None => {
                result.delivery = Some("not-recorded".into());
                result.omission_reason = Some("frame-unavailable-or-not-requested".into())
            }
        }
    }
    Ok(result)
}

fn message_scopes(
    c: &Connection,
    conversation: &str,
) -> Result<std::collections::BTreeMap<String, Vec<String>>, String> {
    let mut q = c.prepare("SELECT s.message_id,s.scope_key FROM conversation_message_scopes s JOIN (SELECT id FROM conversation_messages WHERE conversation_id=?1 AND role='assistant' ORDER BY CAST(created_at AS INTEGER) DESC,id DESC LIMIT 200) m ON m.id=s.message_id WHERE s.relation IN ('focus','current') ORDER BY s.message_id,s.scope_key").map_err(crate::database_error)?;
    let mut result = std::collections::BTreeMap::<String, Vec<String>>::new();
    for row in q
        .query_map([conversation], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(crate::database_error)?
    {
        let (id, key) = row.map_err(crate::database_error)?;
        result.entry(id).or_default().push(key);
    }
    Ok(result)
}
pub(crate) fn typescript_bindings() -> String {
    [
        WorldCapabilities::decl(&Default::default()),
        ScopeRef::decl(&Default::default()),
        ScopeChoice::decl(&Default::default()),
        WorldContextStatus::decl(&Default::default()),
    ]
    .map(|decl| format!("export {decl}"))
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wr_t21_capability_contract_does_not_infer_audio_as_world_reasoning() {
        assert!(for_kind("openai-compatible").fresh_tool_continuation);
        assert!(!for_kind("cloud-tts").state_input);
        assert_eq!(for_kind("unknown").answer_mode, "host-card");
    }
    #[test]
    fn wr_t20_status_lists_registered_ids_without_source_text() {
        let f = crate::memory::personal_state::world::runtime_test_support::Fixture::new(&[]);
        let result = f
            .writer
            .read_serialized(|c| status(c, crate::PRIMARY_CONVERSATION_ID))
            .unwrap();
        assert!(result.choices.iter().any(|c| c.key == f.project));
        assert!(result.choices[0].key.starts_with("user:"));
        let encoded = serde_json::to_string(&result).unwrap();
        assert!(!encoded.contains("prompt"));
        assert!(!encoded.contains("source_text"));
    }
}
