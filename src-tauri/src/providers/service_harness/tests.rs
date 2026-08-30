use super::*;
use std::io::{Read, Write};

#[test]
fn descriptor_rejects_duplicate_capabilities_and_cross_host_urls() {
    let base = url::Url::parse("http://provider.local:9810/").unwrap();
    let duplicate = HarnessDescriptor {
        contract_version: "saaa-service-harness.v1".to_string(),
        revision: "r1".to_string(),
        services: vec![
            ServiceDescriptor {
                capability: "llm".to_string(),
                protocol: "openai.chat-completions.v1".to_string(),
                base_url: "http://provider.local:8080/v1".to_string(),
                model: "m".to_string(),
                language: None,
                voice: None,
                health_url: "http://provider.local:8080/health".to_string(),
            },
            ServiceDescriptor {
                capability: "llm".to_string(),
                protocol: "openai.chat-completions.v1".to_string(),
                base_url: "http://other.local:8080/v1".to_string(),
                model: "m".to_string(),
                language: None,
                voice: None,
                health_url: "http://other.local:8080/health".to_string(),
            },
        ],
    };
    assert!(validate_descriptor(&base, &duplicate).is_err());
}

#[test]
fn address_rejects_public_http_and_descriptor_rejects_https_downgrade() {
    assert!(validate_address("http://provider.example/v1").is_err());
    assert!(validate_address("http://provider.local:9810").is_ok());

    let base = url::Url::parse("https://provider.example/").unwrap();
    let descriptor = HarnessDescriptor {
        contract_version: "saaa-service-harness.v1".to_string(),
        revision: "r1".to_string(),
        services: vec![ServiceDescriptor {
            capability: "llm".to_string(),
            protocol: "openai.chat-completions.v1".to_string(),
            base_url: "http://provider.example/v1".to_string(),
            model: "m".to_string(),
            language: None,
            voice: None,
            health_url: "http://provider.example/health".to_string(),
        }],
    };
    assert!(validate_descriptor(&base, &descriptor).is_err());
}

#[test]
fn legacy_fallback_only_accepts_the_original_dynamic_lan_address_shape() {
    assert_eq!(
        legacy_dynamic_lan_host("http://provider.local:9810").unwrap(),
        Some("provider.local".to_string())
    );
    for address in [
        "https://provider.example",
        "http://provider.local:9811",
        "http://provider.local:9810/harness",
        "http://[::1]:9810",
    ] {
        assert_eq!(legacy_dynamic_lan_host(address).unwrap(), None, "{address}");
    }
}

#[test]
fn asr_descriptor_requires_automatic_language_detection() {
    let base = url::Url::parse("http://provider.local:9810/").unwrap();
    let mut descriptor = HarnessDescriptor {
        contract_version: "saaa-service-harness.v1".to_string(),
        revision: "r1".to_string(),
        services: vec![ServiceDescriptor {
            capability: "asr".to_string(),
            protocol: "openai.audio-transcriptions.v1".to_string(),
            base_url: "http://provider.local:8080/v1".to_string(),
            model: "m".to_string(),
            language: Some("ja".to_string()),
            voice: None,
            health_url: "http://provider.local:8080/health".to_string(),
        }],
    };
    assert!(validate_descriptor(&base, &descriptor).is_err());
    descriptor.services[0].language = Some("auto".to_string());
    assert!(validate_descriptor(&base, &descriptor).is_ok());
}

#[tokio::test]
async fn resolution_marks_a_service_unavailable_when_its_health_check_fails() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for response in [
            format!(
                "{{\"contractVersion\":\"saaa-service-harness.v1\",\"revision\":\"r1\",\"services\":[{{\"capability\":\"asr\",\"protocol\":\"openai.audio-transcriptions.v1\",\"baseUrl\":\"http://{address}/v1\",\"model\":\"m\",\"language\":\"auto\",\"healthUrl\":\"http://{address}/health\"}}]}}"
            ),
            "unavailable".to_string(),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2_048];
            let _ = stream.read(&mut request).unwrap();
            let status = if response == "unavailable" {
                "503 Service Unavailable"
            } else {
                "200 OK"
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                response.len()
            )
            .unwrap();
        }
    });

    let resolution = resolve(&format!("http://{address}")).await.unwrap();
    server.join().unwrap();
    assert_eq!(resolution.state, "degraded");
    let asr = resolution
        .services
        .iter()
        .find(|service| service.capability == "asr")
        .unwrap();
    assert_eq!(asr.state, "unavailable");
    assert!(asr.message.contains("HTTP 503"));
}
