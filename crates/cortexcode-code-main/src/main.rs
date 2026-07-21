//! `cortex` binary entry point.

use cortexcode_code_main::run;
use std::process;

fn main() {
    let args = cortexcode_code_main::parse_args(&std::env::args().skip(1).collect::<Vec<_>>());

    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    match run(&args, &mut stdout, &mut stderr) {
        Ok(code) => process::exit(code.into()),
        Err(e) => {
            eprintln!("fatal error: {}", e);
            process::exit(1);
        }
    }
}
