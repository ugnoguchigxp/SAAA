use super::*;

/// Definition and execution reference issued together for one host-selected tool.
#[derive(Clone, Debug)]
pub struct DirectOffer {
    pub definition: Value,
    pub execution_ref: String,
    pub revision_id: String,
}

impl ToolSelectionService {
    /// Offers one known tool id for a host-confirmed context. The model does not choose the id.
    /// A closed or inactive tool returns no definition and no reference.
    pub fn offer_direct(
        &self,
        context: &RequestContext,
        tool_id: &str,
    ) -> ToolSelectionResult<Option<DirectOffer>> {
        if tool_id != "artifact_webview" {
            return Err(ToolSelectionError::not_found());
        }
        if !crate::artifact_preview::webview_ops::is_offered_for(&context.conversation_id) {
            return Ok(None);
        }
        let principal = context.principal_id.clone();
        let conversation = context.conversation_id.clone();
        let project = context.project_id.clone();
        let loaded = self
            .writer
            .read_serialized(move |connection| {
                let definition = crate::artifact_preview::webview_catalog::offered_definition(
                    connection, &principal,
                )
                .map_err(|error| error.to_string())?;
                let Some(definition) = definition else {
                    return Ok(None);
                };
                let tool = repository::tool_by_id(connection, tool_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "not-found".to_string())?;
                let revision_id = tool
                    .current_revision_id
                    .clone()
                    .ok_or_else(|| "not-found".to_string())?;
                let revision = repository::revision_by_id(connection, &revision_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "not-found".to_string())?;
                let epochs = repository::epochs(connection).map_err(|error| error.to_string())?;
                Ok(Some((definition, tool, revision, epochs)))
            })
            .map_err(|error| match error.as_str() {
                "not-found" => ToolSelectionError::not_found(),
                _ => ToolSelectionError::storage(),
            })?;
        let Some((definition, tool, revision, epochs)) = loaded else {
            return Ok(None);
        };
        let decision_id = crate::new_id("tsdec");
        let now = now_ms();
        let decision = DecisionRecord {
            id: decision_id.clone(),
            principal_id: context.principal_id.clone(),
            conversation_id: conversation,
            run_id: context.run_id.clone(),
            message_id: context.input_message_id.clone(),
            scenario: Scenario::degraded("direct-offer"),
            catalog_epoch: epochs.catalog,
            acl_epoch: epochs.acl,
            rule_epoch: epochs.rule,
            model_hash: None,
            status: DecisionStatus::Ok,
            created_at: now,
            candidates: vec![CandidateRecord {
                revision_id: revision.id.clone(),
                tool_id: tool.id.clone(),
                lex_rank: None,
                vec_rank: None,
                raw_score: None,
                base_score: 1.0,
                final_score: 1.0,
                rule_ids: Vec::new(),
                final_rank: 1,
            }],
        };
        write_transaction(&self.writer, move |connection| {
            repository::insert_decision(connection, &decision).map_err(|error| error.to_string())
        })
        .map_err(|_| ToolSelectionError::storage())?;
        let candidate = ReferenceEntry {
            kind: ReferenceKind::Execution,
            decision_id,
            revision_id: revision.id.clone(),
            tool_id: tool.id,
            schema_hash: revision.schema_hash.clone(),
            principal_id: context.principal_id.clone(),
            conversation_id: context.conversation_id.clone(),
            run_id: context.run_id.clone(),
            project_id: project,
            task_id: context.task_id.clone(),
            catalog_epoch: epochs.catalog,
            acl_epoch: epochs.acl,
            rule_epoch: epochs.rule,
            input_schema: revision.input_schema.clone(),
            created_at_ms: now,
        };
        let execution_ref = self.references.issue(candidate, now)?;
        crate::artifact_preview::webview_ops::stamp_offer(&execution_ref, &context.conversation_id);
        Ok(Some(DirectOffer {
            definition,
            execution_ref,
            revision_id: revision.id,
        }))
    }
}
