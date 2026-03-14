use rusty::cli::run;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("Fatal: {e}");
        std::process::exit(1);
    }
}
