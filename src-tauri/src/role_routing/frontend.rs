use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FrontendKind {
    Greeting,
    Thanks,
    Nod,
    Handoff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontendResult {
    pub kind: FrontendKind,
    pub reply: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Raw {
    kind: FrontendKind,
    reply: String,
}

pub(crate) const WAIT_LINE: &str = "少し考えます。";

pub(crate) const INSTRUCTION: &str =
    "返すのは JSON だけ。kind は greeting、thanks、nod、handoff のどれか。greeting・thanks・nod はターンを閉じる。reply は相手の発話に合わせた短い一言。それ以外は handoff。reply は「少し考えます。」。依頼・質問・作業には自分で答えない。";

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
        kind: raw.kind,
        reply: raw.reply,
    })
}

pub(crate) fn resolves_without_reasoner(result: &FrontendResult) -> bool {
    result.kind != FrontendKind::Handoff && spoken_line(result).is_some()
}

/// The receptionist's own sentence. Empty or multi-line text is not spoken.
pub(crate) fn spoken_line(result: &FrontendResult) -> Option<String> {
    let text = strip_leading_stage_tag(result.reply.trim());
    if text.is_empty() || text.contains('\n') || text.chars().count() > 80 {
        return None;
    }
    Some(text.to_string())
}

fn strip_leading_stage_tag(text: &str) -> &str {
    let Some(rest) = text.strip_prefix('[') else {
        return text;
    };
    let Some(end) = rest.find(']') else {
        return text;
    };
    let tag = &rest[..end];
    if tag.is_empty() || tag.chars().count() > 24 || tag.contains('\n') {
        return text;
    }
    rest[end + 1..].trim_start()
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
                    "kind": {
                        "type": "string",
                        "enum": ["greeting", "thanks", "nod", "handoff"]
                    },
                    "reply": {"type": "string"}
                },
                "required": ["kind", "reply"]
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_exact_schema() {
        assert!(INSTRUCTION.contains(WAIT_LINE));
        let parsed = parse(&format!(
            r#"{{"kind":"handoff","reply":{}}}"#,
            serde_json::to_string(WAIT_LINE).unwrap()
        ))
        .expect("schema");
        assert_eq!(parsed.kind, FrontendKind::Handoff);
        assert_eq!(parsed.reply, WAIT_LINE);
    }

    #[test]
    fn parse_rejects_extra_fields_and_free_text() {
        assert!(parse(r#"{"kind":"nod","reply":"x","extra":1}"#).is_err());
        assert!(parse("x").is_err());
        assert!(parse(r#"{"resolvesTurn":true,"reply":"x"}"#).is_err());
    }

    #[test]
    fn host_closes_only_closed_kinds_with_a_spoken_line() {
        let simple = parse(r#"{"kind":"greeting","reply":"x"}"#).expect("simple");
        assert_eq!(spoken_line(&simple).as_deref(), Some("x"));
        assert!(resolves_without_reasoner(&simple));
        let handoff = parse(&format!(
            r#"{{"kind":"handoff","reply":{}}}"#,
            serde_json::to_string(WAIT_LINE).unwrap()
        ))
        .expect("handoff");
        assert_eq!(spoken_line(&handoff).as_deref(), Some(WAIT_LINE));
        assert!(!resolves_without_reasoner(&handoff));
        let empty = parse(r#"{"kind":"thanks","reply":"  "}"#).expect("empty");
        assert_eq!(spoken_line(&empty), None);
        assert!(!resolves_without_reasoner(&empty));
        let tagged = parse(r#"{"kind":"greeting","reply":"[t]x"}"#).expect("tagged");
        assert_eq!(spoken_line(&tagged).as_deref(), Some("x"));
        let dollar = parse(r#"{"kind":"nod","reply":"[$t] x"}"#).expect("dollar");
        assert_eq!(spoken_line(&dollar).as_deref(), Some("x"));
        let wrapped =
            parse("y\n```json\n{\"kind\":\"greeting\",\"reply\":\"x\"}\n```").expect("wrapped");
        assert_eq!(spoken_line(&wrapped).as_deref(), Some("x"));
        let long = "x".repeat(81);
        let too_long = parse(&format!(r#"{{"kind":"nod","reply":"{long}"}}"#)).expect("long");
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
