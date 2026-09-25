impl DynamicLanConnection {
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(crate) fn allocation_id(&self) -> &str {
        &self.identity.allocation_id
    }

    pub(crate) fn stream_protocol(&self) -> &str {
        "openai.chat-completions.v1"
    }

    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    pub(crate) fn api_key(&self) -> Option<&str> {
        self.api_key.as_ref().map(|value| value.as_str())
    }

    pub(crate) fn prior_release_failure(&self) -> Option<ErrorKind> {
        self.prior_release_failure
    }

    pub(crate) fn validate_request_budget(
        &self,
        max_output_tokens: u32,
    ) -> Result<(), DynamicLanError> {
        if max_output_tokens == 0 || max_output_tokens > self.context_window.output_reserve_tokens {
            return Err(DynamicLanError::new(
                ErrorKind::Contract,
                "The requested output exceeds the LARM profile context budget.",
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> (u32, u32, u32, u32, u64, u64, bool) {
        (
            self.capacity.max_concurrent_requests,
            self.capacity.active_requests,
            self.capacity.max_queued_requests,
            self.capacity.queue_depth,
            self.capacity.queue_timeout_ms,
            self.capacity.retry_after_ms,
            self.capacity.completion_guaranteed,
        )
    }

    pub(crate) async fn acquire_capacity(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, DynamicLanError> {
        tokio::time::timeout(
            Duration::from_millis(self.capacity.queue_timeout_ms),
            self.capacity_gate.clone().acquire_owned(),
        )
        .await
        .map_err(|_| {
            DynamicLanError::new(
                ErrorKind::Capacity,
                "The LARM provider capacity queue timed out.",
            )
        })?
        .map_err(|_| DynamicLanError::new(ErrorKind::Internal, "The LARM capacity gate closed."))
    }

    pub(crate) async fn ensure_lifetime(
        &mut self,
        request_timeout: Duration,
        cancellation: Arc<RunCancellation>,
    ) -> Result<(), DynamicLanError> {
        let required = request_timeout.saturating_add(REQUEST_LIFETIME_MARGIN);
        if required >= Duration::from_secs(CONNECTION_TTL_SECONDS.into()) {
            return Err(DynamicLanError::new(
                ErrorKind::Contract,
                "The configured provider timeout exceeds the dynamic_lan connection lifetime.",
            ));
        }
        let remaining = self
            .identity
            .expires_at
            .signed_duration_since(chrono::Utc::now())
            .to_std()
            .unwrap_or_default();
        if remaining.is_zero() {
            return Err(DynamicLanError::new(
                ErrorKind::StaleConnection,
                "The dynamic LAN provider connection expired before inference started.",
            ));
        }
        if remaining >= required {
            return Ok(());
        }

        let renew_key = format!("saaa-renew-{}", Uuid::new_v4().simple());
        let renewed = send_json_response::<ConnectionState>(
            &self.client,
            Method::POST,
            connection_renew_url(&self.control_base, &self.identity.id)?,
            self.control_credential.as_ref(),
            Some(("idempotency-key", renew_key.as_str())),
            Some(&json!({ "ttlSeconds": CONNECTION_TTL_SECONDS })),
            &cancellation,
        )
        .await?;
        if renewed.status != StatusCode::OK {
            return Err(contract_error(()));
        }
        let next_identity = validate_renewed_state(&renewed.value, &self.identity, &self.audience)?;
        let (descriptor, health) = claim_and_probe(
            &self.client,
            &self.control_base,
            self.control_credential.as_ref(),
            &next_identity,
            &self.audience,
            url_is_loopback(&self.control_base),
            &cancellation,
        )
        .await?;
        self.identity = next_identity;
        self.endpoint = descriptor.configuration.fields.base_url;

        self.model = descriptor.configuration.fields.model;
        self.api_key = descriptor
            .credential
            .filter(|credential| credential.r#type == "bearer")
            .map(|credential| Zeroizing::new(credential.token));
        self.capacity = health.capacity;
        self.capacity_gate = capacity_gate(&health.capacity);
        Ok(())
    }

    pub(crate) async fn release(mut self) -> Result<(), DynamicLanError> {
        self.api_key.take();
        let url = connection_resource_url(&self.control_base, &self.identity.id)?;
        release_connection(&self.client, &url, self.control_credential.as_ref()).await
    }
}
fn contract_error<T>(_: T) -> DynamicLanError {
    DynamicLanError::new(
        ErrorKind::Contract,
        "dynamic_lan returned an incompatible agent-connection response.",
    )
}
