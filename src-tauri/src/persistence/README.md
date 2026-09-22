# Persistence

Owner: application SQLite ownership, reads/writes, schema migration, settings validation, conversation/run records, and audit storage. Domain ledgers define their own state semantics.

Preserve:
- One owning writer; production readers use read-only WAL lanes. Never introduce an independent write connection.
- Keep transactions short and synchronous. In-memory readers can share the writer mutex: never nest a reader call inside write/transact.
- Persist related transitions atomically; keep one-shot run finalization and domain adoption checks intact.
- Validate settings as a batch, including cross-document provider bindings. Stored route settings may differ from the effective Role Routing route.
- Migrations must preserve existing user configuration and work on existing DBs, not only fresh schemas. Do not replace configured values with current deployment assumptions.
- Backups/restores must preserve forget/recovery guarantees. Routine audit stores bounded metadata, not secrets or unrestricted source text.

Locate:
- Writer ownership: `sqlite/owner.rs`, `sqlite/writer.rs`; read lanes/cache: `sqlite/readers.rs`.
- Schema/version: `schema.rs` -> `migrate.rs`, `migrate.d/`; domain schemas are called from here.
- Settings load/save/binding: `settings.rs`, `settings.d/`; defaults/migration: `settings_defaults.rs`, `settings_migration.rs`, `settings_migration.d/`.
- Effective route: `effective_route.rs`, `../runtime/conversation_inputs_roles.d/01.rs`; provider identity: `provider_identity.rs`.
- Conversations/paging: `conversations.rs`, `conversation_page.rs`; lifecycle: `runs.rs`, `../providers/session_store.rs`.
- Audit contract: `audit.rs`, `audit.d/`; IPC: `app_commands.rs`.
- Backup/forget recovery: `../database_backup.rs`, `../backup.rs`, `../memory/personal_state/journal.rs`.
- Tests: `sqlite/tests.rs`, `../../tests/sqlite_architecture.rs`, migration/settings inline tests.

For a stale display, compare committed DB state, reader snapshot/revision, and event delivery before changing writes. A successful write call does not prove the consumer observed the same revision.
