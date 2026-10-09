fn main() {
    let status = match rclaude::launcher::run() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("rclaude: {error}");
            1
        }
    };
    std::process::exit(status);
}
