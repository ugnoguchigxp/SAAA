use super::*;

pub fn render_compact(result: &FetchContentResult) -> String {
    // Japanese page text is budgeted at approximately one character per token.
    // Read the entire HTML first, then cap only the extracted model-facing text.
    const MAX_MODEL_TEXT_CHARS: usize = 3_000;
    let text_end = result
        .text
        .char_indices()
        .map(|(index, char)| index + char.len_utf8())
        .take(MAX_MODEL_TEXT_CHARS)
        .last()
        .unwrap_or(0);
    let text = &result.text[..text_end];
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
            "truncated": result.truncated || text_end < result.text.len(),
            "retrievalStatus": result.retrieval_status,
            "retrievalMethod": result.retrieval_method,
        }
    })
    .to_string()
}
