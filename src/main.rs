use lsp_server::Connection;
use ruby_lsp_rs::check_command;
use ruby_lsp_rs::server::{Server, ServerOptions};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().is_some_and(|argument| argument == "check") {
        std::process::exit(check_command::run(&arguments[1..]));
    }

    let (connection, _io_threads) = Connection::stdio();
    let exit_code = Server::new(connection, ServerOptions::default()).run();
    std::process::exit(exit_code);
}
