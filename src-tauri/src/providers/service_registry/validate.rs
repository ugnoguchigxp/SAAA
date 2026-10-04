use super::types::*;
use std::collections::HashSet;

/// Cross-reference validation for a whole snapshot. Disabled resources may stay
/// bound (the user disabled them); resolution rejects them at run time.
pub(crate) fn validate_snapshot(snapshot: &RegistrySnapshot) -> Result<(), String> {
    let mut connection_ids = HashSet::new();
    for connection in &snapshot.connections {
        if connection.connection_id.trim().is_empty()
            || !connection_ids.insert(connection.connection_id.as_str())
        {
            return Err(format!(
                "Invalid or duplicate connection id: {}",
                connection.connection_id
            ));
        }
    }
    let mut resource_ids = HashSet::new();
    for resource in &snapshot.resources {
        if resource.resource_id.trim().is_empty()
            || !resource_ids.insert(resource.resource_id.as_str())
        {
            return Err(format!(
                "Invalid or duplicate resource id: {}",
                resource.resource_id
            ));
        }
        if !connection_ids.contains(resource.connection_id.as_str()) {
            return Err(format!(
                "Resource references an unknown connection: {}",
                resource.resource_id
            ));
        }
    }
    let mut purposes = HashSet::new();
    for binding in &snapshot.bindings {
        if !purposes.insert(binding.purpose) {
            return Err(format!("Duplicate binding: {}", binding.purpose.id()));
        }
        let mut seen = HashSet::new();
        let ids = binding
            .primary_resource_id
            .iter()
            .chain(binding.fallback_resource_ids.iter());
        for id in ids {
            let resource = snapshot.resource(id).ok_or_else(|| {
                format!(
                    "Binding {} references an unknown resource: {id}",
                    binding.purpose.id()
                )
            })?;
            if resource.capability != binding.purpose.required_capability() {
                return Err(format!(
                    "Resource {id} does not support {}",
                    binding.purpose.id()
                ));
            }
            if !seen.insert(id.as_str()) {
                return Err(format!(
                    "Duplicate resource in {} route: {id}",
                    binding.purpose.id()
                ));
            }
        }
        if binding.primary_resource_id.is_none() && !binding.fallback_resource_ids.is_empty() {
            return Err("A fallback requires a primary resource".to_string());
        }
        if binding.timeout_ms == 0 {
            return Err(format!("Binding {} needs a timeout", binding.purpose.id()));
        }
    }
    Ok(())
}
