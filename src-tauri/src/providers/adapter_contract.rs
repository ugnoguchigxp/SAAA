#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheSupport {
    Verified,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryBinding {
    None,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageMapping {
    OpenAiChat,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdapterContract {
    pub(crate) cache_support: CacheSupport,
    pub(crate) history_binding: HistoryBinding,
    pub(crate) usage_mapping: UsageMapping,
    pub(crate) token_capacity: Option<u32>,
    pub(crate) byte_transport_limit: usize,
}

pub(crate) const CHAT_COMPLETIONS: AdapterContract = AdapterContract {
    cache_support: CacheSupport::Unknown,
    history_binding: HistoryBinding::None,
    usage_mapping: UsageMapping::OpenAiChat,
    token_capacity: None,
    byte_transport_limit: 96_000,
};

pub(crate) const AGENT_SESSION: AdapterContract = AdapterContract {
    cache_support: CacheSupport::Unsupported,
    history_binding: HistoryBinding::Session,
    usage_mapping: UsageMapping::None,
    token_capacity: None,
    byte_transport_limit: 96_000,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cw_53_agent_session_declares_session_binding() {
        assert_eq!(AGENT_SESSION.history_binding, HistoryBinding::Session);
        assert_eq!(CHAT_COMPLETIONS.history_binding, HistoryBinding::None);
    }
}
