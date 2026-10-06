use super::*;

pub fn render_compact(result: &FetchContentResult) -> String {
    // Japanese page text is budgeted at approximately one character per token.
    // Read the entire HTML first, then cap only the extracted model-facing text.
    const MAX_MODEL_TEXT_CHARS: usize = 3_000;
    let (withheld, visible_text, retrieval_status) = model_view(result);
    let text_end = visible_text
        .char_indices()
        .map(|(index, char)| index + char.len_utf8())
        .take(MAX_MODEL_TEXT_CHARS)
        .last()
        .unwrap_or(0);
    let text = &visible_text[..text_end];
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
            "text": text,
            "fetchedAt": result.fetched_at,
            "truncated": !withheld && (result.truncated || text_end < visible_text.len()),
            "retrievalStatus": retrieval_status,
            "retrievalMethod": result.retrieval_method,
        }
    })
    .to_string()
}
