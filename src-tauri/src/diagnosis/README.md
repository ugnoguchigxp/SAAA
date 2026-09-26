# Diagnosis

Owns the self-diagnosis report: `DiagnosisStore` (single-flight), startup fast checks, and manual operational checks. IPC exposes `get_diagnosis_report`, `run_fast_diagnosis`, and `run_diagnosis` (operational).

Invariants: startup `spawn_startup` does not block setup or allocate a LARM Connection. Fast checks read local state and the LARM catalog; advertised services are not reported as operationally healthy. Operational checks have a 75-second deadline; their LARM path uses one claimed Session for LLM, backchannel, ASR, TTS, and embedding probes, then releases it. Runs wait for an in-flight run and then execute. The report carries `diagnosis.mode.fast` or `diagnosis.mode.operational` as an item so the frozen RuntimeEvent IPC contract stays unchanged. Item messages go through `redact_runtime_text` and do not carry URLs or tokens.

Search: `diagnosis::runner::collect`, `diagnosis::store::DiagnosisStore`, `diagnosis-updated`.
