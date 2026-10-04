use crate::{Event, Spec};
use std::{
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};

pub fn view(directory: &Path, spec: &Spec) -> Result<(), String> {
    crate::write_private(
        &directory.join("viewer-receipt.json"),
        &serde_json::to_vec(
            &serde_json::json!({"run":spec.run,"nonce":spec.nonce,"pid":std::process::id()}),
        )
        .map_err(|e| e.to_string())?,
    )?;
    println!(
        "SAAA · {}\n{}\nこの画面は進捗表示専用です。回答・追加指示・停止はSAAAで行ってください。\n",
        spec.cli,
        spec.workspace.display()
    );
    let mut offset = 0;
    let mut done = None;
    loop {
        if let Ok(mut file) = std::fs::File::open(directory.join("events.jsonl")) {
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| e.to_string())?;
            let mut reader = BufReader::new(file);
            loop {
                let mut line = String::new();
                let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
                if n == 0 || !line.ends_with('\n') {
                    break;
                }
                offset += n as u64;
                if let Ok(event) = serde_json::from_str::<Event>(&line) {
                    if event.run != spec.run || event.nonce != spec.nonce {
                        continue;
                    }
                    let text = format!("[{}] {}", event.kind, event.data);
                    // Never interpret CLI output as terminal escape sequences/OSC clipboard commands.
                    println!(
                        "{}",
                        text.chars()
                            .filter(|c| !c.is_control())
                            .take(4000)
                            .collect::<String>()
                    );
                    if event.kind == "exit" {
                        done = Some(Instant::now());
                    }
                }
            }
        }
        if done.is_some_and(|at| at.elapsed() > Duration::from_secs(10)) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
