# records

Owns immutable captures of tool and web results. Callers publish a body only after `commit` succeeds.

Invariants: authorization SQL runs before LIMIT; original bytes are not redacted; `record_fts` is updated only by the FTS helpers, not triggers.
