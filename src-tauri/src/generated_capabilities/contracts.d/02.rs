impl CapabilityReport {
    pub fn validate(&self, package_hash: &str) -> Result<(), String> {
        if self.version != PACKAGE_VERSION_V2
            || self.verifier != VERIFIER_V2
            || self.api_calls != 0
            || self.package_hash.as_deref() != Some(package_hash)
        {
            return Err("invalid capability report header".into());
        }
        for hash in [
            &self.request_revision,
            &self.suite_hash,
            &self.source_hash,
            &self.program_hash,
            &self.artifact_hash,
        ] {
            if hash.as_deref().map(|value| !is_hash(value)).unwrap_or(true) {
                return Err("invalid capability report hash".into());
            }
        }
        if self.results.is_empty() {
            return Err("capability report has no cases".into());
        }
        ensure_unique(
            self.results.iter().map(|item| item.id.as_str()),
            "report result",
        )?;
        ensure_unique(
            self.requirements.iter().map(|item| item.id.as_str()),
            "report requirement",
        )?;
        let requirements = self
            .requirements
            .iter()
            .map(|item| (item.id.as_str(), item.case_ids.as_slice()))
            .collect::<std::collections::HashMap<_, _>>();
        for result in &self.results {
            let expected_status = match &result.actual {
                CaseObservation::Error { code } if code != "INVALID_INPUT" => ReportStatus::Error,
                actual if actual == &result.expected => ReportStatus::Pass,
                _ => ReportStatus::Fail,
            };
            if result.status != expected_status {
                return Err("inconsistent case result status".into());
            }
            if result
                .requirement_ids
                .iter()
                .any(|id| !requirements.contains_key(id.as_str()))
            {
                return Err("invalid requirement mapping".into());
            }
        }
        for (requirement, case_ids) in requirements {
            let actual = self
                .results
                .iter()
                .filter(|item| item.requirement_ids.iter().any(|id| id == requirement))
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>();
            if actual != case_ids.iter().map(String::as_str).collect::<Vec<_>>() {
                return Err("inconsistent requirement case mapping".into());
            }
        }
        if self
            .uncovered_requirements
            .iter()
            .any(|id| !self.requirements.iter().any(|item| &item.id == id))
        {
            return Err("unknown uncovered requirement".into());
        }
        let expected = if self.diagnostics.is_empty()
            && !self
                .results
                .iter()
                .any(|item| item.status == ReportStatus::Error)
        {
            if self
                .results
                .iter()
                .any(|item| item.status == ReportStatus::Fail)
                || !self.uncovered_requirements.is_empty()
            {
                ReportStatus::Fail
            } else {
                ReportStatus::Pass
            }
        } else {
            ReportStatus::Error
        };
        if self.status != expected {
            return Err("inconsistent capability report status".into());
        }
        Ok(())
    }
}
fn ensure_unique<'a>(values: impl Iterator<Item = &'a str>, label: &str) -> Result<(), String> {
    let mut seen = HashSet::new();
    if values.into_iter().any(|value| !seen.insert(value)) {
        return Err(format!("duplicate {label}"));
    }
    Ok(())
}
pub fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
pub fn is_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}
pub fn is_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    value.len() <= 64
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}
fn is_field_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphabetic() || first == b'_' || first == b'$' => {}
        _ => return false,
    }
    value.len() <= 256
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$')
}
/// Flat package file name: no separators, never the manifest itself.
pub fn safe_flat_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 128
        && !path.contains('/')
        && !path.contains('\\')
        && path != "."
        && path != ".."
        && path != "capability.json"
        && !path.starts_with('.')
}
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
/// L-Lang's `canonical`: object keys sorted, arrays kept in order, scalars via JSON encoding.
pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}
fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => out.push_str(&value.to_string()),
        Value::String(value) => {
            out.push_str(&serde_json::to_string(value).expect("string serializes"));
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("key serializes"));
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
    }
}
/// `package_hash` uses the fixed L-Lang rule: `contentHash(parsed manifest)`.
pub fn package_hash(manifest: &Value) -> String {
    sha256_hex(canonical(manifest).as_bytes())
}
/// `contract_hash` covers only the initial subset: sorted field names, input type, requiredness,
/// profile and output type. Description and release are excluded. Format marker: `contract_format=1`.
pub fn contract_hash(contract: &WasmContract) -> String {
    let mut fields = contract.fields.iter().collect::<Vec<_>>();
    fields.sort_by(|left, right| left.name.cmp(&right.name));
    let mut text = format!(
        "contract_format={CONTRACT_FORMAT}\nprofile={PROFILE_PREDICATE_I32_V1}\noutput=boolean\n"
    );
    for field in fields {
        text.push_str(&format!("field\t{}\tboolean\trequired\n", field.name));
    }
    sha256_hex(text.as_bytes())
}
/// Digest over the fixed-order `(relative name, byte hash)` inventory of the managed copy.
pub fn inventory_hash(entries: &[(String, String)]) -> String {
    let mut text = String::new();
    for (name, hash) in entries {
        text.push_str(name);
        text.push('\u{0}');
        text.push_str(hash);
        text.push('\n');
    }
    sha256_hex(text.as_bytes())
}
#[derive(Clone, Debug)]
pub struct ResolvedCapability {
    pub capability_id: String,
    pub revision_id: String,
    pub package_hash: String,
    pub contract_hash: String,
    pub catalog_epoch: i64,
    pub contract: WasmContract,
}
impl ResolvedCapability {
    pub fn input_fields(&self) -> Vec<String> {
        self.contract
            .fields
            .iter()
            .map(|field| field.name.clone())
            .collect()
    }
}
#[derive(Clone, Debug)]
pub struct InvokeRequest {
    pub resolved: ResolvedCapability,
    pub call_id: String,
    pub input: Map<String, Value>,
    pub inner_timeout_ms: u64,
    pub origin: &'static str,
    /// Host-derived ownership. `None` means the call has no recorded owner and cannot be
    /// inspected (plan 12.4); it is never attributed to the current user.
    pub actor: Option<CallActor>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallActor {
    pub principal_id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub run_id: String,
}
impl InvokeRequest {
    pub fn new(resolved: ResolvedCapability, call_id: String, input: Map<String, Value>) -> Self {
        Self {
            resolved,
            call_id,
            input,
            inner_timeout_ms: INNER_TIMEOUT_DEFAULT_MS,
            origin: "internal",
            actor: None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct InvocationResult {
    pub call_id: String,
    pub revision_id: String,
    pub package_hash: String,
    pub value: bool,
    pub elapsed_ms: u64,
}
