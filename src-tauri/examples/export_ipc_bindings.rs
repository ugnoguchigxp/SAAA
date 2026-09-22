use std::{fs, path::PathBuf};

fn main() {
    let output = saaa_lib::ipc_contract::typescript_bindings();
    let output_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/lib/generated/runtimeEvent.ts");
    fs::create_dir_all(output_path.parent().expect("binding path has a parent"))
        .expect("generated binding directory is created");
    fs::write(&output_path, output).expect("generated RuntimeEvent binding is written");
    fs::write(
        output_path.with_file_name("generativeUi.ts"),
        saaa_lib::ipc_contract::ui_typescript_bindings(),
    )
    .expect("UI bindings are written");
    fs::write(
        output_path.with_file_name("coding.ts"),
        saaa_lib::ipc_contract::coding_typescript_bindings(),
    )
    .expect("coding bindings are written");
    fs::write(
        output_path.with_file_name("schedule.ts"),
        saaa_lib::ipc_contract::schedule_typescript_bindings(),
    )
    .expect("schedule bindings are written");
    fs::write(
        output_path.with_file_name("steward.ts"),
        saaa_lib::ipc_contract::steward_typescript_bindings(),
    )
    .expect("steward bindings are written");
    fs::write(
        output_path.with_file_name("artifactPreview.ts"),
        saaa_lib::ipc_contract::artifact_preview_typescript_bindings(),
    )
    .expect("artifact preview bindings are written");
    println!("generated {}", output_path.display());
}
