use lsp_server::Connection;
use ruby_lsp_rs::server::{Server, ServerOptions};
use ruby_lsp_rs::type_check_command;

const USAGE: &str = "Usage:\n  ruby-lsp-rs [--stdio]    Start the language server on standard input and output\n  ruby-lsp-rs type-check   Type check the workspace (see `ruby-lsp-rs type-check --help`)";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        None | Some("--stdio") => {}
        Some(type_check_command::SUBCOMMAND) => std::process::exit(type_check_command::run(&arguments[1..])),
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
