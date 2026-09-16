use super::{generation, managed::Adapter, worker};
use crate::RunCancellation;
use saaa_personal_state_core::SourceRef;
use serde_json::{json, Value};
use std::sync::Arc;
pub async fn extract(
    a: &Adapter,
    input: Value,
    cancel: Arc<RunCancellation>,
) -> Result<String, String> {
    let source: SourceRef = serde_json::from_value(input["source"]["ref"].clone())
        .map_err(|_| "personal-extraction-source")?;
    let ledger = a.writer.read_serialized(super::store::load)?;
    let mut sources = vec![source.clone()];
    if let Some(pending) = input["current"]["pending"].as_array() {
        for entry in pending {
            let s: SourceRef = serde_json::from_value(entry["source"].clone())
                .map_err(|_| "personal-base-source")?;
            if !sources.contains(&s) {
                sources.push(s);
            }
        }
    }
    for assertion in ledger.assertions.values() {
        for key in &assertion.input_dependencies {
            if let Some(s) = ledger.sources.get(key) {
                if !sources.contains(s) {
                    sources.push(s.clone());
                }
            }
        }
    }
    let request = json!({"model":a.certification.model,"messages":[{"role":"system","content":worker::EXTRACTION_INSTRUCTION},{"role":"user","content":super::encode(&json!({"current":input["current"],"source_ref":source.key,"request_scope":input["request_scope"],"instructionAuthority":"none"}))?}],"max_tokens":2000,"temperature":0,"stream":false});
    let id = crate::new_id("generation");
    let m = generation::Manifest {
        generation_id: id.clone(),
        attempt_id: crate::new_id("attempt"),
        run_id: format!("extract-{id}"),
        request_revision: 1,
        input_epoch: ledger.input_epoch,
        policy_revision: ledger.policy_revision,
        projection_revision: ledger.revision,
        purpose: "personal_state_extract".into(),
        request_digest: String::new(),
        sources,
        allocation: a.certification.allocation.clone(),
        runtime: a.certification.runtime.clone(),
        release: a.certification.release.clone(),
        view_id: None,
        view_digest: None,
        lease_epoch: a.certification.lease_epoch,
        expires_at: a.certification.expires_at,
    };
    let (_, v) = super::product::infer(a, m, &request, &[source], cancel).await?;
    v["choices"][0]["message"]["content"]
        .as_str()
        .filter(|s| s.len() <= 16384)
        .map(str::to_string)
        .ok_or("personal-generation-output".into())
}
