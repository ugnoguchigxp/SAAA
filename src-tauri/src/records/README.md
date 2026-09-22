# records

Owns immutable captures of tool and web results. Callers publish a body only after `commit` succeeds.

Invariants: save before publish; do not redact original bytes; authorization SQL runs before LIMIT; `record_fts` is updated only by `fts.rs`, not triggers. FTS shadow tables are not covered by `secure_delete`.
