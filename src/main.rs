#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
mod watcher;

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() {
        println!("Hello, world!");
        return;
    }
    #[cfg(target_os = "macos")]
    let result = capture::run(&args);
    #[cfg(not(target_os = "macos"))]
    let result: Result<i32, Box<dyn std::error::Error>> =
        Err("capture currently requires macOS with APFS".into());
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("kao: {error}");
            std::process::exit(125);
        }
    }
}
