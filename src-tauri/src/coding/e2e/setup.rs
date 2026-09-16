use serde_json::json;
pub(super) fn configure_local_harness(state: &crate::AppState) {
    if std::env::var("SAAA_BBS_USE_HARNESS").as_deref() != Ok("1") {
        return;
    }
    state
        .sqlite_writer
        .write(|c| {
            let mut documents: Vec<crate::SaveSettingsDocumentInput> =
                crate::persistence::list_settings_documents(c)?
                    .into_iter()
                    .map(|d| crate::SaveSettingsDocumentInput {
                        namespace: d.namespace,
                        key: d.key,
                        schema_version: d.schema_version,
                        value_json: d.value_json,
                    })
                    .collect();
            let document = documents
                .iter_mut()
                .find(|d| d.namespace == "routing.tasks" && d.key == "default")
                .ok_or("routing settings missing")?;
            document.value_json["conversationRespond"]["source"] = json!("harness");
            document.value_json["conversationRespond"]["timeoutMs"] = json!(240000);
            document.value_json["conversationRespond"]["primaryProviderId"] =
                serde_json::Value::Null;
            document.value_json["conversationRespond"]["fallbackProviderIds"] = json!([]);
            documents
                .iter_mut()
                .find(|d| d.namespace == "providers.model")
                .ok_or("model settings missing")?
                .value_json["reasoningEffort"] = json!("low");
            crate::persistence::save_settings_documents_to_connection(c, &documents)?;
            Ok(())
        })
        .unwrap();
    println!("Saved local LLM Harness conversation route (persistent)");
}

pub(super) fn seed(state: &crate::AppState) {
    let Ok(source) = std::env::var("SAAA_BBS_SETTINGS_SOURCE") else {
        return;
    };
    let source =
        crate::persistence::sqlite::SqliteReaders::open(std::path::Path::new(&source)).unwrap();
    let (documents, coding) = source
        .read(|c| {
            let documents = crate::persistence::list_settings_documents(c)?
                .into_iter()
                .map(|d| crate::SaveSettingsDocumentInput {
                    namespace: d.namespace,
                    key: d.key,
                    schema_version: d.schema_version,
                    value_json: d.value_json,
                })
                .collect::<Vec<_>>();
            let coding: String = c
                .query_row("SELECT value_json FROM coding_settings LIMIT 1", [], |r| {
                    r.get(0)
                })
                .map_err(crate::database_error)?;
            Ok((documents, coding))
        })
        .unwrap();
    state
        .sqlite_writer
        .write(move |c| {
            assert_eq!(
                c.query_row("SELECT count(*) FROM conversation_messages", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0,
                "seed only a fresh E2E database"
            );
            crate::persistence::save_settings_documents_to_connection(c, &documents)?;
            c.execute("UPDATE coding_settings SET value_json=?1", [coding])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
}
