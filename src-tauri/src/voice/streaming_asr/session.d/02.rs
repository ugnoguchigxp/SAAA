fn failure_message(code: VoiceAsrFailureCode) -> &'static str {
    match code {
        VoiceAsrFailureCode::StreamProtocol => "ASR streaming protocol failed",
        VoiceAsrFailureCode::StreamTimeout => "ASR streaming timed out",
        VoiceAsrFailureCode::FinalTimeout => "ASR finalization timed out",
        VoiceAsrFailureCode::LanguageNotAllowed => "ASR returned a disallowed language",
        VoiceAsrFailureCode::TargetSpeakerUnavailable => {
            "Target-speaker verification is unavailable"
        }
        VoiceAsrFailureCode::Backpressure => "ASR input exceeded its bounded capacity",
        VoiceAsrFailureCode::Cancelled => "ASR was cancelled",
        _ => "ASR provider failed",
    }
}
#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde_json::Value;

    use super::*;
    use crate::voice::streaming_asr::speaker_gate_runtime::SpeakerScorer;

    struct FixedDecoder;
    #[async_trait]
    impl BatchDecode for FixedDecoder {
        async fn decode(
            &self,
            _pcm16le: Zeroizing<Vec<u8>>,
            _cancellation: Arc<RunCancellation>,
        ) -> Result<BatchDecodeOutcome, String> {
            Ok(BatchDecodeOutcome::Transcript {
                text: "hello".to_string(),
                language: Some("en".to_string()),
            })
        }
    }

    struct BlockingDecoder(Arc<tokio::sync::Notify>);
    #[async_trait]
    impl BatchDecode for BlockingDecoder {
        async fn decode(
            &self,
            _pcm16le: Zeroizing<Vec<u8>>,
            _cancellation: Arc<RunCancellation>,
        ) -> Result<BatchDecodeOutcome, String> {
            self.0.notified().await;
            Ok(BatchDecodeOutcome::Transcript {
                text: "hello".to_string(),
                language: Some("en".to_string()),
            })
        }
    }

    struct RecordingDecoder {
        seen: Arc<Mutex<Vec<Vec<u8>>>>,
    }
    #[async_trait]
    impl BatchDecode for RecordingDecoder {
        async fn decode(
            &self,
            pcm16le: Zeroizing<Vec<u8>>,
            _cancellation: Arc<RunCancellation>,
        ) -> Result<BatchDecodeOutcome, String> {
            self.seen.lock().unwrap().push(pcm16le.to_vec());
            Ok(BatchDecodeOutcome::NoSpeech)
        }
    }

    struct RejectScorer;
    impl SpeakerScorer for RejectScorer {
        fn score(&self, _samples_16k: Zeroizing<Vec<f32>>) -> Result<f32, String> {
            Ok(0.1)
        }
        fn threshold(&self) -> f32 {
            0.5
        }
    }

    struct PositiveVoiceScorer;
    impl SpeakerScorer for PositiveVoiceScorer {
        fn score(&self, samples_16k: Zeroizing<Vec<f32>>) -> Result<f32, String> {
            let mean = samples_16k.iter().sum::<f32>() / samples_16k.len() as f32;
            Ok(if mean > 0.01 { 0.9 } else { 0.1 })
        }
        fn threshold(&self) -> f32 {
            0.5
        }
    }

    fn event_channel() -> (Channel<VoiceAsrStreamEvent>, Arc<Mutex<Vec<Value>>>) {
        let values = Arc::new(Mutex::new(Vec::new()));
        let captured = values.clone();
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                captured
                    .lock()
                    .expect("capture lock")
                    .push(serde_json::from_str(&json).expect("event json"));
            }
            Ok(())
        });
        (channel, values)
    }

    fn failing_event_channel() -> Channel<VoiceAsrStreamEvent> {
        Channel::new(|_| {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "channel closed").into())
        })
    }

    fn config(event: Channel<VoiceAsrStreamEvent>) -> SessionConfig {
        SessionConfig {
            session_id: "session_test".to_string(),
            current_utterance_id: "utterance_test".to_string(),
            event: VoiceAsrAuditChannel::plain(event),
            batch_decoder: Arc::new(FixedDecoder),
            speaker_gate: SpeakerGate::new(None, 0.008),
            cancellation: Arc::new(RunCancellation::default()),
        }
    }

    #[tokio::test]
    async fn batch_commit_emits_final_before_stopped() {
        let (event, captured) = event_channel();
        let (command_tx, command_rx) = mpsc::channel(8);
        let task = tokio::spawn(run(config(event), command_rx));
        command_tx
            .send(SessionCommand::Audio(Zeroizing::new(vec![1; 3_200])))
            .await
            .unwrap();
        let (accepted_tx, accepted_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Commit {
                reason: CommitReason::Silence,
                accepted: accepted_tx,
            })
            .await
            .unwrap();
        accepted_rx.await.unwrap().unwrap();
        let (completed_tx, completed_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Stop {
                finalize_current: true,
                accepted: completed_tx,
            })
            .await
            .unwrap();
        completed_rx.await.unwrap().unwrap();
        task.await.unwrap();
        let types = captured
            .lock()
            .unwrap()
            .iter()
            .map(|value| value["type"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(types, vec!["final", "stopped"]);
    }

    #[tokio::test]
    async fn stop_acknowledges_before_asynchronous_finalization_finishes() {
        let (event, captured) = event_channel();
        let release = Arc::new(tokio::sync::Notify::new());
        let mut config = config(event);
        config.batch_decoder = Arc::new(BlockingDecoder(release.clone()));
        let (command_tx, command_rx) = mpsc::channel(8);
        let task = tokio::spawn(run(config, command_rx));
        command_tx
            .send(SessionCommand::Audio(Zeroizing::new(vec![1; 3_200])))
            .await
            .unwrap();
        let (accepted_tx, accepted_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Stop {
                finalize_current: true,
                accepted: accepted_tx,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_millis(100), accepted_rx)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!task.is_finished());
        release.notify_one();
        task.await.unwrap();
        let types = captured
            .lock()
            .unwrap()
            .iter()
            .map(|value| value["type"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(types, vec!["final", "stopped"]);
    }

    #[tokio::test]
    async fn event_channel_failure_cancels_the_session() {
        let config = config(failing_event_channel());
        let cancellation = config.cancellation.clone();
        let (command_tx, command_rx) = mpsc::channel(2);
        command_tx
            .send(SessionCommand::Audio(Zeroizing::new(vec![1; 3_200])))
            .await
            .unwrap();
        let (accepted, _) = oneshot::channel();
        command_tx
            .send(SessionCommand::Commit {
                reason: CommitReason::Silence,
                accepted,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), run(config, command_rx))
            .await
            .unwrap();
        assert!(cancellation.is_cancelled());
    }

    async fn run_pcm_fixture(gate: SpeakerGate, packet_count: usize) -> Vec<Vec<u8>> {
        let (event, _) = event_channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let config = SessionConfig {
            session_id: "session_pcm".to_string(),
            current_utterance_id: "utterance_pcm".to_string(),
            event: VoiceAsrAuditChannel::plain(event),
            batch_decoder: Arc::new(RecordingDecoder { seen: seen.clone() }),
            speaker_gate: gate,
            cancellation: Arc::new(RunCancellation::default()),
        };
        let (command_tx, command_rx) = mpsc::channel(32);
        let task = tokio::spawn(run(config, command_rx));
        let packet = 5_000_i16.to_le_bytes().repeat(1_600);
        for _ in 0..packet_count {
            command_tx
                .send(SessionCommand::Audio(Zeroizing::new(packet.clone())))
                .await
                .unwrap();
        }
        let (accepted_tx, accepted_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Commit {
                reason: CommitReason::Silence,
                accepted: accepted_tx,
            })
            .await
            .unwrap();
        accepted_rx.await.unwrap().unwrap();
        let (completed_tx, completed_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Stop {
                finalize_current: true,
                accepted: completed_tx,
            })
            .await
            .unwrap();
        completed_rx.await.unwrap().unwrap();
        task.await.unwrap();
        Arc::try_unwrap(seen).unwrap().into_inner().unwrap()
    }

    #[tokio::test]
    async fn all_speakers_preserves_pcm_and_target_rejection_never_leaks_raw_pattern() {
        let all = run_pcm_fixture(SpeakerGate::new(None, 0.001), 5).await;
        assert_eq!(all.len(), 1);
        let expected = 5_000_i16.to_le_bytes();
        assert!(all[0].chunks_exact(2).all(|sample| sample == expected));

        let target =
            run_pcm_fixture(SpeakerGate::new(Some(Arc::new(RejectScorer)), 0.001), 15).await;
        assert_eq!(target.len(), 1);
        assert!(target[0].iter().all(|byte| *byte == 0));
    }

    #[tokio::test]
    async fn target_silence_commits_despite_other_audible_speaker() {
        let (event, captured) = event_channel();
        let mut config = config(event);
        config.speaker_gate = SpeakerGate::new(Some(Arc::new(PositiveVoiceScorer)), 0.001);
        let (commands, receiver) = mpsc::channel(64);
        let task = tokio::spawn(run(config, receiver));
        let other = (-5_000_i16).to_le_bytes().repeat(1_600);
        let target = 5_000_i16.to_le_bytes().repeat(1_600);
        for _ in 0..20 {
            commands
                .send(SessionCommand::Audio(Zeroizing::new(other.clone())))
                .await
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!captured
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "final"));
        for _ in 0..20 {
            commands
                .send(SessionCommand::Audio(Zeroizing::new(target.clone())))
                .await
                .unwrap();
        }
        for _ in 0..24 {
            commands
                .send(SessionCommand::Audio(Zeroizing::new(other.clone())))
                .await
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!captured
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "final"));
        commands
            .send(SessionCommand::Audio(Zeroizing::new(other.clone())))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if captured
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| event["type"] == "final")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("target utterance should finalize without a frontend commit");
        let (accepted, received) = oneshot::channel();
        commands
            .send(SessionCommand::Stop {
                finalize_current: false,
                accepted,
            })
            .await
            .unwrap();
        received.await.unwrap().unwrap();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn resumed_target_before_timeout_keeps_one_utterance_open() {
        let (event, captured) = event_channel();
        let mut config = config(event);
        config.speaker_gate = SpeakerGate::new(Some(Arc::new(PositiveVoiceScorer)), 0.001);
        let (commands, receiver) = mpsc::channel(64);
        let task = tokio::spawn(run(config, receiver));
        for (count, sample) in [(20, 5_000_i16), (10, -5_000), (20, 5_000)] {
            let packet = sample.to_le_bytes().repeat(1_600);
            for _ in 0..count {
                commands
                    .send(SessionCommand::Audio(Zeroizing::new(packet.clone())))
                    .await
                    .unwrap();
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!captured
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "final"));
        let (accepted, received) = oneshot::channel();
        commands
            .send(SessionCommand::Stop {
                finalize_current: true,
                accepted,
            })
            .await
            .unwrap();
        received.await.unwrap().unwrap();
        task.await.unwrap();
        assert_eq!(
            captured
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event["type"] == "final")
                .count(),
            1
        );
    }

    #[test]
    fn three_consecutive_batch_failures_are_fatal() {
        assert_eq!(next_failure(0), (1, false));
        assert_eq!(next_failure(1), (2, false));
        assert_eq!(next_failure(2), (3, true));
    }

    #[test]
    fn completed_decode_cancellations_do_not_accumulate_for_the_session_lifetime() {
        let completed = Arc::new(RunCancellation::default());
        let mut tracked = vec![Arc::downgrade(&completed)];
        drop(completed);
        let active = Arc::new(RunCancellation::default());

        track_decode_cancellation(&mut tracked, &active);
        track_decode_cancellation(&mut tracked, &active);

        assert_eq!(tracked.len(), 1);
        assert!(tracked[0].upgrade().is_some());
    }
}
