impl DynamicLanConnection {
    pub(crate) async fn resolve(
        host: &str,
        stored_profile: Option<&str>,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Self, DynamicLanError> {
        Self::resolve_with_profile(control_base_url(host)?, stored_profile, cancellation).await
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
        Self::resolve_with_profile(control_base, None, cancellation).await
    }

    async fn resolve_with_profile(
        control_base: Url,
        stored_profile: Option<&str>,
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
                stored_profile,
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
        stored_profile: Option<&str>,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Self, DynamicLanError> {
        let control_is_loopback = url_is_loopback(&control_base);
        let token = control_token()?;
        let preference = crate::providers::larm_resources::profile::preference(stored_profile);
        let selected_profile = match preference {
            saaa_larm_session::ProfilePreference::Variant(variant) => {
                let catalog = saaa_larm_session::catalog::fetch(
                    &client,
                    &control_base,
                    &token,
                    variant.selector(),
                )
                .await
                .map_err(|code| {
                    DynamicLanError::with_code(
                        ErrorKind::Contract,
                        "LARM profile catalog was rejected.",
                        code,
                    )
                })?;
                selected_llm_from_catalog(&catalog)?
            }
            saaa_larm_session::ProfilePreference::Explicit(id) => SelectedLlmProfile {
                selector: id.clone(),
                catalog_revision: None,
                catalog_models: None,
                id,
                capability: String::new(),
                model: String::new(),
                protocol: "openai.chat-completions.v1".into(),
                context_window: ProviderContextWindow {
                    max_tokens: 1,
                    output_reserve_tokens: 1,
                    safety_margin_tokens: 1,
                },
                compare_catalog: false,
            },
        };
        let audience = AUDIENCE.to_string();

        let idempotency_key = format!("saaa-{}", Uuid::new_v4().simple());
        let mut create_body = json!({
            "profile": selected_profile.selector.as_str(),
            "audience": audience.as_str(),
            "client": CLIENT_ID,
            "ttlSeconds": CONNECTION_TTL_SECONDS,
            "allowFallback": false,
            "deploymentPolicy": "existing-only"
        });
        if let Some(revision) = &selected_profile.catalog_revision {
            create_body["expectedCatalogRevision"] = json!(revision);
        }
        let create_url = control_base
            .join("v1/agent-connections")
            .map_err(contract_error)?;
        let ready_deadline = tokio::time::Instant::now() + READY_TIMEOUT + Duration::from_secs(15);
        let created = send_json_response::<ConnectionState>(
            &client,
            Method::POST,
            create_url.clone(),
            control_credential.as_ref(),
            Some(("idempotency-key", idempotency_key.as_str())),
            Some(&create_body),
            // Receive a late creation id so the detached initializer can release it.
            &RunCancellation::default(),
        )
        .await;
        let mut created = match created {
            Err(error) if matches!(error.kind, ErrorKind::Network | ErrorKind::Timeout) => {
                send_json_response::<ConnectionState>(
                    &client,
                    Method::POST,
                    create_url,
                    control_credential.as_ref(),
                    Some(("idempotency-key", idempotency_key.as_str())),
                    Some(&create_body),
                    &RunCancellation::default(),
                )
                .await?
            }
            result => result?,
        };
        if created.value.id.is_empty() {
            if let Some(location) = created.location.as_deref() {
                let url = control_base.join(location).map_err(contract_error)?;
                if url.origin() == control_base.origin()
                    && url.query().is_none()
                    && url.fragment().is_none()
                {
                    if let Some(id) = url.path().strip_prefix("/v1/agent-connections/") {
                        if !id.contains('/') {
                            created.value.id = id.to_string();
                        }
                    }
                }
            }
        }
        if !matches!(created.status, StatusCode::CREATED | StatusCode::ACCEPTED)
            || (created.status == StatusCode::CREATED && created.value.status != "ready")
            || (created.status == StatusCode::ACCEPTED
                && !matches!(created.value.status.as_str(), "pending" | "deploying" | "probing"))
        {
            let error = contract_error(());
            return if let Ok(url) = connection_resource_url(&control_base, &created.value.id) {
                Err(error_after_release(error, &client, &url, control_credential.as_ref()).await)
            } else {
                Err(unidentified_create_error(error))
            };
        }
        let mut state = created.value;
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
                return Err(unidentified_create_error(error));
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

        loop {
            match state.status.as_str() {
                "ready" => break,
                "pending" | "deploying" | "probing" => {}
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

        let context_window = descriptor
            .context_window
            .unwrap_or(identity.profile.context_window);
        Ok(Self {
            client,
            control_base,
            control_credential,
            identity,
            audience,
            endpoint: descriptor.base_url,
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
}

fn unidentified_create_error(mut error: DynamicLanError) -> DynamicLanError {
    error.release_failure = Some(ErrorKind::Contract);
    error
}
