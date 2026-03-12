use symphony::cli;

#[tokio::main]
async fn main() {
    if let Err(e) = cli::run().await {
        eprintln!("Fatal: {e}");
        std::process::exit(1);
    }
}
