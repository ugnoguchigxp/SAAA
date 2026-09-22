# Diagnosis

Owns the startup self-diagnosis report: `DiagnosisStore` (single-flight), `runner::collect`, and IPC `get_diagnosis_report` / `run_diagnosis`.

Invariants: startup `spawn_startup` does not block setup; a second `try_begin` waits for the in-flight run; item messages go through `redact_runtime_text` and do not carry URLs or tokens. Checks call existing probes only.

Search: `diagnosis::runner::collect`, `diagnosis::store::DiagnosisStore`, `diagnosis-updated`.
