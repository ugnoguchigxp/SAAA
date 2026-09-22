#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "saaa-generation-config-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, bytes: &[u8]) {
        let mut file = fs::File::create(path).unwrap();
        file.write_all(bytes).unwrap();
    }

    fn sample_requests(dir: &Path) -> PathBuf {
        let request = dir.join("request.json");
        let suite = dir.join("tests.json");
        let metadata = dir.join("metadata.json");
        write(
            &request,
            serde_json::json!({
                "version": 2,
                "id": "req-cap",
                "body": "x",
                "contract": {
                    "version": 1,
                    "fields": [
                        { "name": "enabled", "kind": "boolean", "values": [], "nullable": false, "undefinable": false, "optional": false },
                        { "name": "suspended", "kind": "boolean", "values": [], "nullable": false, "undefinable": false, "optional": false }
                    ]
                }
            })
            .to_string()
            .as_bytes(),
        );
        write(&suite, b"{\"version\":1}");
        write(&metadata, b"{\"id\":\"req-cap\"}");
        let path = dir.join("requests.json");
        write(
            &path,
            serde_json::json!({
                "formatVersion": 1,
                "entries": [{
                    "id": "req-1",
                    "capabilityId": "req-cap",
                    "purpose": "Decide whether a user may proceed.",
                    "fields": ["enabled", "suspended"],
                    "requestPath": request.to_string_lossy(),
                    "suitePath": suite.to_string_lossy(),
                    "metadataPath": metadata.to_string_lossy(),
                    "acceptanceId": "acc-1",
                    "scope": { "kind": "user" },
                    "allowCreate": true,
                    "allowUpdate": false,
                    "autoActivate": true,
                    "grantOnCreate": true
                }]
            })
            .to_string()
            .as_bytes(),
        );
        path
    }

    fn sample_config(dir: &Path) -> PathBuf {
        let bun = dir.join("bun");
        write(&bun, b"#!/bin/sh\n");
        let requests = sample_requests(dir);
        let path = dir.join("generation.json");
        write(
            &path,
            serde_json::json!({
                "formatVersion": 1,
                "enabled": true,
                "bunPath": bun.to_string_lossy(),
                "kitRoot": dir.to_string_lossy(),
                "expectedKitDigest": "a".repeat(64),
                "requestsPath": requests.to_string_lossy()
            })
            .to_string()
            .as_bytes(),
        );
        path
    }

    #[test]
    fn config_round_trips_and_requires_enabled() {
        let dir = temp_dir("config");
        let path = sample_config(&dir);
        let config = load_config(&path).unwrap();
        assert!(config.enabled);
        assert_eq!(config.format_version, 1);

        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("enabled");
        write(&path, value.to_string().as_bytes());
        assert!(load_config(&path).is_err());

        value
            .as_object_mut()
            .unwrap()
            .insert("enabled".into(), true.into());
        value
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), true.into());
        write(&path, value.to_string().as_bytes());
        assert!(load_config(&path).is_err());
    }

    #[test]
    fn requests_are_hashed_at_load_and_validated() {
        let dir = temp_dir("requests");
        let path = sample_requests(&dir);
        let entries = load_requests(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].fields, vec!["enabled", "suspended"]);
        assert_eq!(entries[0].scope, RequestScope::User);
        assert_eq!(entries[0].request_hash.len(), 64);

        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value["entries"][0]["fields"] = serde_json::json!(["enabled", "enabled"]);
        write(&path, value.to_string().as_bytes());
        assert!(load_requests(&path).is_err());

        value["entries"][0]["fields"] = serde_json::json!(["enabled", "9bad"]);
        write(&path, value.to_string().as_bytes());
        assert!(load_requests(&path).is_err());
    }

    #[test]
    fn request_contract_mismatch_is_rejected() {
        let dir = temp_dir("contract");
        let path = sample_requests(&dir);
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value["entries"][0]["fields"] = serde_json::json!(["suspended", "enabled"]);
        write(&path, value.to_string().as_bytes());
        assert!(load_requests(&path).is_err());
    }

    #[test]
    fn duplicate_request_ids_are_rejected() {
        let dir = temp_dir("dupes");
        let path = sample_requests(&dir);
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let entry = value["entries"][0].clone();
        value["entries"].as_array_mut().unwrap().push(entry);
        write(&path, value.to_string().as_bytes());
        assert!(load_requests(&path).is_err());
    }

    #[test]
    fn unset_environment_skips_generation_config() {
        let previous = std::env::var_os(GENERATION_CONFIG_ENV);
        std::env::remove_var(GENERATION_CONFIG_ENV);
        let loaded = from_environment();
        match previous {
            Some(value) => std::env::set_var(GENERATION_CONFIG_ENV, value),
            None => std::env::remove_var(GENERATION_CONFIG_ENV),
        }
        assert!(loaded.unwrap().is_none());
    }
}
