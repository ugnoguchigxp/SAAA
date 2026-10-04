fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(result) = saaa_terminal_agent_runtime::helper(&args) {
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(2);
        }
    } else {
        eprintln!("--saaa-terminal-agent <run|mcp|hook|view> <private run directory>");
        std::process::exit(2);
    }
}
