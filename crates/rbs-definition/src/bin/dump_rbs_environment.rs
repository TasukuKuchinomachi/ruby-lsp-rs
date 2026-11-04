use std::{env, path::PathBuf, process::ExitCode};

use rbs_definition::{dump::dump_environment, loader::load};

fn main() -> ExitCode {
    let Some(directory) = env::args().nth(1) else {
        eprintln!(
            "usage: dump_rbs_environment <directory of the rbs gem> [signature directory...]"
        );
        return ExitCode::FAILURE;
    };
    match load(
        &PathBuf::from(&directory),
        &env::args().skip(2).map(PathBuf::from).collect::<Vec<_>>(),
    ) {
        Ok(environment) => {
            print!(
                "{}",
                dump_environment(&environment, &PathBuf::from(&directory))
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
