use lsp_server::Connection;
use ruby_lsp_rs::server::{Server, ServerOptions};

fn main() {
    let (connection, _io_threads) = Connection::stdio();
    let exit_code = Server::new(connection, ServerOptions::default()).run();
    std::process::exit(exit_code);
}
