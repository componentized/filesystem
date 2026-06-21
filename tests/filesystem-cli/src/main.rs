use std::{
    fs::{self, File, OpenOptions},
    io,
    path::PathBuf,
};

use clap::{Parser, Subcommand};

/// componentized filesystem CLI
#[derive(Debug, Parser)] // requires `derive` feature
#[command(name = "filesystem")]
#[command(about = "componentized filesystem CLI", long_about = None)]
struct FilesystemCli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand, Clone)]
enum Commands {
    /// List items in a directory
    List {
        #[arg()]
        path: PathBuf,
    },

    /// Read a file
    #[command(arg_required_else_help = true)]
    Read {
        #[arg()]
        path: PathBuf,
    },

    /// Write a file
    #[command(arg_required_else_help = true)]
    Write {
        #[arg()]
        path: PathBuf,
    },

    /// Append to a file
    #[command(arg_required_else_help = true)]
    Append {
        #[arg()]
        path: PathBuf,
    },

    /// Move a file
    #[command(arg_required_else_help = true)]
    Move {
        #[arg()]
        from: PathBuf,

        #[arg()]
        to: PathBuf,
    },

    /// Remove a file
    #[command(arg_required_else_help = true)]
    Remove {
        #[arg()]
        path: PathBuf,
    },
}

fn main() -> Result<(), std::io::Error> {
    match FilesystemCli::parse().command {
        Commands::List { path } => {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                println!("{:?}", entry.file_name());
            }
            Ok(())
        }
        Commands::Read { path } => {
            let mut from = OpenOptions::new().read(true).open(path)?;
            let mut to = io::stdout();
            io::copy(&mut from, &mut to)?;
            Ok(())
        }
        Commands::Write { path } => {
            let mut from = io::stdin();
            let mut to = File::create(path)?;
            io::copy(&mut from, &mut to)?;
            Ok(())
        }
        Commands::Append { path } => {
            let mut from = io::stdin();
            let mut to = OpenOptions::new().create(true).append(true).open(path)?;
            io::copy(&mut from, &mut to)?;
            Ok(())
        }
        Commands::Move { from, to } => fs::rename(from, to),
        Commands::Remove { path } => {
            if path.is_dir() {
                return fs::remove_dir_all(path);
            }
            fs::remove_file(path)
        }
    }
}
