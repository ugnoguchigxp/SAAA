//! Separate process so environment changes cannot leak into the library tests.
use std::path::PathBuf;

use saaa_feature_lab::{open_lab, open_service, LabConfig};

#[test]
fn unrelated_environment_does_not_supply_a_larm_token() {
    std::env::set_var("SAAA_LARM_TOKEN", "env-token-must-not-be-used-0123456789");
    std::env::set_var("REPLICATE_API_TOKEN", "env-replicate-must-not-be-used");
    std::env::set_var("HOME", "/tmp/saaa-lab-no-home");
    let directory = std::env::temp_dir().join(format!("saaa-lab-env-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let path: PathBuf = directory.join("lab.sqlite");
    let open = open_lab(LabConfig {
        database_path: path.display().to_string(),
        allowed_origin: "http://127.0.0.1:1422".into(),
        session_token: "0123456789abcdef0123456789abcdef".into(),
        provider: "larm".into(),
        larm_endpoint: "http://127.0.0.1:9/".into(),
        larm_token: None,
        replace_route: false,
    })
    .unwrap();
    let Err(error) = open_service(open, "127.0.0.1:9".into()) else {
        panic!("larm without an explicit token must be refused");
    };
    assert!(error.contains("explicit token"));
    assert!(!error.contains("env-token"));
    assert!(path.exists());
    let _ = std::fs::remove_dir_all(directory);
}
