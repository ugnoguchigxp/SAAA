pub(crate) mod builder;
pub(crate) mod carry;
pub(crate) mod dynamic;
pub(crate) mod entries;
pub(crate) mod manifest;
pub(crate) mod schema;
pub(crate) mod triggers;

pub(crate) fn path_selected(segments_enabled: bool, history_binding_none: bool, chat_completions: bool) -> bool {
    segments_enabled && history_binding_none && chat_completions
}

pub(crate) fn prefix_match_bytes(previous: &[u8], current: &[u8]) -> usize {
    previous.iter().zip(current.iter()).take_while(|(left, right)| left == right).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cw_45_segment_path_used_only_for_chat_completions() {
        assert!(path_selected(true, true, true));
        assert!(!path_selected(true, true, false));
    }

    #[test]
    fn cw_45_agent_session_uses_legacy_path() {
        assert!(!path_selected(true, false, false));
    }

    #[test]
    fn cw_45_prefix_match_bytes_grows_when_prefix_stable() {
        let fixed = b"FIXED-POLICY";
        let first = [fixed, b"|L1"].concat();
        let second = [fixed, b"|L1|L2"].concat();
        assert!(prefix_match_bytes(&first, &second) >= fixed.len());
    }
}
