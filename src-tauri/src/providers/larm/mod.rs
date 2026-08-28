#[allow(dead_code)]
pub(crate) mod contracts;
#[allow(dead_code)]
pub(crate) mod session;

#[cfg(test)]
mod tests {
    #[test]
    fn pre_g1_contract_and_session_modules_have_no_io_dependencies() {
        for source in [include_str!("contracts.rs"), include_str!("session.rs")] {
            for forbidden in ["reqwest::", "rusqlite::", "tauri::", "std::fs", "std::net"] {
                assert!(
                    !source.contains(forbidden),
                    "pure LARM module contains forbidden dependency: {forbidden}"
                );
            }
        }
    }
}
