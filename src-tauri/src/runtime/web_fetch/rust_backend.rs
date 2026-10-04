use super::*;

pub(super) async fn execute_via_rust(
    call: &AgentToolCall,
    timeout: Duration,
    cancellation: WebFetchCancel,
) -> String {
    let Some(runtime) = WEB_FETCH_RUNTIME.get() else {
        return tool_error_content(
            "web-fetch-unavailable",
            "WebFetch is temporarily unavailable.",
        );
    };
    let arguments = match serde_json::from_str::<Value>(&call.arguments) {
        Ok(Value::Object(arguments)) => Value::Object(arguments),
        _ => {
            return tool_error_content(
                "INVALID_INPUT",
                "Tool arguments do not match the WebFetch schema.",
            );
        }
    };
    if call.name == FETCH_CONTENT_TOOL_NAME {
        let input = match FetchContentInput::parse(&arguments) {
            Ok(input) => input,
            Err(_) => {
                return tool_error_content(
                    "INVALID_INPUT",
                    "Tool arguments do not match the WebFetch schema.",
                );
            }
        };
        match runtime.content.fetch(input, timeout, cancellation).await {
            Ok(result) => content::render_compact(&result),
            Err(failure) => failure_content(&failure),
        }
    } else if call.name == WEB_SEARCH_TOOL_NAME {
        let input = match SearchInput::parse(&arguments) {
            Ok(input) => input,
            Err(_) => {
                return tool_error_content(
                    "INVALID_INPUT",
                    "Tool arguments do not match the WebFetch schema.",
                );
            }
        };
        // Lazily built Rust provider is also usable without the global slot;
        // prefer the installed runtime so tests can inject fakes.
        match runtime.search.search(input, timeout, cancellation).await {
            Ok(outcome) => search::render_compact(&outcome),
            Err(failure) => failure_content(&failure),
        }
    } else {
        tool_error_content(
            "INVALID_INPUT",
            "Tool arguments do not match the WebFetch schema.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::web_fetch::content::FakeContentFetcher;

    #[test]
    fn definitions_match_the_llm_fetch_chat_completions_contract() {
        let definitions = tool_definitions();
        let names = definitions
            .iter()
            .map(|definition| {
                definition
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .expect("tool name exists")
            })
            .collect::<Vec<_>>();
        assert_eq!(names, WEB_FETCH_TOOL_NAMES);
        assert_eq!(
            definitions[1].pointer("/function/parameters/required"),
            Some(&json!(["url", "maxCharacters", "query"]))
        );
        assert!(definitions.iter().all(|definition| {
            definition.pointer("/function/strict") == Some(&Value::Bool(true))
                && definition.pointer("/function/parameters/additionalProperties")
                    == Some(&Value::Bool(false))
        }));
        let serialized = serde_json::to_string(&definitions).expect("definitions serialize");
        assert!(serialized.contains("Search with a concise standalone query"));
        assert!(serialized.contains("split the request into short independent queries"));
        assert!(serialized.contains("answer from that snippet"));
        assert!(serialized.contains("answer immediately from that snippet"));
        assert!(serialized.contains("a Markdown source link with a descriptive label"));
        assert!(serialized.contains("no cursor, click, typing, scrolling, or form actions"));
        for unsupported in ["cursorX", "cursorY", "selector", "click", "keystrokes"] {
            assert!(definitions.iter().all(|definition| definition
                .pointer("/function/parameters/properties")
                .and_then(Value::as_object)
                .is_none_or(|properties| !properties.contains_key(unsupported))));
        }
    }

    #[test]
    fn sidecar_removes_rust_only_query_and_sets_the_same_default_budget() {
        let call = AgentToolCall {
            id: "fetch".into(),
            name: FETCH_CONTENT_TOOL_NAME.into(),
            arguments: json!({"url":"https://example.com","maxCharacters":null,"query":"revenue"})
                .to_string(),
        };
        let projected: Value =
            serde_json::from_str(&sidecar_compatible_call(&call).unwrap().arguments).unwrap();
        assert!(projected.get("query").is_none());
        assert_eq!(projected["maxCharacters"], 2_500);
        let invalid = AgentToolCall {
            arguments: json!({"url":"https://example.com","maxCharacters":null,"query":7})
                .to_string(),
            ..call
        };
        assert!(sidecar_compatible_call(&invalid).is_err());
    }

    #[tokio::test]
    async fn rust_backend_serves_fetch_content_with_fake_and_keeps_schema() {
        install_runtime_for_tests(WebFetchRuntime {
            content: std::sync::Arc::new(FakeContentFetcher::ok("hello world")),
            search: std::sync::Arc::new(FakeSearch),
        });
        // Force webview without touching process env for other tests: call
        // the rust path directly through the installed runtime.
        let call = AgentToolCall {
            id: "call_test".to_string(),
            name: FETCH_CONTENT_TOOL_NAME.to_string(),
            arguments: r#"{"url":"https://example.com/","maxCharacters":500}"#.to_string(),
        };
        let arguments: Value = serde_json::from_str(&call.arguments).unwrap();
        let input = FetchContentInput::parse(&arguments).unwrap();
        let runtime = WEB_FETCH_RUNTIME.get().unwrap();
        let result = runtime
            .content
            .fetch(input, Duration::from_secs(5), WebFetchCancel::never())
            .await
            .unwrap();
        let rendered: Value = serde_json::from_str(&content::render_compact(&result)).unwrap();
        assert_eq!(
            rendered.pointer("/type").and_then(|v| v.as_str()),
            Some("fetch_content_result")
        );
        assert_eq!(
            rendered
                .pointer("/security/tainted")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    struct FakeSearch;

    #[async_trait::async_trait]
    impl SearchProvider for FakeSearch {
        async fn search(
            &self,
            _input: SearchInput,
            _deadline: Duration,
            _cancellation: WebFetchCancel,
        ) -> Result<search::SearchOutcome, contracts::WebFetchFailure> {
            Ok(search::SearchOutcome {
                hits: Vec::new(),
                blocked_result_count: 0,
                warning_categories: Vec::new(),
                decision: "allow",
            })
        }
    }

    #[tokio::test]
    async fn bundled_sidecar_executes_the_llm_fetch_protocol() {
        // WF-12: on macOS the sidecar binary is no longer staged. When no
        // binary is available the backend must fail safe (unavailable) rather
        // than panic; rollback restores the binary via SAAA_WEBFETCH_PATH.
        let result = execute_via_sidecar(
            &AgentToolCall {
                id: "call_webfetch_test".to_string(),
                name: FETCH_CONTENT_TOOL_NAME.to_string(),
                arguments: r#"{"url":"http://127.0.0.1/private","maxCharacters":500}"#.to_string(),
            },
            Duration::from_secs(5),
            WebFetchCancel::never(),
        )
        .await;

        if std::env::var_os("SAAA_WEBFETCH_PATH").is_some()
            || super::sidecar::BUNDLED_WEB_FETCH_PATH.get().is_some()
            || std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join("bin")
                .join(if cfg!(windows) {
                    "webfetch.exe"
                } else {
                    "webfetch"
                })
                .is_file()
        {
            assert!(result.contains("UNSAFE_URL"));
            assert!(!result.contains("127.0.0.1"));
        } else {
            // No staged binary (macOS post-WF-12): must fail safe.
            assert!(result.contains("web-fetch-unavailable"));
        }
    }
}
