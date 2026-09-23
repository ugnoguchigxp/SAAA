//! Project plugin documents to the compact model-facing result.
use super::FetchContentResult;

pub(super) fn project_document(
    document: &tauri_plugin_llm_fetch::RetrievedDocument,
    model_max_characters: usize,
    query: Option<&str>,
) -> FetchContentResult {
    let decision = guard_decision_label(document.security.decision.clone());
    let mut warning_categories: Vec<String> = document
        .security
        .findings
        .iter()
        .filter(|finding| {
            !matches!(
                finding.category,
                tauri_plugin_llm_fetch::SecurityFindingCategory::BenignMention
            )
        })
        .map(|finding| {
            serde_json::to_value(&finding.category)
                .and_then(serde_json::from_value::<String>)
                .unwrap_or_else(|_| "unknown".to_string())
        })
        .collect();
    warning_categories.sort();
    warning_categories.dedup();
    let (text, relevant, selection_truncated) = super::super::static_content::project_visible_text(
        &document.text,
        query,
        model_max_characters,
    );
    FetchContentResult {
        final_url: document.final_url.clone(),
        text,
        fetched_at: document.fetched_at.clone(),
        truncated: document.truncated || selection_truncated,
        decision,
        warning_categories,
        retrieval_status: if document.text.trim().is_empty() {
            "insufficient"
        } else if query.is_none() {
            "partial"
        } else if relevant {
            "relevant"
        } else {
            "insufficient"
        },
        retrieval_method: "webview",
    }
}

pub(super) fn guard_decision_label(
    decision: tauri_plugin_llm_fetch::GuardDecision,
) -> &'static str {
    use tauri_plugin_llm_fetch::GuardDecision as Decision;
    match decision {
        Decision::Allow => "allow",
        Decision::AllowWithWarning => "allow_with_warning",
        Decision::RequireApproval => "require_approval",
        Decision::Deny => "deny",
    }
}

/// Trim the extracted text down to the model's `maxCharacters` ask without
/// splitting UTF-8. The plugin floor is 1_000 chars; smaller asks are served
/// from the same extraction and marked truncated.
pub(super) fn truncate_to_model_max(text: &str, model_max: usize) -> String {
    if text.chars().count() <= model_max {
        return text.to_string();
    }
    text.chars().take(model_max).collect()
}

/// Render the compact `fetch_content_result` JSON. Plugin-internal fields
/// (session ID, worker label, proxy URL, stages, raw exceptions) are never
/// included.
pub fn render_compact(result: &FetchContentResult) -> String {
    serde_json::json!({
        "type": "fetch_content_result",
        "security": {
            "trust": "untrusted",
            "tainted": true,
            "decision": result.decision,
            "warningCategories": result.warning_categories,
        },
        "document": {
            "url": result.final_url,
            "text": result.text,
            "fetchedAt": result.fetched_at,
            "truncated": result.truncated,
            "retrievalStatus": result.retrieval_status,
            "retrievalMethod": result.retrieval_method,
        }
    })
    .to_string()
}
