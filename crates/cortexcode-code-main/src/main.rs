//! `cortex` binary entry point.

use cortexcode_code_main::run;
use std::process;

fn main() {
    // Auto-migrate settings from the legacy TypeScript hoocode CLI's
    // `~/.hoocode/settings.json` on first run, writing the converted result
    // to `~/.cortexcode/config.json`. Best-effort: a failure here must not
    // block the CLI from starting.
    if let Err(e) = cortexcode_code_config::migrate::auto_migrate() {
        eprintln!("warning: failed to load or migrate config: {}", e);
    }

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
