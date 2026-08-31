use std::{fs, path::PathBuf};

fn main() {
    let output = saaa_lib::voice_asr_contract::typescript_bindings();
    let output_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/lib/generated/voiceAsr.ts");
    fs::create_dir_all(output_path.parent().expect("binding path has a parent"))
        .expect("generated binding directory is created");
    fs::write(&output_path, output).expect("generated voice ASR binding is written");
    println!("generated {}", output_path.display());
}
