use super::*;

impl SituationRuntime {
    pub(crate) fn tick_sampled(
        &self,
        connection: &SqliteWriter,
        sample: SituationSample,
    ) -> Result<(), String> {
        let SituationSample {
            foreground,
            calendar,
            input_activity,
            observed_at: now,
            observed_ms: now_ms,
        } = sample;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        if !inner.settings.enabled {
            return Ok(());
        }

        let previous_health = signal_health(&inner.signals);
        let sequence = inner.signals.sequence.saturating_add(1);
        let owned_is_stale = inner.owned_updated_ms == 0
            || now_ms.saturating_sub(inner.owned_updated_ms)
                > u128::from(inner.settings.sample_interval_ms).saturating_mul(3);
        let owned = fresh_owned(
            &inner.owned,
            inner.owned_updated_ms,
            now_ms,
            inner.settings.sample_interval_ms,
        );
        let signals = SignalSnapshot {
            sequence,
            observed_at: now.clone(),
            foreground,
            conversation: ConversationSignal {
                state: owned.conversation_state,
            },
            microphone: MicrophoneSignal {
                state: owned.microphone_state,
                health: SignalHealth::Ready,
            },
            audio: AudioSignal {
                state: owned.audio_state,
                health: SignalHealth::Ready,
            },
            calendar,
            input_activity,
        };
        let next_health = signal_health(&signals);
        let mut pending_events = Vec::new();
        for item in &next_health {
            if previous_health
                .iter()
                .find(|previous| previous.source == item.source)
                .map(|previous| &previous.health)
                != Some(&item.health)
            {
                pending_events.push(SituationEvent::SignalHealthChanged {
                    source: item.source.clone(),
                    health: item.health,
                });
            }
        }

        let candidate = classify_with_parameters(&signals, &inner.calibration_parameters);
        if candidate.scene != inner.last_candidate_scene {
            let mut projected = inner.state.clone();
            projected.scene.clone_from(&candidate.scene);
            projected.confidence = candidate.confidence;
            projected.evidence.clone_from(&candidate.evidence);
            projected
                .user_attention
                .clone_from(&candidate.user_attention);
            projected
                .audio_environment
                .clone_from(&candidate.audio_environment);
            projected.candidate_since = now.clone();
            projected.updated_at = now.clone();
            pending_events.push(SituationEvent::CandidateChanged { state: projected });
        }
        let mut hysteresis = inner.hysteresis.clone();
        let (mut state, transitioned) = hysteresis.update_with_parameters(
            &candidate,
            &now,
            now_ms,
            &inner.calibration_parameters,
        );
        state
            .rule_version
            .clone_from(&inner.calibration_rule_version);
        let decision = shadow_policy(&state, &signals, &now, &inner.calibration_parameters);
        let decision_changed = decision.proposed_attention != inner.decision.proposed_attention;
        let mut quality = inner.quality.clone();
        let quality_started_ms = if quality.sample_count == 0 {
            now_ms
        } else {
            inner.quality_started_ms
        };
        accumulate_quality(
            &mut quality,
            candidate.scene != inner.last_candidate_scene,
            transitioned,
            state.scene == "UNKNOWN",
            owned_is_stale,
            &decision.proposed_attention,
            &next_health,
        );
        let quality_due = now_ms.saturating_sub(quality_started_ms)
            >= u128::from(inner.settings.heartbeat_interval_ms);
        let heartbeat_due = inner.last_persisted_ms == 0
            || now_ms.saturating_sub(inner.last_persisted_ms)
                >= u128::from(inner.settings.heartbeat_interval_ms);
        let entry_kind = if transitioned {
            Some("transition")
        } else if decision_changed {
            Some("decision")
        } else if heartbeat_due || quality_due {
            Some("heartbeat")
        } else {
            None
        };
        let entry = entry_kind.map(|entry_kind| SituationLedgerEntry {
            id: crate::new_id("situation"),
            observed_at: now.clone(),
            state: state.clone(),
            decision: decision.clone(),
            signal_health: next_health.clone(),
            entry_kind: entry_kind.to_string(),
            feedback: None,
        });
        if let Some(entry) = &entry {
            let quality_window = quality_due.then_some((
                quality_started_ms,
                inner.calibration_rule_version.as_str(),
                &quality,
            ));
            if let Err(error) = connection.write(|database| {
                repository::persist_entry_with_retention(
                    database,
                    entry,
                    &inner.settings,
                    now_ms,
                    quality_window,
                )
            }) {
                if now_ms.saturating_sub(quality_started_ms)
                    >= u128::from(inner.settings.heartbeat_interval_ms).saturating_mul(2)
                {
                    inner.quality = QualityWindowCounters::default();
                    inner.quality_started_ms = 0;
                } else {
                    inner.quality = quality;
                    inner.quality_started_ms = quality_started_ms;
                }
                return Err(error);
            }
        }

        inner.last_candidate_scene.clone_from(&candidate.scene);
        inner.hysteresis = hysteresis;
        inner.signals = signals;
        inner.state = state;
        inner.decision = decision;
        if quality_due {
            inner.quality = QualityWindowCounters::default();
            inner.quality_started_ms = 0;
        } else {
            inner.quality = quality;
            inner.quality_started_ms = quality_started_ms;
        }
        inner.last_failure = None;
        for event in pending_events {
            push_event(&mut inner, event);
        }
        if let Some(entry) = entry {
            inner.last_persisted_ms = now_ms;
            if transitioned {
                push_event(
                    &mut inner,
                    SituationEvent::StableStateChanged {
                        entry: entry.clone(),
                    },
                );
            }
            if decision_changed {
                push_event(&mut inner, SituationEvent::ShadowDecisionChanged { entry });
            }
        }
        Ok(())
    }

    #[allow(dead_code)] // Used by database-isolated runtime tests; app commands use snapshot_locked.
    pub fn snapshot(&self, connection: &Connection) -> Result<SituationSnapshot, String> {
        let (monitoring_enabled, signals, state, decision, last_failure) = self.snapshot_state()?;
        Ok(SituationSnapshot {
            monitoring_enabled,
            monitoring_active: self.is_worker_running(),
            signals,
            state,
            decision,
            last_failure,
            history: repository::list_history(connection)?,
            evaluation: repository::evaluation_summary(connection)?,
        })
    }

    pub fn snapshot_locked(&self, readers: &SqliteReaders) -> Result<SituationSnapshot, String> {
        let (monitoring_enabled, signals, state, decision, last_failure) = self.snapshot_state()?;
        readers.read(|database| {
            Ok(SituationSnapshot {
                monitoring_enabled,
                monitoring_active: self.is_worker_running(),
                signals,
                state,
                decision,
                last_failure,
                history: repository::list_history(database)?,
                evaluation: repository::evaluation_summary(database)?,
            })
        })
    }

    fn snapshot_state(
        &self,
    ) -> Result<
        (
            bool,
            SignalSnapshot,
            SituationState,
            ShadowDecision,
            Option<SituationRuntimeFailure>,
        ),
        String,
    > {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        Ok((
            inner.settings.enabled,
            inner.signals.clone(),
            inner.state.clone(),
            inner.decision.clone(),
            inner.last_failure.clone(),
        ))
    }

    #[cfg(test)]
    pub fn events_after(&self, revision: u64) -> Result<(u64, Vec<SituationEvent>), String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        let events = inner
            .events
            .iter()
            .filter(|(event_revision, _)| *event_revision > revision)
            .map(|(_, event)| event.clone())
            .collect();
        Ok((inner.next_revision.saturating_sub(1), events))
    }
}
