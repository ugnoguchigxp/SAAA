use std::{fs, path::PathBuf};

#[test]
fn generated_runtime_event_binding_is_current() {
    let expected = saaa_lib::ipc_contract::typescript_bindings();
    let binding_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/lib/generated/runtimeEvent.ts");
    let actual = fs::read_to_string(&binding_path).unwrap_or_else(|error| {
        panic!(
            "could not read generated binding {}: {error}; run `bun run ipc:generate`",
            binding_path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "generated RuntimeEvent binding is stale; run `bun run ipc:generate`"
    );
}

#[test]
fn generated_ui_binding_is_current() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/lib/generated/generativeUi.ts");
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        saaa_lib::ipc_contract::ui_typescript_bindings()
    );
}

#[test]
fn generated_coding_binding_is_current() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/lib/generated/coding.ts");
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        saaa_lib::ipc_contract::coding_typescript_bindings()
    );
}

#[test]
fn generated_schedule_binding_is_current() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/lib/generated/schedule.ts");
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        saaa_lib::ipc_contract::schedule_typescript_bindings()
    );
}

#[test]
fn generated_steward_binding_is_current() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/lib/generated/steward.ts");
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        saaa_lib::ipc_contract::steward_typescript_bindings()
    );
}
