//! Reserved small-model boundary. It cannot authorize a free-form response.
//! Deliberately not invoked until a LARM capability contract is available.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct Classification {
    route: Route,
    reply_key: Option<ReplyKey>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum Route {
    Delegate,
    SimpleReply,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum ReplyKey {
    Greeting,
    Acknowledgement,
}
#[allow(dead_code)]
impl Classification {
    pub(crate) fn allowed_reply(&self, input: &str) -> Option<&'static str> {
        if !matches!(self.route, Route::SimpleReply) {
            return None;
        }
        match (&self.reply_key, input.trim()) {
            (Some(ReplyKey::Greeting), "こんにちは" | "こんにちは。") => {
                Some("こんにちは。")
            }
            (Some(ReplyKey::Acknowledgement), "ありがとう" | "ありがとう。") => {
                Some("どういたしまして。")
            }
            _ => None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classifier_cannot_downgrade_a_mixed_request() {
        let value: Classification =
            serde_json::from_str(r#"{"route":"simple_reply","replyKey":"acknowledgement"}"#)
                .unwrap();
        assert_eq!(
            value.allowed_reply("ありがとう"),
            Some("どういたしまして。")
        );
        assert_eq!(value.allowed_reply("ありがとう、でも条件が違う"), None);
        assert!(serde_json::from_str::<Classification>(
            r#"{"route":"simple_reply","replyKey":"acknowledgement","text":"勝手な結論"}"#
        )
        .is_err());
    }
}
