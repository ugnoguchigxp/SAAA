//! Side-effect-free Jarvis intake shadow: only sanitized scheduling metadata is audited.
use super::{record_event, AuditAttributeValue, FrontendAuditEventInput, SqliteWriter};
use crate::runtime::voice_frontend::{Dispatcher, Intake, Update};
use crate::voice::streaming_asr::contracts::VoiceAsrStreamEvent;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

enum Signal {
    Asr(VoiceAsrStreamEvent),
    Boundary,
}

type Sender = mpsc::Sender<Signal>;
struct ObservationSender {
    sender: Sender,
    dropped: Arc<AtomicU64>,
}
impl ObservationSender {
    fn send(&self, signal: Signal) {
        if self.sender.try_send(signal).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
type Registry = Mutex<HashMap<String, Weak<ObservationSender>>>;
static REGISTRY: OnceLock<Registry> = OnceLock::new();

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) struct ObservationHandle {
    sender: Arc<ObservationSender>,
}

impl ObservationHandle {
    pub(super) fn new(writer: Arc<SqliteWriter>, conversation_id: String) -> Self {
        let (sender, receiver) = mpsc::channel(64);
        let dropped = Arc::new(AtomicU64::new(0));
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(run(receiver, writer, conversation_id, dropped.clone()));
        }
        Self {
            sender: Arc::new(ObservationSender { sender, dropped }),
        }
    }

    pub(super) fn observe(&self, event: &VoiceAsrStreamEvent) {
        if let VoiceAsrStreamEvent::Ready { session_id, .. } = event {
            registry()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(session_id.clone(), Arc::downgrade(&self.sender));
        }
        self.sender.send(Signal::Asr(event.clone()));
    }
}

pub(super) fn observe_boundary(session_id: &str) {
    let sender = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(session_id)
        .and_then(Weak::upgrade);
    if let Some(sender) = sender {
        sender.send(Signal::Boundary);
    }
}

async fn run(
    mut receiver: mpsc::Receiver<Signal>,
    writer: Arc<SqliteWriter>,
    conversation_id: String,
    dropped: Arc<AtomicU64>,
) {
    let started = Instant::now();
    let mut dispatcher = Dispatcher::default();
    let mut session_id: Option<String> = None;
    loop {
        let until_tick = dispatcher
            .next_tick_ms()
            .map(|due| due.saturating_sub(elapsed_ms(started)))
            .unwrap_or(3_600_000);
        tokio::select! {
            signal = receiver.recv() => match signal {
                Some(Signal::Asr(VoiceAsrStreamEvent::Ready { session_id: id, .. })) => {
                    session_id = Some(id.clone());
                    dispatcher.start_session(id, conversation_id.clone(), elapsed_ms(started));
                }
                Some(Signal::Asr(VoiceAsrStreamEvent::Partial { session_id: id, utterance_id, revision, stable_text, unstable_text, .. })) => {
                    let update = Update {
                        session_id: id.clone(), conversation_id: conversation_id.clone(), utterance_id,
                        asr_revision: revision, text: format!("{stable_text}{unstable_text}"),
                        final_result: false, stop_candidate: false,
                    };
                    let intake = dispatcher.update(update, elapsed_ms(started));
                    if intake == Intake::Full { record_full(&writer, &conversation_id, &id); }
                }
                Some(Signal::Asr(VoiceAsrStreamEvent::Final { session_id: id, utterance_id, revision, text, .. })) => {
                    let update = Update {
                        session_id: id.clone(), conversation_id: conversation_id.clone(), utterance_id,
                        asr_revision: revision, text, final_result: true, stop_candidate: false,
                    };
                    let intake = dispatcher.update(update, elapsed_ms(started));
                    if intake == Intake::Full { record_full(&writer, &conversation_id, &id); }
                }
                Some(Signal::Asr(VoiceAsrStreamEvent::UtteranceDiscarded { utterance_id, .. })) => {
                    dispatcher.discard(&utterance_id);
                }
                Some(Signal::Asr(VoiceAsrStreamEvent::Stopped { .. })) => break,
                Some(Signal::Asr(VoiceAsrStreamEvent::Failed { fatal: true, .. })) => break,
                Some(Signal::Asr(_)) => {},
                Some(Signal::Boundary) => dispatcher.boundary_latest(),
                None => break,
            },
            _ = tokio::time::sleep(Duration::from_millis(until_tick)) => dispatcher.tick(elapsed_ms(started)),
        }
        while let Some(update) = dispatcher.take_ready() {
            record_dispatch(&writer, &update, dispatcher.pending_count());
            dispatcher.finish(); // The shadow sink is deliberately instantaneous.
        }
        let missed = dropped.swap(0, Ordering::Relaxed);
        if missed > 0 {
            if let Some(id) = &session_id {
                record_ingress_full(&writer, &conversation_id, id, missed);
            }
        }
    }
    dispatcher.stop_session();
    if let Some(id) = session_id {
        registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&id);
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn record_dispatch(writer: &SqliteWriter, update: &Update, pending_count: usize) {
    let attributes = BTreeMap::from([
        (
            "asrRevision".into(),
            AuditAttributeValue::Integer(update.asr_revision),
        ),
        (
            "finalResult".into(),
            AuditAttributeValue::Boolean(update.final_result),
        ),
        (
            "pendingCount".into(),
            AuditAttributeValue::Integer(pending_count as u64),
        ),
    ]);
    record(
        writer,
        "jarvis-observed-dispatch",
        &update.session_id,
        &update.conversation_id,
        Some(&update.utterance_id),
        None,
        attributes,
    );
}

fn record_full(writer: &SqliteWriter, conversation_id: &str, session_id: &str) {
    record(
        writer,
        "jarvis-observed-full",
        session_id,
        conversation_id,
        None,
        Some("blocked"),
        BTreeMap::new(),
    );
}

fn record_ingress_full(
    writer: &SqliteWriter,
    conversation_id: &str,
    session_id: &str,
    missed: u64,
) {
    record(
        writer,
        "jarvis-observed-ingress-full",
        session_id,
        conversation_id,
        None,
        Some("blocked"),
        BTreeMap::from([("droppedCount".into(), AuditAttributeValue::Integer(missed))]),
    );
}

fn record(
    writer: &SqliteWriter,
    event_name: &str,
    session_id: &str,
    conversation_id: &str,
    subject_id: Option<&str>,
    outcome: Option<&str>,
    attributes: BTreeMap<String, AuditAttributeValue>,
) {
    let input = FrontendAuditEventInput {
        component: "voice-asr".into(),
        event_name: event_name.into(),
        phase: "progress".into(),
        outcome: outcome.map(str::to_string),
        correlation_id: Some(session_id.into()),
        causation_id: None,
        conversation_id: Some(conversation_id.into()),
        runtime_run_id: None,
        session_id: Some(session_id.into()),
        subject_id: subject_id.map(str::to_string),
        failure_code: None,
        attributes,
    };
    let _ = writer.write(|connection| record_event::record_event(connection, &input));
}
