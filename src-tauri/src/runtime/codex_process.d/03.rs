#[cfg(test)]
mod tests {
    use super::developer_instructions;

    #[test]
    fn wd_09_host_snapshot_is_kept_at_the_developer_boundary() {
        let instructions = developer_instructions(
            "HOST_STATE_SNAPSHOT <host-state-snapshot>{}</host-state-snapshot>",
        );
        assert!(instructions.contains("Operate read-only"));
        assert!(instructions.contains("HOST_STATE_SNAPSHOT"));
    }
}
