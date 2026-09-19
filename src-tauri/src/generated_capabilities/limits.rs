use std::time::Duration;

// Initial M1 ceilings. Boundary tests live next to the call sites; changing a value here is a
// deliberate contract change, never a way to make a failing fixture pass.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_STDOUT_BYTES: usize = 1024 * 1024;
pub const MAX_STDERR_BYTES: usize = 64 * 1024;

pub const INSPECT_TIMEOUT: Duration = Duration::from_secs(15);
pub const VERIFY_TIMEOUT: Duration = Duration::from_secs(30);
pub const INVOKE_TIMEOUT: Duration = Duration::from_secs(15);

pub const INNER_TIMEOUT_DEFAULT_MS: u64 = 1_000;
pub const INNER_TIMEOUT_MAX_MS: u64 = 10_000;

/// candidate import: manifest + 5 referenced files, manifest <= 64 KiB, each file <= 1 MiB,
/// total <= 6 MiB. These mirror the fixed L-Lang per-file limit (1 MiB).
pub const MAX_CANDIDATE_FILES: usize = 6;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_CANDIDATE_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_CANDIDATE_TOTAL_BYTES: u64 = 6 * 1024 * 1024;

pub const MAX_ACCEPTANCE_CASES: usize = 256;
pub const ACCEPTANCE_DEADLINE: Duration = Duration::from_secs(60);

pub const MAX_METADATA_TEXT_BYTES: usize = 2 * 1024;

/// The M1 input subset: 1..=8 mandatory boolean fields.
pub const MAX_CONTRACT_FIELDS: usize = 8;
pub const MIN_CONTRACT_FIELDS: usize = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_ceilings_match_the_documented_contract() {
        assert_eq!(MAX_REQUEST_BYTES, 64 * 1024);
        assert_eq!(MAX_STDOUT_BYTES, 1024 * 1024);
        assert_eq!(MAX_STDERR_BYTES, 64 * 1024);
        assert_eq!(INSPECT_TIMEOUT, Duration::from_secs(15));
        assert_eq!(VERIFY_TIMEOUT, Duration::from_secs(30));
        assert_eq!(INVOKE_TIMEOUT, Duration::from_secs(15));
        assert_eq!(INNER_TIMEOUT_DEFAULT_MS, 1_000);
        assert_eq!(INNER_TIMEOUT_MAX_MS, 10_000);
        assert_eq!(MAX_CANDIDATE_FILES, 6);
        assert_eq!(MAX_CANDIDATE_TOTAL_BYTES, 6 * 1024 * 1024);
        assert_eq!(MAX_CANDIDATE_FILE_BYTES, 1024 * 1024);
        assert_eq!(MAX_MANIFEST_BYTES, 64 * 1024);
        assert_eq!(MAX_ACCEPTANCE_CASES, 256);
        assert_eq!(ACCEPTANCE_DEADLINE, Duration::from_secs(60));
        assert_eq!(MAX_CONTRACT_FIELDS, 8);
        assert_eq!(MIN_CONTRACT_FIELDS, 1);
    }
}
