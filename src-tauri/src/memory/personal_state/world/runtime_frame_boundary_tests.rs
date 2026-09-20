#![cfg(test)]

//! M2-28: boundary checks. The M2A read path adds no DB table, writes no World
//! assertion, persists no external evidence and never opens a Context Broker
//! generation. Existing continuity/scope regression is covered by G1-G4.

use super::evidence_eligibility::assess_contextstill_v1;
use super::runtime_test_support::*;
use crate::meeting::MeetingState;

#[test]
fn m2_28_frame_reads_do_not_mutate_schema_or_world() {
    let fixture = Fixture::with_entities(&[("resource", MEETING_ID)], 2);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);

    let tables_before = fixture.table_names();
    let assertions_before = fixture.table_count("personal_assertions");
    let transitions_before = fixture.table_count("personal_transitions");
    let patches_before = fixture.table_count("personal_patches");
    let generations_before = fixture.table_count("context_generations");
    let sources_before = fixture.table_count("personal_sources");

    let service = fixture.service();
    for _ in 0..3 {
        let access = fixture.access();
        let request = fixture.request(
            access,
            vec![fixture.meeting_ref(MEETING_ID)],
            Some(fixture.graph_request("ent0")),
        );
        let prepared = service.prepare_frame(request).unwrap();
        let _ = service.revalidate_frame(&prepared).unwrap();
    }
    // Evidence assessment is a pure capability judgement; it persists nothing.
    let assessment = assess_contextstill_v1();
    assert_eq!(assessment.reasons.len(), 5);

    assert_eq!(fixture.table_names(), tables_before, "no new tables");
    assert_eq!(
        fixture.table_count("personal_assertions"),
        assertions_before
    );
    assert_eq!(
        fixture.table_count("personal_transitions"),
        transitions_before
    );
    assert_eq!(fixture.table_count("personal_patches"), patches_before);
    assert_eq!(
        fixture.table_count("context_generations"),
        generations_before
    );
    assert_eq!(fixture.table_count("personal_sources"), sources_before);
}

#[test]
fn m2_28_no_context_generation_or_broker_row_is_created() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    let _ = service.prepare_frame(request).unwrap();
    assert_eq!(fixture.table_count("context_generations"), 0);
    assert_eq!(fixture.table_count("personal_generations"), 0);
}

#[test]
fn m2_28_evidence_assessment_persists_nothing() {
    use super::evidence_eligibility::EvidenceEligibility;
    let fixture = Fixture::with_entities(&[], 1);
    let before = fixture.table_count("personal_assertions");
    let assessment = assess_contextstill_v1();
    assert_eq!(assessment.eligibility, EvidenceEligibility::TransientOnly);
    assert_eq!(fixture.table_count("personal_assertions"), before);
}
