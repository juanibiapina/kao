mod capture;
mod snapshot;

const USAGE: &str =
    "usage: kao run -- <command> [args...] 3>capture.tar\n       kao lock -- <command> [args...]";

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = if args.len() < 3 || args[1] != "--" {
        Err(USAGE.into())
    } else if args[0] == "run" {
        capture::run(&args[2..])
    } else if args[0] == "lock" {
        capture::lock(&args[2..])
    } else {
        Err(USAGE.into())
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("kao: {error}");
            std::process::exit(125);
        }
    }
}
