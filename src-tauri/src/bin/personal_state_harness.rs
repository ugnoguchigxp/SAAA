#[cfg(feature = "quality-eval-harness")]
#[tokio::main]
async fn main() {
    use std::io::Read;
    let mut input = String::new();
    if std::io::stdin()
        .take(262145)
        .read_to_string(&mut input)
        .is_err()
    {
        std::process::exit(1);
    }
    match saaa_lib::personal_state_harness::run_json(&input).await {
        Ok(result) => println!("{result}"),
        Err(_) => {
            eprintln!("harness-failed");
            std::process::exit(1);
        }
    }
}
#[cfg(not(feature = "quality-eval-harness"))]
fn main() {
    eprintln!("personal_state_harness requires quality-eval-harness");
    std::process::exit(64);
}
