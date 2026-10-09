use clap::Parser;

#[derive(Parser)]
#[command(name = "sshhh", version)]
struct Cli {}

fn main() {
    let _ = Cli::parse();
    let _ = russh::client::Config::default();
    let _ = sshhh::error::Error::Usage(String::new()).code();
}
