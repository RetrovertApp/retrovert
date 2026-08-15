//! The `retrovert-publish` command-line entry point.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use jiff::Timestamp;
use retrovert_publish::{KeySet, Result, Workspace, init, publish};

#[derive(Debug, Parser)]
#[command(name = "retrovert-publish", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a channel and a fresh disposable test root in an empty directory.
    Init(InitArgs),

    /// Publish a release-set manifest as the channel's next generation.
    Publish(PublishArgs),
}

#[derive(Debug, Args)]
struct InitArgs {
    /// Workspace to create: `repository/` is publishable, `keys/` is not.
    dir: PathBuf,

    /// Initialize even if the directory already has contents.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct PublishArgs {
    /// Workspace holding the channel and its signing keys.
    dir: PathBuf,

    /// The release-set manifest to publish.
    manifest: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("retrovert-publish: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::Init(args) => run_init(args),
        Command::Publish(args) => run_publish(args),
    }
}

fn run_publish(args: &PublishArgs) -> Result<()> {
    let workspace = Workspace::new(&args.dir);
    let report = publish(&workspace, &args.manifest, Timestamp::now())?;

    println!("channel:  {}", workspace.channel().path().display());
    for path in &report.written {
        println!("  wrote   {}", path.display());
    }
    println!(
        "release:  v{} (revision {})",
        report.version, report.source_revision
    );
    println!("generation: {}", report.generation_id);
    Ok(())
}

fn run_init(args: &InitArgs) -> Result<()> {
    let workspace = Workspace::new(&args.dir);
    let keys = KeySet::generate()?;
    let report = init(&workspace, &keys, Timestamp::now(), args.force)?;

    println!("channel:  {}", workspace.channel().path().display());
    for path in &report.metadata {
        println!("  wrote   {}", path.display());
    }
    println!("root key id: {}", report.root_key_id);
    println!(
        "keys:     {} — do not publish; the root key belongs in offline storage",
        workspace.keys().path().display()
    );
    #[cfg(not(unix))]
    println!(
        "warning:  this platform has no owner-only enforcement; restrict that directory yourself"
    );
    Ok(())
}
