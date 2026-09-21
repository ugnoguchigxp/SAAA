//! Metadata-only World observation. Never holds a database lock.
use super::{signal_health, speech::holds_speech, SituationRuntime};
use saaa_personal_state_core::world::frame_sources::{
    WorldSourceAvailability as Availability, WorldSourceEntry, WorldSourceKind, WorldSourcePayload,
};
use saaa_personal_state_core::world::runtime_frame::hex_sha256;

impl SituationRuntime {
    pub(crate) fn world_snapshot(&self, scope: &str, now: i64) -> Result<WorldSourceEntry, String> {
        let mut inner = self.inner.lock().map_err(|_| "situation-unavailable")?;
        let observed = chrono::DateTime::parse_from_rfc3339(&inner.signals.observed_at)
            .map(|time| time.timestamp_millis())
            .unwrap_or(0);
        let health =
            serde_json::to_string(&signal_health(&inner.signals)).map_err(|e| e.to_string())?;
        let fresh = inner.signals.sequence > 0
            && now >= observed
            && now.saturating_sub(observed)
                <= inner
                    .settings
                    .sample_interval_ms
                    .saturating_mul(3)
                    .max(1000) as i64;
        let (availability, reason) = if !inner.settings.enabled {
            (
                Availability::Unavailable,
                Some("monitor-disabled".to_string()),
            )
        } else if !fresh {
            (Availability::Stale, Some("observation-stale".to_string()))
        } else if inner.last_failure.is_some() {
            (
                Availability::Unavailable,
                Some("monitor-failed".to_string()),
            )
        } else {
            (Availability::Available, None)
        };
        let held = holds_speech(&inner.state.scene, &inner.decision.proposed_attention);
        update_version(&mut inner)?;
        let digest = hex_sha256(
            serde_json::to_vec(&(availability, &inner.world_digest))
                .map_err(|e| e.to_string())?
                .as_slice(),
        );
        let payload =
            (availability == Availability::Available).then(|| WorldSourcePayload::Situation {
                scene: Some(inner.state.scene.clone()),
                attention: Some(inner.decision.proposed_attention.clone()),
                hold: Some(if held { "held" } else { "clear" }.into()),
                signal_health: health,
                sequence: inner.world_sequence,
                monitor_enabled: inner.settings.enabled,
                inferred: true,
            });
        Ok(WorldSourceEntry {
            kind: WorldSourceKind::Situation,
            source_id: "situation".into(),
            owner_scope_key: scope.into(),
            availability,
            observed_at_ms: observed.max(0),
            as_of_ms: observed.max(0),
            version: Some(inner.world_sequence.to_string()),
            digest,
            payload,
            reason_code: reason,
        })
    }
}

pub(super) fn update_version(inner: &mut super::RuntimeInner) -> Result<(), String> {
    let meaning = serde_json::json!([
        inner.settings.enabled,
        inner.state.scene,
        inner.decision.proposed_attention,
        signal_health(&inner.signals),
        inner.last_failure.is_some()
    ]);
    let digest = hex_sha256(
        serde_json::to_vec(&meaning)
            .map_err(|e| e.to_string())?
            .as_slice(),
    );
    if inner.world_digest != digest {
        inner.world_sequence = inner.world_sequence.saturating_add(1);
        inner.world_digest = digest;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wr_t03_semantic_changes_and_staleness_are_explicit() {
        let runtime = SituationRuntime::new(Default::default(), None).unwrap();
        {
            let mut inner = runtime.inner.lock().unwrap();
            inner.settings.enabled = true;
            inner.signals.sequence = 1;
            inner.signals.observed_at = "1970-01-01T00:00:01.000Z".into();
            inner.state.scene = "CODING".into();
        }
        let a = runtime.world_snapshot("user:p", 1000).unwrap();
        assert_eq!(a.availability, Availability::Available);
        let b = runtime.world_snapshot("user:p", 1001).unwrap();
        assert_eq!(a.version, b.version);
        {
            let mut inner = runtime.inner.lock().unwrap();
            inner.state.scene = "MEETING".into();
            inner.decision.proposed_attention = "OBSERVE".into();
        }
        let changed = runtime.world_snapshot("user:p", 1001).unwrap();
        assert_ne!(a.version, changed.version);
        assert!(matches!(
            changed.payload,
            Some(WorldSourcePayload::Situation { inferred: true, .. })
        ));
        let stale = runtime.world_snapshot("user:p", 100000).unwrap();
        assert_eq!(stale.availability, Availability::Stale);
        assert!(stale.payload.is_none());
        assert_eq!(
            runtime.world_snapshot("user:p", 999).unwrap().availability,
            Availability::Stale
        );
    }
}
