use rusqlite::types::Value;

#[derive(Debug, Clone)]
pub(crate) struct Authorization {
    pub(crate) principal_id: String,
    pub(crate) conversation_id: String,
    pub(crate) allowed_scope_keys: Vec<String>,
}

impl Authorization {
    pub(crate) fn sql_filter(&self, alias: &str) -> (String, Vec<Value>) {
        let mut params = vec![
            Value::Text(self.principal_id.clone()),
            Value::Text(self.conversation_id.clone()),
        ];
        let scope = if self.allowed_scope_keys.is_empty() {
            "0=1".to_string()
        } else {
            let marks = vec!["?"; self.allowed_scope_keys.len()].join(",");
            for key in &self.allowed_scope_keys {
                params.push(Value::Text(key.clone()));
            }
            format!("s.scope_key IN ({marks})")
        };
        let sql = format!(
            "{alias}.principal_id = ? AND {alias}.forget_epoch IS NULL AND (NOT EXISTS(SELECT 1 FROM record_scopes s WHERE s.record_id = {alias}.id) AND {alias}.conversation_id = ? OR EXISTS(SELECT 1 FROM record_scopes s WHERE s.record_id = {alias}.id AND {scope}))"
        );
        (sql, params)
    }
}
