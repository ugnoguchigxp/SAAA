#[cfg(feature = "quality-eval-harness")]
#[tokio::main]
async fn main() {
    use std::io::Read;
    let mut input = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("Could not read quality runtime request: {error}");
        std::process::exit(1);
    }
    match saaa_lib::quality_eval::run_json(&input).await {
        Ok(response) => println!("{response}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(feature = "quality-eval-harness"))]
fn main() {
    eprintln!("conversation_quality_runtime requires --features quality-eval-harness");
    std::process::exit(64);
}
