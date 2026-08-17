use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use mr_crabs_oracle::{refresh_corpus_dir, run_corpus_dir};

fn main() -> ExitCode {
    match run() {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("mr-crabs-oracle: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<String, String> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    match arguments
        .next()
        .and_then(|argument| argument.into_string().ok())
        .as_deref()
    {
        Some("corpus") => {
            let directory = required_path(&mut arguments, "corpus directory")?;
            reject_extra_arguments(&mut arguments)?;
            let checked = run_corpus_dir(&directory).map_err(|error| error.to_string())?;
            Ok(format!("checked {checked} immutable Ghostty corpus cases"))
        }
        Some("refresh") => {
            let executable = required_path(&mut arguments, "Ghostty oracle executable")?;
            let directory = required_path(&mut arguments, "corpus directory")?;
            reject_extra_arguments(&mut arguments)?;
            let refreshed =
                refresh_corpus_dir(&executable, &directory).map_err(|error| error.to_string())?;
            Ok(format!("refreshed {refreshed} Ghostty corpus cases"))
        }
        _ => Err(usage()),
    }
}

fn required_path(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
    description: &str,
) -> Result<PathBuf, String> {
    arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {description}\n{}", usage()))
}

fn reject_extra_arguments(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
) -> Result<(), String> {
    if let Some(argument) = arguments.next() {
        Err(format!("unexpected argument {:?}\n{}", argument, usage()))
    } else {
        Ok(())
    }
}

fn usage() -> String {
    concat!(
        "usage:\n",
        "  mr-crabs-oracle corpus <corpus-directory>\n",
        "  mr-crabs-oracle refresh <ghostty-oracle-executable> <corpus-directory>"
    )
    .to_owned()
}
