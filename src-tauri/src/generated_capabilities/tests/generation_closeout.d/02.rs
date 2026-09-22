#[tokio::test]
async fn gc_06_epoch_change_during_update_conflicts() {
    let env = TestEnv::start(true);
    let request_a = registered("req-a", CANDIDATE_A, ACCEPTANCE_A, true, false);
    let request_b = registered("req-b", CANDIDATE_B, ACCEPTANCE_B, false, true);
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let fake_a = FakeGenerator::new(&request_a, FakeBody::EnabledAndNotSuspended);
    let generation_a = GenerationService::new(
        env.writer.clone(),
        env.service.clone(),
        vec![request_a.clone(), request_b.clone()],
        Arc::new(fake_a),
        Arc::new(SequencePackager::new(vec![
            candidate_dir(CANDIDATE_A),
            candidate_dir(CANDIDATE_B),
        ])),
        &env.data_directory,
    );
    let first = generation_a
        .generate(
            context("run-a", "msg-a"),
            GenerateInput {
                request_id: "req-a".into(),
                base_revision_id: None,
            },
            RunCancellation::default(),
        )
        .await;
    assert_eq!(first.status, GenerationStatus::Active, "{first:?}");
    let revision_a = first.revision_id.expect("revision A");
    let capability_id = capability_id(&env, &revision_a);
    let gated = GatedGenerator {
        inner: FakeGenerator::new(&request_b, FakeBody::EnabledOnly),
        started: started.clone(),
        release: release.clone(),
    };
    let generation_b = Arc::new(GenerationService::new(
        env.writer.clone(),
        env.service.clone(),
        vec![request_a, request_b],
        Arc::new(gated),
        Arc::new(SequencePackager::new(vec![candidate_dir(CANDIDATE_B)])),
        &env.data_directory,
    ));
    let task = {
        let generation_b = generation_b.clone();
        let revision_a = revision_a.clone();
        tokio::spawn(async move {
            generation_b
                .generate(
                    context("run-b", "msg-b"),
                    GenerateInput {
                        request_id: "req-b".into(),
                        base_revision_id: Some(revision_a),
                    },
                    RunCancellation::default(),
                )
                .await
        })
    };
    let wait = tokio::time::Instant::now();
    while !started.load(Ordering::SeqCst) {
        if wait.elapsed() > Duration::from_secs(2) {
            panic!("update generator did not start");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    env.writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE generated_capabilities SET catalog_epoch = catalog_epoch + 1 WHERE id = ?1",
                    rusqlite::params![capability_id],
                )
                .unwrap();
            Ok(())
        })
        .expect("bump epoch");
    release.store(true, Ordering::SeqCst);
    let second = task.await.expect("join");
    assert_eq!(second.status, GenerationStatus::Conflict, "{second:?}");
    assert_eq!(
        env.service
            .resolve_active(&capability_id)
            .expect("A still active")
            .revision_id,
        revision_a
    );
}
#[tokio::test]
async fn gc_06_other_principal_cannot_load_stored_inspection() {
    let env = TestEnv::start(true);
    let (_generation, _fake, revision_a) = generate_a(&env).await;
    invoke_on(&env, "call-a-10", &revision_a).await;
    bind_typescript(
        &env,
        &revision_a,
        "insp-a",
        "export const marker = \"rev-A\";\n",
    );
    assert_eq!(
        load_stored_inspection(&env.writer, &env.data_directory, "other-user", "call-a-10")
            .unwrap_err()
            .code,
        CapabilityErrorCode::NotActive
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gc_07_restore_a_suspended_revision_and_invoke_via_mcp() {
    use crate::generated_capabilities::publication_sync::{activate_and_publish, PublishRequest};
    use crate::tool_selection::backends::llang::LlangBackend;
    use crate::tool_selection::backends::{BackendRequest, TechnicalStatus, ToolBackend};

    let env = TestEnv::start(true);
    let (_generation, _fake, revision_a) = generate_a(&env).await;
    let capability_id = invoke_on(&env, "call-a-10", &revision_a).await;
    bind_typescript(
        &env,
        &revision_a,
        "insp-restore",
        "export const marker = \"rev-A\";\n",
    );
    assert!(catalog_enabled(&env) >= 1);

    // Stop: suspension unpublishes the catalog but keeps the stored evidence.
    let epoch = env.service.catalog_epoch(&capability_id).expect("epoch");
    env.service
        .suspend_revision(&revision_a, epoch)
        .expect("suspend");
    assert_eq!(catalog_enabled(&env), 0);

    // Restore: re-verify the suspended revision against the current runtime and acceptance, then
    // re-publish it with the same publication function.
    let epoch = env.service.catalog_epoch(&capability_id).expect("epoch");
    lifecycle::restore_revision(&env.writer, &revision_a, epoch).expect("reopen suspended");
    let summary = env
        .service
        .verify_candidate(&revision_a, ACCEPTANCE_A, &Cancellation::default())
        .await
        .expect("re-verify");
    assert!(summary.passed, "{summary:?}");
    let principal =
        crate::tool_selection::service::ensure_principal(&env.writer).expect("principal");
    let epoch = env.service.catalog_epoch(&capability_id).expect("epoch");
    lifecycle::transaction(&env.writer, |transaction| {
        activate_and_publish(
            transaction,
            PublishRequest {
                principal_id: &principal,
                revision_id: &revision_a,
                expected_epoch: epoch,
                job_id: None,
                grant_on_create: false,
                fail_at: None,
            },
        )
    })
    .expect("restore");
    assert!(catalog_enabled(&env) >= 1);

    // MCP: the restored revision is invoked through the MCP backend and its TypeScript is
    // retrievable from the same call id.
    let resolved = env.service.resolve_active(&capability_id).expect("active");
    let binding = json!({
        "capabilityId": resolved.capability_id,
        "revisionId": resolved.revision_id,
        "packageHash": resolved.package_hash,
        "contractHash": resolved.contract_hash,
        "catalogEpoch": resolved.catalog_epoch,
        "inputFields": resolved.input_fields(),
    });
    let request = BackendRequest {
        call_id: "call-mcp-restore".into(),
        tool_id: "tool-mcp-restore".into(),
        revision_id: resolved.revision_id.clone(),
        backend_key: "llang".into(),
        binding,
        arguments: json!({ "enabled": true, "suspended": false }),
        timeout: Duration::from_secs(5),
        origin: "mcp",
        actor: Some(actor()),
    };
    let outcome = LlangBackend::new(Some(env.service.clone()))
        .invoke(request, &RunCancellation::default())
        .await;
    assert_eq!(outcome.status, TechnicalStatus::Succeeded, "{outcome:?}");
    let stored = load_stored_inspection(
        &env.writer,
        &env.data_directory,
        "principal-flow",
        "call-mcp-restore",
    )
    .expect("mcp inspect");
    assert!(stored.typescript_text.contains("rev-A"));
}
/// C14 live lane. Requires `SAAA_LLANG_GENERATION_CONFIG` pointing at a trusted kit (see
/// `scripts/llang/build-generation-kit.ts`), a registered request catalog, and a real
/// conversation provider credential. Ignored by default; run explicitly:
/// `cargo test --lib c14_live -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live lane: trusted kit + registered requests + real provider credential required"]
async fn c14_live_generate_new_and_update_with_a_real_model() {
    let configured = crate::generated_capabilities::generation::config::from_environment()
        .expect("generation config");
    let Some((config, requests)) = configured else {
        panic!("SAAA_LLANG_GENERATION_CONFIG must be set, enabled and valid for the live lane");
    };
    let request = requests
        .first()
        .expect("at least one registered request")
        .clone();
    let env = TestEnv::start(true);
    let kit = Arc::new(
        crate::generated_capabilities::generation::kit::GenerationKit::load(&config)
            .expect("trusted kit"),
    );
    let generator = Arc::new(
        crate::generated_capabilities::generation::generator::ConversationProviderGenerator::new(
            env.writer.clone(),
        ),
    );
    let packager = Arc::new(
        crate::generated_capabilities::generation::packager::KitPackager::new(
            kit,
            &env.data_directory,
        ),
    );
    let generation = GenerationService::new(
        env.writer.clone(),
        env.service.clone(),
        requests.clone(),
        generator,
        packager,
        &env.data_directory,
    );
    let started = std::time::Instant::now();
    let first = generation
        .generate(
            context("run-live-a", "msg-live-a"),
            GenerateInput {
                request_id: request.id.clone(),
                base_revision_id: None,
            },
            RunCancellation::default(),
        )
        .await;
    eprintln!(
        "c14 live generation latency_ms={} receipt={first:?}",
        started.elapsed().as_millis()
    );
    assert_eq!(first.status, GenerationStatus::Active, "{first:?}");
    let revision_a = first.revision_id.clone().expect("revision A");
    if let Some(update) = requests.iter().find(|candidate| {
        candidate.id != request.id && candidate.capability_id == request.capability_id
    }) {
        let second = generation
            .generate(
                context("run-live-b", "msg-live-b"),
                GenerateInput {
                    request_id: update.id.clone(),
                    base_revision_id: Some(revision_a),
                },
                RunCancellation::default(),
            )
            .await;
        eprintln!("c14 live change generation receipt={second:?}");
        assert_eq!(second.status, GenerationStatus::Active, "{second:?}");
    }
}
