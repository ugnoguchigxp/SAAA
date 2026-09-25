use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontendResult {
    pub resolves_turn: bool,
    pub reply: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Raw {
    resolves_turn: bool,
    reply: String,
}

pub(crate) const INSTRUCTION: &str = "あなたは音声受付。返すのは JSON だけ。\
簡単に自分で返せるときは resolvesTurn を true にし、reply に相手の言い方に合わせた返事を一文だけ書く。\
調べたり考えたりする必要があるときは resolvesTurn を false にし、reply は「少し考えます。」にする。その続きは思考担当が答える。\
自信がないときも resolvesTurn は false、reply は「少し考えます。」。";

pub(crate) fn parse(raw: &str) -> Result<FrontendResult, &'static str> {
    let raw = raw.trim();
    if let Some(result) = parse_object(raw) {
        return Ok(result);
    }
    let Some(start) = raw.find('{') else {
        return Err("frontend_invalid");
    };
    let Some(end) = raw.rfind('}') else {
        return Err("frontend_invalid");
    };
    if end < start {
        return Err("frontend_invalid");
    }
    parse_object(&raw[start..=end]).ok_or("frontend_invalid")
}

fn parse_object(raw: &str) -> Option<FrontendResult> {
    let raw: Raw = serde_json::from_str(raw).ok()?;
    Some(FrontendResult {
        resolves_turn: raw.resolves_turn,
        reply: raw.reply,
    })
}

pub(crate) fn resolves_without_reasoner(result: &FrontendResult) -> bool {
    result.resolves_turn && spoken_line(result).is_some()
}

/// The receptionist's own sentence. Empty or multi-line text is not spoken.
pub(crate) fn spoken_line(result: &FrontendResult) -> Option<String> {
    let text = result.reply.trim();
    if text.is_empty() || text.contains('\n') || text.chars().count() > 80 {
        return None;
    }
    Some(text.to_string())
}

#[cfg(test)]
pub(crate) fn record_filler_tick(tick: u32) {
    filler_ticks().lock().unwrap().push(tick);
}

#[cfg(test)]
pub(crate) fn take_filler_ticks() -> Vec<u32> {
    std::mem::take(&mut *filler_ticks().lock().unwrap())
}

#[cfg(test)]
fn filler_ticks() -> &'static std::sync::Mutex<Vec<u32>> {
    static TICKS: std::sync::OnceLock<std::sync::Mutex<Vec<u32>>> = std::sync::OnceLock::new();
    TICKS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// `(speak, defer_to_next_tick)`. Tick 20 is the caller's timeout and never speaks.
pub(crate) fn filler_decision(tick: u32, playing: bool, deferred: bool) -> (bool, bool) {
    if tick == 0 || tick >= 20 {
        return (false, false);
    }
    if deferred {
        return (!playing, false);
    }
    if tick % 5 != 0 {
        return (false, false);
    }
    if playing {
        return (false, true);
    }
    (true, false)
}

pub(crate) fn response_format() -> serde_json::Value {
    serde_json::json!({
        "type": "json_schema",
        "json_schema": {
            "name": "frontend_ack",
            "strict": true,
            "schema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "resolvesTurn": {"type": "boolean"},
                    "reply": {"type": "string"}
                },
                "required": ["resolvesTurn", "reply"]
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_exact_schema() {
        let parsed = parse(r#"{"resolvesTurn":false,"reply":"少し考えます。"}"#).expect("schema");
        assert!(!parsed.resolves_turn);
        assert_eq!(parsed.reply, "少し考えます。");
    }

    #[test]
    fn parse_rejects_extra_fields_and_free_text() {
        assert!(parse(r#"{"resolvesTurn":true,"reply":"はい。","extra":1}"#).is_err());
        assert!(parse("はい、承知しました").is_err());
    }

    #[test]
    fn simple_reply_finishes_and_handoff_keeps_the_model_sentence() {
        let simple =
            parse(r#"{"resolvesTurn":true,"reply":"おはようございます。"}"#).expect("simple");
        assert_eq!(
            spoken_line(&simple).as_deref(),
            Some("おはようございます。")
        );
        assert!(resolves_without_reasoner(&simple));
        let handoff = parse(r#"{"resolvesTurn":false,"reply":"少し考えます。"}"#).expect("handoff");
        assert_eq!(spoken_line(&handoff).as_deref(), Some("少し考えます。"));
        assert!(!resolves_without_reasoner(&handoff));
        let empty = parse(r#"{"resolvesTurn":true,"reply":"  "}"#).expect("empty");
        assert_eq!(spoken_line(&empty), None);
        assert!(!resolves_without_reasoner(&empty));
        let wrapped = parse(
            "考えます。\n```json\n{\"resolvesTurn\":true,\"reply\":\"おはようございます。\"}\n```",
        )
        .expect("wrapped");
        assert_eq!(
            spoken_line(&wrapped).as_deref(),
            Some("おはようございます。")
        );
        let long = "あ".repeat(81);
        let too_long =
            parse(&format!(r#"{{"resolvesTurn":true,"reply":"{long}"}}"#)).expect("long");
        assert_eq!(spoken_line(&too_long), None);
        assert!(!resolves_without_reasoner(&too_long));
    }

    #[test]
    fn filler_speaks_on_each_fifth_tick_and_defers_once() {
        assert_eq!(filler_decision(4, false, false), (false, false));
        assert_eq!(filler_decision(5, false, false), (true, false));
        assert_eq!(filler_decision(5, true, false), (false, true));
        assert_eq!(filler_decision(6, false, true), (true, false));
        assert_eq!(filler_decision(6, true, true), (false, false));
        assert_eq!(filler_decision(10, false, false), (true, false));
        assert_eq!(filler_decision(15, false, false), (true, false));
        assert_eq!(filler_decision(20, false, false), (false, false));
    }
}
