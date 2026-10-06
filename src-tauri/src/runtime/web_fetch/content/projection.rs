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
    let withheld = guard_withholds_text(decision);
    let (text, relevant, selection_truncated) = if withheld {
        (String::new(), false, false)
    } else {
        super::super::static_content::project_visible_text(
            &document.text,
            query,
            model_max_characters,
        )
    };
    FetchContentResult {
        final_url: document.final_url.clone(),
        text,
        fetched_at: document.fetched_at.clone(),
        truncated: !withheld && (document.truncated || selection_truncated),
        decision,
        warning_categories,
        retrieval_status: if withheld {
            "blocked"
        } else if document.text.trim().is_empty() {
            "insufficient"
        } else if query.is_none() {
            "partial"
        } else if relevant {
            "relevant"
        } else {
            "partial"
        },
        retrieval_method: "webview",
    }
}

/// Page text the guard denied or held for approval never reaches the model.
pub(super) fn guard_withholds_text(decision: &str) -> bool {
    matches!(decision, "deny" | "require_approval")
}

/// The last gate before the model: (withheld, visible text, retrieval status).
fn model_view(result: &FetchContentResult) -> (bool, &str, &'static str) {
    match guard_withholds_text(result.decision) {
        true => (true, "", "blocked"),
        false => (false, &result.text, result.retrieval_status),
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
#[path = "projection/compact.rs"]
mod compact;
pub use compact::render_compact;
