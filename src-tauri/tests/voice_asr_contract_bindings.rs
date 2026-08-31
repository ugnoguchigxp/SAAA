use std::{fs, path::PathBuf};

#[test]
fn generated_voice_asr_binding_is_current() {
    let expected = saaa_lib::voice_asr_contract::typescript_bindings();
    let binding_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/lib/generated/voiceAsr.ts");
    let actual = fs::read_to_string(&binding_path).unwrap_or_else(|error| {
        panic!(
            "could not read generated binding {}: {error}; run `bun run ipc:generate`",
            binding_path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "generated voice ASR binding is stale; run `bun run ipc:generate`"
    );
}
