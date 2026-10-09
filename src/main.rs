use lsp_server::Connection;
use ruby_lsp_rs::server::{Server, ServerOptions};

const USAGE: &str = "Usage: ruby-lsp-rs [--stdio]    Start the language server on standard input and output";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        None | Some("--stdio") => {}
        Some("--help" | "-h") => {
            println!("{USAGE}");
            return;
        }
        Some(unknown) => {
            eprintln!("Unknown command: {unknown}\n{USAGE}");
            std::process::exit(2);
        }
    }

    let (connection, _io_threads) = Connection::stdio();
    let exit_code = Server::new(connection, ServerOptions::default()).run();
    std::process::exit(exit_code);
}
