//! Operation-level intent over the full persisted user source.
use super::contracts::Operation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntentDecision {
    Allowed,
    Denied,
    Ambiguous,
    NotRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationIntent {
    pub operation: Operation,
    pub decision: IntentDecision,
    pub reason: &'static str,
}

pub(crate) fn classify_source(full_text: &str, operations: &[Operation]) -> Vec<OperationIntent> {
    operations
        .iter()
        .copied()
        .map(|operation| classify_operation(full_text, operation))
        .collect()
}

pub(crate) fn overall(intents: &[OperationIntent]) -> IntentDecision {
    if intents
        .iter()
        .any(|item| item.decision == IntentDecision::Denied)
    {
        return IntentDecision::Denied;
    }
    if intents
        .iter()
        .any(|item| item.decision == IntentDecision::Ambiguous)
    {
        return IntentDecision::Ambiguous;
    }
    if intents
        .iter()
        .all(|item| item.decision == IntentDecision::NotRequest)
    {
        return IntentDecision::NotRequest;
    }
    IntentDecision::Allowed
}

pub(crate) fn reason_for(decision: IntentDecision) -> &'static str {
    match decision {
        IntentDecision::Denied => "quoted_or_negative_source",
        IntentDecision::Ambiguous => "ambiguous_request",
        IntentDecision::NotRequest => "not_a_request",
        IntentDecision::Allowed => "allowed_request",
    }
}

fn classify_operation(full_text: &str, operation: Operation) -> OperationIntent {
    let lower = full_text.to_lowercase();
    if explanation_of_quoted_command(full_text, &lower) {
        return OperationIntent {
            operation,
            decision: IntentDecision::NotRequest,
            reason: "quoted_explanation",
        };
    }
    if double_negative_hedge(&lower) {
        return OperationIntent {
            operation,
            decision: IntentDecision::Ambiguous,
            reason: "hedged_negation",
        };
    }
    if hypothetical(&lower) && !explicit_request(&lower, operation) {
        return OperationIntent {
            operation,
            decision: IntentDecision::NotRequest,
            reason: "hypothetical",
        };
    }
    if denies(operation, full_text, &lower) {
        return OperationIntent {
            operation,
            decision: IntentDecision::Denied,
            reason: "explicit_prohibition",
        };
    }
    if operation == Operation::Read
        && denies(Operation::TestRun, full_text, &lower)
        && !explicit_request(&lower, Operation::Read)
    {
        return OperationIntent {
            operation,
            decision: IntentDecision::Denied,
            reason: "explicit_prohibition",
        };
    }
    if explicit_request(&lower, operation) {
        return OperationIntent {
            operation,
            decision: IntentDecision::Allowed,
            reason: "explicit_request",
        };
    }
    if ambiguous_only(&lower) {
        return OperationIntent {
            operation,
            decision: IntentDecision::Ambiguous,
            reason: "underspecified",
        };
    }
    OperationIntent {
        operation,
        decision: IntentDecision::NotRequest,
        reason: "not_addressed",
    }
}

fn explanation_of_quoted_command(text: &str, lower: &str) -> bool {
    let quoted_command = ["「実行して」", "『実行して』", "\"run this\"", "\"実行して\""]
        .iter()
        .any(|marker| text.contains(marker));
    quoted_command
        && ["説明して", "explain", "どういう", "means"]
            .iter()
            .any(|marker| lower.contains(marker))
}

fn double_negative_hedge(lower: &str) -> bool {
    lower.contains("とは言っていない")
        || lower.contains("とはいっていない")
        || lower.contains("did not say not")
        || lower.contains("didn't say not")
}

fn hypothetical(lower: &str) -> bool {
    ["もし", "仮に", "suppose", "if we", "hypothetically"]
        .iter()
        .any(|marker| lower.contains(marker))
}

fn ambiguous_only(lower: &str) -> bool {
    matches!(
        lower.trim(),
        "それをお願い" | "お願い" | "do it" | "それ" | "あの件" | "任せる"
    )
}

fn denies(operation: Operation, text: &str, lower: &str) -> bool {
    let test_denied = contains_any(
        lower,
        &[
            "テストを実行しないで",
            "テストしないで",
            "実行しないで",
            "走らせないで",
            "don't run tests",
            "do not run tests",
            "do not run the tests",
            "never run tests",
        ],
    );
    let read_denied = contains_any(
        lower,
        &["読まないで", "don't read", "do not read", "読むな"],
    );
    match operation {
        Operation::Read => read_denied,
        Operation::TestRun => test_denied && !read_only_override(text, lower),
    }
}

fn read_only_override(text: &str, lower: &str) -> bool {
    contains_any(lower, &["読んで", "read the test", "テスト結果を読"])
        && contains_any(text, &["変更しないで", "直さないで", "do not change"])
}

fn explicit_request(lower: &str, operation: Operation) -> bool {
    match operation {
        Operation::Read => contains_any(
            lower,
            &[
                "読んで",
                "調べて",
                "確認して",
                "結果を読",
                "調査",
                "read",
                "inspect",
                "look at",
            ],
        ),
        Operation::TestRun => {
            contains_any(
                lower,
                &[
                    "テストして",
                    "テストを実行",
                    "run tests",
                    "run the tests",
                ],
            ) && !contains_any(lower, &["しないで", "do not", "don't"])
        }
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(read: bool, test: bool) -> Vec<Operation> {
        let mut items = Vec::new();
        if read {
            items.push(Operation::Read);
        }
        if test {
            items.push(Operation::TestRun);
        }
        items
    }

    #[test]
    fn rf5_n_02_prohibition_read_mix_quote_and_hedge() {
        let denied = classify_source("テストを実行しないで", &ops(false, true));
        assert_eq!(denied[0].decision, IntentDecision::Denied);
        let english = classify_source("do not run tests", &ops(false, true));
        assert_eq!(english[0].decision, IntentDecision::Denied);
        let mixed = classify_source("変更しないで、テスト結果を読んで", &ops(true, true));
        assert_eq!(mixed[0].decision, IntentDecision::Allowed);
        assert_eq!(mixed[1].decision, IntentDecision::NotRequest);
        let explained = classify_source("『実行して』という文を説明して", &ops(true, true));
        assert!(explained
            .iter()
            .all(|item| item.decision == IntentDecision::NotRequest));
        let hedge = classify_source("実行しないでとは言っていない", &ops(false, true));
        assert_eq!(hedge[0].decision, IntentDecision::Ambiguous);
        let allowed = classify_source("失敗テストをテストして", &ops(false, true));
        assert_eq!(allowed[0].decision, IntentDecision::Allowed);
        let delegated = classify_source("任せる", &ops(true, true));
        assert!(delegated
            .iter()
            .all(|item| item.decision == IntentDecision::Ambiguous));
        let inspect = classify_source("新しい調査を任せる", &ops(true, false));
        assert_eq!(inspect[0].decision, IntentDecision::Allowed);
    }
}
