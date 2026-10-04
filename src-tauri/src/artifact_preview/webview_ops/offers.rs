use super::*;

struct OfferStamp {
    conversation_id: String,
    generation: u64,
}

fn offer_stamps() -> &'static Mutex<HashMap<String, OfferStamp>> {
    static STAMPS: std::sync::OnceLock<Mutex<HashMap<String, OfferStamp>>> =
        std::sync::OnceLock::new();
    STAMPS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn stamp_offer(execution_ref: &str, conversation_id: &str) {
    let generation = active_hub().generation_of(conversation_id);
    if let Ok(mut stamps) = offer_stamps().lock() {
        stamps.insert(
            execution_ref.to_string(),
            OfferStamp {
                conversation_id: conversation_id.to_string(),
                generation,
            },
        );
    }
}

pub(crate) fn reject_stale_offer(
    execution_ref: &str,
    conversation_id: &str,
) -> Option<&'static str> {
    let Ok(stamps) = offer_stamps().lock() else {
        return Some("webview-unavailable");
    };
    let Some(stamp) = stamps.get(execution_ref) else {
        return Some("webview-not-operable");
    };
    if stamp.conversation_id != conversation_id {
        return Some("webview-conversation-changed");
    }
    let generation = stamp.generation;
    drop(stamps);
    if generation != active_hub().generation_of(conversation_id) {
        return Some("webview-generation-changed");
    }
    None
}

pub(crate) fn is_offered_for(conversation_id: &str) -> bool {
    active_hub().is_offered(conversation_id)
}

type RankedCandidates = (Vec<EligibleRevision>, Vec<String>, Vec<(String, Vec<f32>)>);

/// Drops `artifact_webview` before ranking when that conversation has no operable website tabs.
pub(crate) fn restrict_candidates(
    conversation_id: &str,
    mut eligible: Vec<EligibleRevision>,
    mut lexical: Vec<String>,
    mut embeddings: Vec<(String, Vec<f32>)>,
) -> RankedCandidates {
    if is_offered_for(conversation_id) {
        return (eligible, lexical, embeddings);
    }
    let blocked: HashSet<String> = eligible
        .iter()
        .filter(|item| item.revision.tool_id == "artifact_webview")
        .map(|item| item.revision.id.clone())
        .collect();
    eligible.retain(|item| item.revision.tool_id != "artifact_webview");
    lexical.retain(|id| !blocked.contains(id));
    embeddings.retain(|(id, _)| !blocked.contains(id));
    (eligible, lexical, embeddings)
}
