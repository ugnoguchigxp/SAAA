fn main() {
    let mut scenario = None;
    let mut json = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--scenario" => scenario = args.next(),
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let code = saaa_lib::tool_selection::simulator::run(scenario.as_deref(), json);
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    #[test]
    fn scenarios_match_the_production_path() {
        assert_eq!(saaa_lib::tool_selection::simulator::run(None, false), 0);
    }
}
