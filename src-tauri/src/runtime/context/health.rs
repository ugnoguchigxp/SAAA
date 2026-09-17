#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Status {
    Green,
    Yellow,
    Red,
}

impl Status {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Red => "red",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) status: Status,
    pub(crate) reason_codes: Vec<String>,
    pub(crate) projected_bytes: usize,
    pub(crate) hard_limit_bytes: usize,
    pub(crate) selected_sources: usize,
    pub(crate) omitted_sources: usize,
}
