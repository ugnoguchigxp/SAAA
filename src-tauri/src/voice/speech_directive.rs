use crate::CloudTtsProviderSettings;

const MARKER_LIMIT: usize = 32;
const MARKERS: &[(&str, SpeechExpression)] = &[
    ("[$natural]", SpeechExpression::Natural),
    ("[$bright]", SpeechExpression::Bright),
    ("[$gentle]", SpeechExpression::Gentle),
    ("[$serious]", SpeechExpression::Serious),
    ("[$excited]", SpeechExpression::Excited),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SpeechExpression {
    #[default]
    Natural,
    Bright,
    Gentle,
    Serious,
    Excited,
}

impl SpeechExpression {
    #[allow(dead_code)]
    pub(crate) fn audit_value(self) -> &'static str {
        match self {
            Self::Natural => "natural",
            Self::Bright => "bright",
            Self::Gentle => "gentle",
            Self::Serious => "serious",
            Self::Excited => "excited",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectionOutput {
    pub(crate) visible: String,
    pub(crate) decided: Option<SpeechExpression>,
}

#[derive(Debug, Clone)]
enum Phase {
    Leading(String),
    Body {
        expression: SpeechExpression,
        candidate: Option<String>,
        skip_separator: bool,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct SpeechDirectiveProjection {
    phase: Phase,
}

impl Default for SpeechDirectiveProjection {
    fn default() -> Self {
        Self {
            phase: Phase::Leading(String::new()),
        }
    }
}

impl SpeechDirectiveProjection {
    pub(crate) fn push(&mut self, delta: &str) -> ProjectionOutput {
        let mut visible = String::new();
        let mut decided = None;
        for ch in delta.chars() {
            let step = self.push_char(ch);
            visible.push_str(&step.visible);
            if step.decided.is_some() {
                decided = step.decided;
            }
        }
        ProjectionOutput { visible, decided }
    }

    pub(crate) fn finish(&mut self) -> ProjectionOutput {
        match &self.phase {
            Phase::Leading(buf) => {
                let visible = if buf.starts_with("[$") {
                    String::new()
                } else {
                    buf.clone()
                };
                self.phase = Phase::Body {
                    expression: SpeechExpression::Natural,
                    candidate: None,
                    skip_separator: false,
                };
                ProjectionOutput {
                    visible,
                    decided: Some(SpeechExpression::Natural),
                }
            }
            Phase::Body {
                candidate: Some(_), ..
            } => {
                if let Phase::Body { candidate, .. } = &mut self.phase {
                    *candidate = None;
                }
                ProjectionOutput {
                    visible: String::new(),
                    decided: None,
                }
            }
            Phase::Body { .. } => ProjectionOutput {
                visible: String::new(),
                decided: None,
            },
        }
    }

    pub(crate) fn expression(&self) -> Option<SpeechExpression> {
        match self.phase {
            Phase::Body { expression, .. } => Some(expression),
            Phase::Leading(_) => None,
        }
    }

    fn push_char(&mut self, ch: char) -> ProjectionOutput {
        match &mut self.phase {
            Phase::Leading(buf) => {
                buf.push(ch);
                if !buf.starts_with('[') || (buf.len() > 1 && !buf.starts_with("[$")) {
                    let visible = std::mem::take(buf);
                    self.phase = Phase::Body {
                        expression: SpeechExpression::Natural,
                        candidate: None,
                        skip_separator: false,
                    };
                    return ProjectionOutput {
                        visible,
                        decided: Some(SpeechExpression::Natural),
                    };
                }
                if buf.len() >= MARKER_LIMIT && !buf.contains(']') {
                    self.phase = Phase::Body {
                        expression: SpeechExpression::Natural,
                        candidate: None,
                        skip_separator: false,
                    };
                    return ProjectionOutput {
                        visible: String::new(),
                        decided: Some(SpeechExpression::Natural),
                    };
                }
                if let Some(end) = buf.find(']') {
                    let marker = &buf[..=end];
                    let expression = MARKERS
                        .iter()
                        .find(|(text, _)| *text == marker)
                        .map(|(_, expression)| *expression)
                        .unwrap_or(SpeechExpression::Natural);
                    let rest = &buf[end + 1..];
                    let visible = if rest.starts_with(['\n', ' ']) {
                        rest.chars().skip(1).collect()
                    } else {
                        rest.to_string()
                    };
                    let skip_separator = rest.is_empty();
                    self.phase = Phase::Body {
                        expression,
                        candidate: None,
                        skip_separator,
                    };
                    return ProjectionOutput {
                        visible,
                        decided: Some(expression),
                    };
                }
                ProjectionOutput {
                    visible: String::new(),
                    decided: None,
                }
            }
            Phase::Body {
                candidate,
                skip_separator,
                ..
            } => {
                if *skip_separator && candidate.is_none() {
                    *skip_separator = false;
                    if ch == '\n' || ch == ' ' {
                        return ProjectionOutput {
                            visible: String::new(),
                            decided: None,
                        };
                    }
                }
                if let Some(held) = candidate {
                    held.push(ch);
                    if held == "[" || (held.starts_with('[') && !held.starts_with("[$")) {
                        let visible = std::mem::take(held);
                        *candidate = None;
                        return ProjectionOutput {
                            visible,
                            decided: None,
                        };
                    }
                    if held.contains(']') || held.len() >= MARKER_LIMIT {
                        *candidate = None;
                    }
                    return ProjectionOutput {
                        visible: String::new(),
                        decided: None,
                    };
                }
                if ch == '[' {
                    *candidate = Some("[".into());
                    return ProjectionOutput {
                        visible: String::new(),
                        decided: None,
                    };
                }
                ProjectionOutput {
                    visible: ch.to_string(),
                    decided: None,
                }
            }
        }
    }
}

pub(crate) fn project_complete_assistant_content(text: &str) -> (String, SpeechExpression) {
    let mut parser = SpeechDirectiveProjection::default();
    let mut visible = parser.push(text).visible;
    visible.push_str(&parser.finish().visible);
    (
        visible,
        parser.expression().unwrap_or(SpeechExpression::Natural),
    )
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.clamp(min, max)
}

pub(crate) fn apply_expression(
    provider: &CloudTtsProviderSettings,
    expression: SpeechExpression,
) -> CloudTtsProviderSettings {
    let mut next = provider.clone();
    if provider.model != "voicevox-core" || expression == SpeechExpression::Natural {
        return next;
    }
    let (speed_factor, pitch_delta, intonation_factor) = match expression {
        SpeechExpression::Natural => (1.0, 0.0, 1.0),
        SpeechExpression::Bright => (1.05, 0.02, 1.15),
        SpeechExpression::Gentle => (0.92, -0.01, 0.90),
        SpeechExpression::Serious => (0.95, -0.02, 0.90),
        SpeechExpression::Excited => (1.12, 0.03, 1.30),
    };
    next.speed = Some(clamp(
        provider.speed.unwrap_or(1.0) * speed_factor,
        0.5,
        2.0,
    ));
    next.pitch_scale = Some(clamp(
        provider.pitch_scale.unwrap_or(0.0) + pitch_delta,
        -0.15,
        0.15,
    ));
    next.intonation_scale = Some(clamp(
        provider.intonation_scale.unwrap_or(1.0) * intonation_factor,
        0.0,
        2.0,
    ));
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_splits(text: &str) -> (String, SpeechExpression) {
        let mut parser = SpeechDirectiveProjection::default();
        let mut visible = String::new();
        let mut expression = None;
        for index in 0..=text.len() {
            if !text.is_char_boundary(index) {
                continue;
            }
            if index == 0 {
                continue;
            }
            let previous = (0..index)
                .rev()
                .find(|point| text.is_char_boundary(*point))
                .unwrap_or(0);
            if previous == index {
                continue;
            }
        }
        for ch in text.chars() {
            let step = parser.push(&ch.to_string());
            visible.push_str(&step.visible);
            if let Some(decided) = step.decided {
                expression = Some(decided);
            }
        }
        visible.push_str(&parser.finish().visible);
        (visible, expression.unwrap_or(SpeechExpression::Natural))
    }

    #[test]
    fn ve_06_all_valid_markers_project_at_every_split() {
        for (marker, expression) in MARKERS {
            let text = format!("{marker}\nhello");
            let mut parser = SpeechDirectiveProjection::default();
            let mut visible = String::new();
            let mut decided = None;
            for index in 1..=text.len() {
                let step = parser.push(&text[index - 1..index]);
                visible.push_str(&step.visible);
                decided = step.decided.or(decided);
            }
            visible.push_str(&parser.finish().visible);
            assert_eq!(visible, "hello");
            assert_eq!(decided, Some(*expression));
            assert!(!visible.contains("[$"));
        }
    }

    #[test]
    fn ve_06_missing_invalid_oversized_and_partial_markers_fall_back() {
        for sample in [
            "hello",
            "[$happy] hi",
            "[$Bright] hi",
            "[$bright speed=2] hi",
        ] {
            let (visible, expression) = feed_splits(sample);
            assert_eq!(expression, SpeechExpression::Natural);
            assert!(!visible.contains("[$"));
        }
        let (visible, expression) = feed_splits(&format!("[${}", "x".repeat(40)));
        assert_eq!(expression, SpeechExpression::Natural);
        assert!(!visible.contains("[$"));
        assert_eq!(visible, "x".repeat(10));
        let (visible, expression) = project_complete_assistant_content("[$bright");
        assert_eq!(expression, SpeechExpression::Natural);
        assert!(visible.is_empty());
    }

    #[test]
    fn ve_06_only_leading_marker_is_control() {
        let (visible, expression) = project_complete_assistant_content("hello [$bright] there");
        assert_eq!(expression, SpeechExpression::Natural);
        assert_eq!(visible, "hello  there");
        let (visible, expression) = project_complete_assistant_content("[$serious] keep [$bright]");
        assert_eq!(expression, SpeechExpression::Serious);
        assert_eq!(visible, "keep ");
    }

    #[test]
    fn ve_06_user_quote_and_code_are_not_parsed() {
        let (visible, expression) = project_complete_assistant_content("[通常文] $natural");
        assert_eq!(
            (visible.as_str(), expression),
            ("[通常文] $natural", SpeechExpression::Natural)
        );
    }

    #[test]
    fn ve_06_expression_math_clamps_and_preserves_natural_none() {
        let mut provider = CloudTtsProviderSettings {
            id: "tts".into(),
            enabled: true,
            label: "TTS".into(),
            location: "local".into(),
            endpoint: "http://127.0.0.1/v1".into(),
            model: "voicevox-core".into(),
            voice: "voice".into(),
            response_format: "wav".into(),
            authentication: "none".into(),
            style: Some("normal".into()),
            speed: None,
            pitch_scale: None,
            intonation_scale: None,
        };
        let natural = apply_expression(&provider, SpeechExpression::Natural);
        assert!(natural.speed.is_none());
        let bright = apply_expression(&provider, SpeechExpression::Bright);
        assert_eq!(bright.speed, Some(1.05));
        assert_eq!(bright.pitch_scale, Some(0.02));
        assert_eq!(bright.intonation_scale, Some(1.15));
        assert_eq!(bright.style.as_deref(), Some("normal"));
        provider.speed = Some(2.0);
        provider.pitch_scale = Some(0.15);
        provider.intonation_scale = Some(2.0);
        let excited = apply_expression(&provider, SpeechExpression::Excited);
        assert_eq!(excited.speed, Some(2.0));
        assert_eq!(excited.pitch_scale, Some(0.15));
        assert_eq!(excited.intonation_scale, Some(2.0));
        provider.model = "other".into();
        let ignored = apply_expression(&provider, SpeechExpression::Bright);
        assert_eq!(ignored.speed, Some(2.0));
    }
}
