mod capture;
mod snapshot;

const USAGE: &str = "usage: kao run -- <command> [args...] 3>capture.tar";

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = if args.len() < 3 || args[0] != "run" || args[1] != "--" {
        Err(USAGE.into())
    } else {
        capture::run(&args[2..])
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("kao: {error}");
            std::process::exit(125);
        }
    }
}
