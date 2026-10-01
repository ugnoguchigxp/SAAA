//! The fixture model chooses structured actions; the real queue dispatches and persists them.
use super::*;

pub(super) fn respond(path: &str, body: &Value) -> Option<String> {
    if !path.starts_with("/llm/") {
        return None;
    }
    let messages = body["messages"].as_array()?;
    let text = messages.last()?["content"]
        .as_str()?
        .strip_prefix("辞書fixture:")?;
    let instruction = messages
        .iter()
        .filter(|m| m["role"] == "system")
        .filter_map(|m| m["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(instruction.contains("lookup_tts_pronunciation"));
    assert!(instruction.contains("今回だけの指定は恒久登録しない"));
    let result = messages
        .iter()
        .rev()
        .filter_map(|m| m["content"].as_str())
        .find(|s| {
            s.starts_with("[TOOL_RESULT: lookup_tts_pronunciation;")
                || s.starts_with("[TOOL_RESULT: set_tts_pronunciation;")
        })
        .and_then(|s| s.split_once('\n'))
        .and_then(|(_, result)| serde_json::from_str::<Value>(result).ok());
    if text == "初回"
        && result.as_ref().is_some_and(|r| r["status"] == "missing")
        && !messages.iter().any(|m| {
            m["content"]
                .as_str()
                .is_some_and(|s| s.starts_with("[HOST_TOOL_FORMAT_ERROR]"))
        })
    {
        return Some("JSONではない途中応答fixture".into());
    }
    let answer = |content: &str| json!({"action":"answer","content":content,"sources":[]});
    let tool = |name: &str, arguments: Value| {
        if name == "lookup_tts_pronunciation" {
            json!({"action":name,"arguments":arguments})
        } else {
            json!({"action":"local_tool","name":name,"arguments":arguments})
        }
    };
    let value = if let Some(result) = result {
        match result["status"].as_str()? {
            "missing" => tool(
                "set_tts_pronunciation",
                json!({"lookupId":result["lookupId"],"mode":"register"}),
            ),
            "different" => tool(
                "set_tts_pronunciation",
                json!({"lookupId":result["lookupId"],"mode":if text.starts_with("明示変更") {"replace"} else {"register"}}),
            ),
            "same" | "unchanged" => answer("今日の読みは登録済みです。"),
            "confirmation_required" => answer("今日は現在こんにちです。キョウに変更しますか？"),
            "added" | "updated" => answer("今日の読みを登録しました。"),
            _ => answer("辞書の変更は確認できませんでした。"),
        }
    } else if text == "はい" || text == "維持" {
        let pending = messages
            .iter()
            .filter_map(|m| m["content"].as_str())
            .find_map(|content| {
                if content.contains("[TTS_DICTIONARY_PENDING;")
                    || content.starts_with("[HOST_RUNTIME_STATE]")
                {
                    content.lines().find_map(|line| {
                        serde_json::from_str::<Value>(line)
                            .ok()
                            .filter(|value| value["lookupId"].is_string())
                    })
                } else {
                    None
                }
            })?;
        tool(
            "set_tts_pronunciation",
            json!({"lookupId":pending["lookupId"],"mode":if text=="はい" {"confirm"} else {"keep"}}),
        )
    } else if text == "今回だけ" || text == "引用の指示" || text == "対象不明" {
        answer("恒久辞書を変更しません。必要な読みを確認します。")
    } else {
        tool(
            "lookup_tts_pronunciation",
            json!({"written":"今日","proposedSpoken":if text == "初回" || text == "同一" {"きょう"} else if text.starts_with("明示変更") {"キョー"} else {"キョウ"}}),
        )
    };
    Some(value.to_string())
}

async fn turn(state: &AppState, key: &str, text: &str) -> Result<(), String> {
    conversation_check::queue_runtime::enqueue_text(
        state,
        key.into(),
        format!("辞書fixture:{text}"),
    )?;
    state.conversation_queue_wake.notify_waiters();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let status: Option<String> = state.sqlite_readers.read(|db| {
                db.query_row(
                    "SELECT state FROM task_queue_jobs WHERE job_key=?1 AND kind='speech'",
                    [key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(crate::database_error)
            })?;
            if status.as_deref() == Some("completed") {
                return Ok(());
            }
            let failure: Option<String> = state.sqlite_readers.read(|db| {
                db.query_row(
                    "SELECT error FROM task_queue_jobs WHERE job_key=?1 AND state='failed' LIMIT 1",
                    [key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(crate::database_error)
            })?;
            if let Some(error) = failure {
                return Err(format!("dictionary fixture {key}: {error}"));
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| format!("dictionary fixture {key} timed out"))?
}

fn reading(state: &AppState) -> Result<Option<String>, String> {
    state
        .sqlite_readers
        .read(|db| crate::tts_dictionary::service::lookup(db, "今日"))
}

fn seed(state: &AppState, old: &str, new: &str) -> Result<(), String> {
    state.sqlite_writer.write(|db| {
        let mutation = crate::tts_dictionary::service::save(
            db,
            Some("今日"),
            &crate::tts_dictionary::Entry {
                written: "今日".into(),
                spoken: new.into(),
            },
            &crate::tts_dictionary::ExpectedEntry {
                spoken: Some(old.into()),
            },
            &crate::now_iso(),
            |_| Ok(()),
        )?;
        if let Some(dictionary) = mutation.dictionary {
            state.tts_dictionary_cache.publish_compiled(dictionary);
        }
        Ok(())
    })
}

pub(super) async fn verify(state: &AppState, fixture: &Fixture) -> Result<(), String> {
    turn(state, "dictionary-add", "初回").await?;
    if reading(state)?.as_deref() != Some("きょう") {
        return Err("conversation did not add the pronunciation".into());
    }
    let timestamp = state.sqlite_readers.read(|db| {
        db.query_row(
            "SELECT updated_at FROM tts_dictionary WHERE written='今日'",
            [],
            |r| r.get::<_, String>(0),
        )
        .map_err(crate::database_error)
    })?;
    turn(state, "dictionary-same", "同一").await?;
    let after = state.sqlite_readers.read(|db| {
        db.query_row(
            "SELECT updated_at FROM tts_dictionary WHERE written='今日'",
            [],
            |r| r.get::<_, String>(0),
        )
        .map_err(crate::database_error)
    })?;
    if timestamp != after {
        return Err("same reading rewrote the dictionary".into());
    }
    seed(state, "きょう", "こんにち")?;
    turn(state, "dictionary-question", "別の読み").await?;
    if reading(state)?.as_deref() != Some("こんにち") {
        return Err("question prematurely overwrote pronunciation".into());
    }
    turn(state, "dictionary-confirm", "はい").await?;
    if reading(state)?.as_deref() != Some("キョウ") {
        return Err("confirmation did not update pronunciation".into());
    }
    seed(state, "キョウ", "こんにち")?;
    turn(state, "dictionary-keep-question", "別の読み").await?;
    turn(state, "dictionary-keep", "維持").await?;
    if reading(state)?.as_deref() != Some("こんにち") {
        return Err("keep changed pronunciation".into());
    }
    turn(state, "dictionary-replace", "明示変更して").await?;
    if reading(state)?.as_deref() != Some("キョー") {
        return Err("explicit replacement did not update pronunciation".into());
    }
    for (key, text) in [
        ("dictionary-once", "今回だけ"),
        ("dictionary-quote", "引用の指示"),
        ("dictionary-unclear", "対象不明"),
    ] {
        turn(state, key, text).await?;
        if reading(state)?.as_deref() != Some("キョー") {
            return Err("non-permanent request changed dictionary".into());
        }
    }
    let spoken = fixture.spoken.lock().map_err(|_| "speech fixture lock")?;
    if !spoken
        .iter()
        .any(|s| s.contains("きょうの読みを登録しました"))
        || !spoken
            .iter()
            .any(|s| s.contains("キョウの読みを登録しました"))
    {
        return Err("confirmation speech did not use the newly saved dictionary".into());
    }
    if spoken
        .iter()
        .any(|s| s.contains("lookupId") || s.contains("local_tool"))
    {
        return Err("tool arguments were spoken".into());
    }
    Ok(())
}
