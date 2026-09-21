//! Runs the ordinary library build, not a cfg(test) canary.
#[tokio::main]
async fn main() {
    let Some(host) = std::env::args().nth(1) else {
        eprintln!("Usage: harness_llm_diagnostic <private-host-without-port>");
        std::process::exit(2);
    };
    let result = if std::env::args().nth(2).as_deref() == Some("--frontdesk") {
        saaa_lib::harness_llm_diagnostic::check_frontdesk(&host).await
    } else { saaa_lib::harness_llm_diagnostic::check_response(&host).await };
    match result {
        Ok(report) => println!("{report}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
