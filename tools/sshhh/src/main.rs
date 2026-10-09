mod cli;

fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("{{\"error\":\"io_error\",\"message\":\"cannot start the async runtime: {e}\"}}");
            return std::process::ExitCode::from(255);
        }
    };
    std::process::ExitCode::from(runtime.block_on(cli::main()))
}
