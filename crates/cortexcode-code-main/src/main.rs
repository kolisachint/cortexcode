//! `cortex` binary entry point. Everything lives in `cortexcode-code-cli`.

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(cortexcode_code_cli::main(&argv));
}
