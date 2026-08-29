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
