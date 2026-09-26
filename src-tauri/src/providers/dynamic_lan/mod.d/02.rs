impl DynamicLanConnection {
    pub(crate) async fn resolve(
        host: &str,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Self, DynamicLanError> {
        Self::resolve_at(control_base_url(host)?, cancellation).await
    }

    #[cfg(test)]
    pub(crate) async fn resolve_world_fixture(
        base: Url,
        cancel: Arc<RunCancellation>,
    ) -> Result<Self, DynamicLanError> {
        Self::resolve_at(base, cancel).await
    }

    async fn resolve_at(
        control_base: Url,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Self, DynamicLanError> {
        let control_credential = Some(control_credential()?);
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| {
                DynamicLanError::new(
                    ErrorKind::Internal,
                    "Could not initialize the dynamic_lan discovery client.",
                )
            })?;
        let mut prior_release_failure = None;
        for attempt in 0..2 {
            match Self::resolve_once(
                client.clone(),
                control_base.clone(),
                control_credential.clone(),
                cancellation.clone(),
            )
            .await
            {
                Ok(mut connection) => {
                    connection.prior_release_failure =
                        prior_release_failure.or(connection.prior_release_failure);
                    return Ok(connection);
                }
                Err(error) if error.kind == ErrorKind::StaleConnection && attempt == 0 => {
                    prior_release_failure = prior_release_failure.or(error.release_failure);
                }
                Err(mut error) => {
                    error.release_failure = prior_release_failure.or(error.release_failure);
                    return Err(error);
                }
            }
        }
        let mut error = DynamicLanError::new(
            ErrorKind::StaleConnection,
            "The dynamic_lan daemon restarted while resolving the provider connection.",
        );
        error.release_failure = prior_release_failure;
        Err(error)
    }

    async fn resolve_once(
        client: reqwest::Client,
        control_base: Url,
        control_credential: Option<HeaderValue>,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Self, DynamicLanError> {
        let control_is_loopback = url_is_loopback(&control_base);
        let mut profile_url = control_base
            .join("v3/agent-profiles")
            .map_err(contract_error)?;
        profile_url
            .query_pairs_mut()
            .append_pair("profile", PROFILE_SELECTOR);
        let profiles = send_json_response::<AgentProfileCatalog>(
            &client,
            Method::GET,
            profile_url,
            control_credential.as_ref(),
            None,
            None,
            &cancellation,
        )
        .await?;
        validate_config_revision(profiles.config_revision.as_deref())?;
        let selected_profile = select_default_llm_profile(&profiles.value)?;
        let audience = select_audience(&profiles.value.audiences)?.to_string();

        let idempotency_key = format!("saaa-{}", Uuid::new_v4().simple());
        let create_body = json!({
            "profile": PROFILE_SELECTOR,
            "audience": audience.as_str(),
            "client": CLIENT_ID,
            "ttlSeconds": CONNECTION_TTL_SECONDS,
            "allowFallback": false,
            "deploymentPolicy": "existing-only"
        });
        let create_url = control_base
            .join("v1/agent-connections")
            .map_err(contract_error)?;
        let created = send_json_response::<Value>(
            &client,
            Method::POST,
            create_url,
            control_credential.as_ref(),
            Some(("idempotency-key", idempotency_key.as_str())),
            Some(&create_body),
            // Receive a late creation id so the detached initializer can release it.
            &RunCancellation::default(),
        )
        .await?;
        if !matches!(
            created.status,
            StatusCode::OK | StatusCode::CREATED | StatusCode::ACCEPTED
        ) {
            return Err(contract_error(()));
        }
        let raw_id = created
            .value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| validate_connection_id(id).is_ok())
            .map(str::to_owned);
        let state_result = serde_json::from_value::<ConnectionState>(created.value).map_err(|_| {
            DynamicLanError::with_code(
                ErrorKind::Contract,
                "Harness response does not match the expected schema.",
                "harness-connection-schema-invalid",
            )
        });
        let mut state = match state_result {
            Ok(state) => state,
            Err(error) => {
                if let Some(id) = raw_id {
                    if let Ok(url) = connection_resource_url(&control_base, &id) {
                        return Err(error_after_release(
                            error,
                            &client,
                            &url,
                            control_credential.as_ref(),
                        )
                        .await);
                    }
                }
                return Err(error);
            }
        };
        let identity = match validate_initial_state(&state, &audience, &selected_profile) {
            Ok(identity) => identity,
            Err(error) => {
                if let Ok(url) = connection_resource_url(&control_base, &state.id) {
                    return Err(error_after_release(
                        error,
                        &client,
                        &url,
                        control_credential.as_ref(),
                    )
                    .await);
                }
                return Err(error);
            }
        };
        let connection_url = connection_resource_url(&control_base, &identity.id)?;
        if matches!(created.status, StatusCode::CREATED | StatusCode::ACCEPTED) {
            if let Err(error) = validate_create_location(
                created.location.as_deref(),
                &control_base,
                &connection_url,
            ) {
                return Err(error_after_release(
                    error,
                    &client,
                    &connection_url,
                    control_credential.as_ref(),
                )
                .await);
            }
        }
        let mut poll_interval = created.retry_after.unwrap_or(POLL_INTERVAL);
        let ready_deadline = tokio::time::Instant::now() + READY_TIMEOUT;

        loop {
            match state.status.as_str() {
                "ready" => break,
                "pending" | "probing" => {}
                "failed" => {
                    let kind = state
                        .error
                        .as_ref()
                        .map(|error| error.code.as_str())
                        .unwrap_or_default();
                    let error = classify_api_error(kind);
                    return Err(error_after_release(
                        error,
                        &client,
                        &connection_url,
                        control_credential.as_ref(),
                    )
                    .await);
                }
                "released" | "expired" => {
                    let error = DynamicLanError::new(
                        ErrorKind::StaleConnection,
                        "The dynamic LAN provider connection became inactive before it was ready.",
                    );
                    return Err(error_after_release(
                        error,
                        &client,
                        &connection_url,
                        control_credential.as_ref(),
                    )
                    .await);
                }
                _ => {
                    return Err(error_after_release(
                        contract_error(()),
                        &client,
                        &connection_url,
                        control_credential.as_ref(),
                    )
                    .await);
                }
            }
            let remaining = ready_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                let error = DynamicLanError::new(
                    ErrorKind::Timeout,
                    "dynamic_lan did not finish resolving the provider before the startup timeout.",
                );
                return Err(error_after_release(
                    error,
                    &client,
                    &connection_url,
                    control_credential.as_ref(),
                )
                .await);
            }
            if let Err(error) = cancellable_sleep(poll_interval.min(remaining), &cancellation).await
            {
                return Err(error_after_release(
                    error,
                    &client,
                    &connection_url,
                    control_credential.as_ref(),
                )
                .await);
            }
            if tokio::time::Instant::now() >= ready_deadline {
                let error = DynamicLanError::new(
                    ErrorKind::Timeout,
                    "dynamic_lan did not finish resolving the provider before the startup timeout.",
                );
                return Err(error_after_release(
                    error,
                    &client,
                    &connection_url,
                    control_credential.as_ref(),
                )
                .await);
            }
            let polled = send_json_response::<ConnectionState>(
                &client,
                Method::GET,
                connection_url.clone(),
                control_credential.as_ref(),
                None,
                None,
                &cancellation,
            )
            .await;
            match polled {
                Ok(next) => {
                    if let Err(error) = validate_successor_state(&next.value, &identity, &audience)
                    {
                        return Err(error_after_release(
                            error,
                            &client,
                            &connection_url,
                            control_credential.as_ref(),
                        )
                        .await);
                    }
                    poll_interval = next.retry_after.unwrap_or(POLL_INTERVAL);
                    state = next.value;
                }
                Err(error) => {
                    return Err(error_after_release(
                        error,
                        &client,
                        &connection_url,
                        control_credential.as_ref(),
                    )
                    .await);
                }
            }
        }

        let (descriptor, health) = match claim_and_probe(
            &client,
            &control_base,
            control_credential.as_ref(),
            &identity,
            &audience,
            control_is_loopback,
            &cancellation,
        )
        .await
        {
            Ok(descriptor) => descriptor,
            Err(error) => {
                return Err(error_after_release(
                    error,
                    &client,
                    &connection_url,
                    control_credential.as_ref(),
                )
                .await);
            }
        };

        let context_window = identity.profile.context_window;
        Ok(Self {
            client,
            control_base,
            control_credential,
            identity,
            audience,
            endpoint: descriptor.configuration.fields.base_url,
            model: descriptor.configuration.fields.model,
            api_key: descriptor
                .credential
                .filter(|credential| credential.r#type == "bearer")
                .map(|credential| Zeroizing::new(credential.token)),
            context_window,
            capacity: health.capacity,
            capacity_gate: capacity_gate(&health.capacity),
            prior_release_failure: None,
        })
    }

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
