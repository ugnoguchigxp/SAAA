impl CapabilityReport {
    fn validate(&self, package_hash: &str) -> Result<(), String> {
        if self.version != 1
            || self.verifier != "capability-predicate-v1"
            || self.api_calls != 0
            || self.package_hash.as_deref() != Some(package_hash)
            || !self.unchecked.iter().any(|item| item == "SAAA acceptance")
        {
            return Err("invalid capability report header".into());
        }
        ensure_unique(
            self.results.iter().map(|item| item.id.as_str()),
            "report result",
        )?;
        ensure_unique(
            self.requirements.iter().map(|item| item.id.as_str()),
            "report requirement",
        )?;
        let passed = self
            .results
            .iter()
            .filter(|item| item.status == ReportStatus::Pass)
            .count();
        let failed = self
            .results
            .iter()
            .filter(|item| item.status == ReportStatus::Fail)
            .count();
        let errors = self
            .results
            .iter()
            .filter(|item| item.status == ReportStatus::Error)
            .count()
            + self.diagnostics.len();
        let status = if errors > 0 {
            ReportStatus::Error
        } else if failed > 0 {
            ReportStatus::Fail
        } else {
            ReportStatus::Pass
        };
        if (self.passed, self.failed, self.errors, self.status) != (passed, failed, errors, status)
        {
            return Err("inconsistent capability report totals".into());
        }
        let requirements = self
            .requirements
            .iter()
            .map(|item| (item.id.as_str(), item.case_ids.as_slice()))
            .collect::<HashMap<_, _>>();
        for result in &self.results {
            let expected_status = match &result.actual {
                CaseObservation::Error { code } if code != "INVALID_INPUT" => ReportStatus::Error,
                actual if actual == &result.expected => ReportStatus::Pass,
                _ => ReportStatus::Fail,
            };
            if result.status != expected_status {
                return Err("inconsistent case result status".into());
            }
            if result.origin == CaseOrigin::Suite
                && (result.requirement_ids.is_empty()
                    || result
                        .requirement_ids
                        .iter()
                        .any(|id| !requirements.contains_key(id.as_str())))
            {
                return Err("invalid suite requirement mapping".into());
            }
        }
        for (requirement, case_ids) in requirements {
            let actual = self
                .results
                .iter()
                .filter(|item| {
                    item.origin == CaseOrigin::Suite
                        && item.requirement_ids.iter().any(|id| id == requirement)
                })
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>();
            if actual != case_ids.iter().map(String::as_str).collect::<Vec<_>>() {
                return Err("inconsistent requirement case mapping".into());
            }
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
fn safe_flat_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 128
        && !path.contains('/')
        && !path.contains('\\')
        && path != "."
        && path != ".."
}
pub(super) fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
