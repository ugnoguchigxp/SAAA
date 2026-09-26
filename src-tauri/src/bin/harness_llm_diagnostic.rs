//! Runs the ordinary library build, not a cfg(test) canary.
#[tokio::main]
async fn main() {
    let Some(target) = std::env::args().nth(1) else {
        eprintln!("Usage: harness_llm_diagnostic <control-base-url|database> [--enable-role-routing]");
        std::process::exit(2);
    };
    let result = if std::env::args().nth(2).as_deref() == Some("--enable-role-routing") {
        saaa_lib::harness_llm_diagnostic::enable_role_routing(&target)
    } else {
        saaa_lib::harness_llm_diagnostic::check_response(&target).await
    };
    match result {
        Ok(report) => println!("{report}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
