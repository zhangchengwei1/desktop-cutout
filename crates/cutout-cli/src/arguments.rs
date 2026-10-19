//! clap owns option validation and OS-native paths; execution stays separate.
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "cutout-cli",
    version,
    about = "ClearFrame native verification tools"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Print the core runtime contract (also the default command).
    Info,
    /// Inspect a supported image without running inference.
    Inspect { image: PathBuf },
    /// Apply a same-size grayscale mask, preserving source alpha.
    ApplyMask {
        image: PathBuf,
        mask: PathBuf,
        output: PathBuf,
    },
    /// Verify fixed model byte size and SHA-256.
    VerifyModel { model_id: String, model: PathBuf },
    /// Run high-quality IS-Net and report stage timings.
    Cutout {
        runtime: PathBuf,
        model: PathBuf,
        image: PathBuf,
        output: PathBuf,
        #[arg(long)]
        tensor: Option<PathBuf>,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=20))]
        runs: u32,
    },
    /// Run IS-Net from a prepared tensor.
    InferTensor {
        runtime: PathBuf,
        model: PathBuf,
        input: PathBuf,
        output: PathBuf,
    },
    /// Run MobileSAM with JSON point groups.
    Select {
        runtime: PathBuf,
        encoder: PathBuf,
        decoder: PathBuf,
        image: PathBuf,
        output: PathBuf,
        groups: PathBuf,
    },
    /// Refine a coarse mask with ViTMatte.
    Refine {
        runtime: PathBuf,
        model: PathBuf,
        image: PathBuf,
        coarse: PathBuf,
        output: PathBuf,
        #[arg(long, default_value_t = 12, value_parser = clap::value_parser!(u32).range(0..=1024))]
        edge: u32,
    },
    /// Run the explicit batch benchmark; ZIP modes are mutually exclusive.
    BatchBench {
        runtime: PathBuf,
        models: PathBuf,
        plan: PathBuf,
        output: PathBuf,
        #[arg(long, conflicts_with = "zip_during")]
        zip: bool,
        #[arg(long, conflicts_with = "zip")]
        zip_during: bool,
    },
}
