use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ack {
    None,
    Nod,
    Greeting,
    Thanks,
    Working,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Intent {
    Social,
    Acknowledgement,
    Request,
    Unclear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrontendResult {
    pub ack: Ack,
    pub intent: Intent,
    pub resolves_turn: bool,
    pub high_confidence: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Raw {
    ack: String,
    intent: String,
    resolves_turn: bool,
    confidence: String,
}

pub(crate) const INSTRUCTION: &str = "あなたは音声受付。返すのは JSON だけ。文章は書かない。\
相槌・同意なら ack は nod、intent は acknowledgement。\
挨拶だけなら ack は greeting、intent は social。\
お礼だけなら ack は thanks、intent は social。\
依頼・質問なら ack は working、intent は request。\
判断できないなら ack は none、intent は unclear。\
resolvesTurn は、挨拶・お礼・相槌だけで返事が完結するときだけ true。\
自信がなければ confidence は low。";

pub(crate) fn parse(raw: &str) -> Result<FrontendResult, &'static str> {
    let raw: Raw = serde_json::from_str(raw.trim()).map_err(|_| "frontend_invalid")?;
    let ack = match raw.ack.as_str() {
        "none" => Ack::None,
        "nod" => Ack::Nod,
        "greeting" => Ack::Greeting,
        "thanks" => Ack::Thanks,
        "working" => Ack::Working,
        _ => return Err("frontend_invalid"),
    };
    let intent = match raw.intent.as_str() {
        "social" => Intent::Social,
        "acknowledgement" => Intent::Acknowledgement,
        "request" => Intent::Request,
        "unclear" => Intent::Unclear,
        _ => return Err("frontend_invalid"),
    };
    let high_confidence = match raw.confidence.as_str() {
        "high" => true,
        "low" => false,
        _ => return Err("frontend_invalid"),
    };
    Ok(FrontendResult {
        ack,
        intent,
        resolves_turn: raw.resolves_turn,
        high_confidence,
    })
}

pub(crate) fn resolves_without_reasoner(result: &FrontendResult, already_greeted: bool) -> bool {
    result.resolves_turn
        && result.high_confidence
        && matches!(result.intent, Intent::Social | Intent::Acknowledgement)
        && matches!(result.ack, Ack::Nod | Ack::Greeting | Ack::Thanks)
        && ack_text(result.ack, already_greeted).is_some()
}

pub(crate) fn ack_text(ack: Ack, already_greeted: bool) -> Option<&'static str> {
    match ack {
        Ack::None => None,
        Ack::Nod => Some("はい。"),
        Ack::Greeting if already_greeted => None,
        Ack::Greeting => Some("こんにちは。"),
        Ack::Thanks => Some("どういたしまして。"),
        Ack::Working => Some("確認します。"),
    }
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
                    "ack": {"type": "string", "enum": ["none", "nod", "greeting", "thanks", "working"]},
                    "intent": {"type": "string", "enum": ["social", "acknowledgement", "request", "unclear"]},
                    "resolvesTurn": {"type": "boolean"},
                    "confidence": {"type": "string", "enum": ["high", "low"]}
                },
                "required": ["ack", "intent", "resolvesTurn", "confidence"]
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_exact_schema() {
        let parsed = parse(
            r#"{"ack":"working","intent":"request","resolvesTurn":false,"confidence":"high"}"#,
        )
        .expect("schema");
        assert_eq!(parsed.ack, Ack::Working);
        assert_eq!(parsed.intent, Intent::Request);
        assert!(!parsed.resolves_turn);
        assert!(parsed.high_confidence);
    }

    #[test]
    fn parse_rejects_extra_fields_and_free_text() {
        assert!(parse(
            r#"{"ack":"nod","intent":"acknowledgement","resolvesTurn":true,"confidence":"high","extra":1}"#
        )
        .is_err());
        assert!(parse("はい、承知しました").is_err());
    }

    #[test]
    fn ack_text_is_host_owned() {
        assert_eq!(ack_text(Ack::Nod, false), Some("はい。"));
        assert_eq!(ack_text(Ack::Greeting, false), Some("こんにちは。"));
        assert_eq!(ack_text(Ack::Greeting, true), None);
        assert_eq!(ack_text(Ack::Thanks, false), Some("どういたしまして。"));
        assert_eq!(ack_text(Ack::Working, false), Some("確認します。"));
        assert_eq!(ack_text(Ack::None, false), None);
    }

    #[test]
    #[test]
    fn social_ack_resolves_only_when_the_phrase_exists() {
        let nod = parse(
            r#"{"ack":"nod","intent":"acknowledgement","resolvesTurn":true,"confidence":"high"}"#,
        )
        .expect("nod");
        assert!(resolves_without_reasoner(&nod, false));
        let request = parse(
            r#"{"ack":"working","intent":"request","resolvesTurn":true,"confidence":"high"}"#,
        )
        .expect("request");
        assert!(!resolves_without_reasoner(&request, false));
        let eager = parse(
            r#"{"ack":"working","intent":"social","resolvesTurn":true,"confidence":"high"}"#,
        )
        .expect("eager");
        assert!(!resolves_without_reasoner(&eager, false));
        let low = parse(
            r#"{"ack":"thanks","intent":"social","resolvesTurn":true,"confidence":"low"}"#,
        )
        .expect("low");
        assert!(!resolves_without_reasoner(&low, false));
        let greeted = parse(
            r#"{"ack":"greeting","intent":"social","resolvesTurn":true,"confidence":"high"}"#,
        )
        .expect("greeting");
        assert!(!resolves_without_reasoner(&greeted, true));
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
