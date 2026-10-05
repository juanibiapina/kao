#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod snapshot;

const USAGE: &str = "usage: kao run -- <command> [args...] 3>capture.tar";

#[cfg(target_os = "macos")]
fn run(command: &[std::ffi::OsString]) -> Result<i32, Box<dyn std::error::Error>> {
    capture::run(command)
}

#[cfg(not(target_os = "macos"))]
fn run(_: &[std::ffi::OsString]) -> Result<i32, Box<dyn std::error::Error>> {
    Err("capture currently requires macOS".into())
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result: Result<i32, Box<dyn std::error::Error>> =
        if args.len() < 3 || args[0] != "run" || args[1] != "--" {
            Err(USAGE.into())
        } else {
            run(&args[2..])
        };
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("kao: {error}");
            std::process::exit(125);
        }
    }
}
