pub(crate) fn render(now_ms: i64, timezone: &str, scope_key: &str, tool_remaining: i64, input_origin: &str, presentation_mode: &str) -> String {
    format!(
        "utc={now_ms} timezone={timezone} scope={scope_key} tool_remaining={tool_remaining} input_origin={input_origin} presentation_mode={presentation_mode} instruction_authority=none"
    )
}

pub(crate) fn fixed_excludes_origin(fixed: &str) -> bool {
    !fixed.contains("input_origin=") && !fixed.contains("presentation_mode=")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cw_43_dynamic_contains_utc_and_tool_budget() {
        let text = render(1_700_000_000_000, "Asia/Tokyo", "project:p", 9, "voice", "spoken");
        assert!(text.contains("utc="));
        assert!(text.contains("tool_remaining=9"));
    }

    #[test]
    fn cw_43_dynamic_carries_input_origin_not_fixed() {
        let dynamic = render(1, "UTC", "conversation:c", 12, "text", "visual");
        let fixed = "policy CONTEXT_POLICY tools read_record";
        assert!(dynamic.contains("input_origin=text"));
        assert!(fixed_excludes_origin(fixed));
    }
}
