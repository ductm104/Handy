use clap::Parser;

#[derive(Parser, Debug, Clone, Default)]
#[command(name = "hanhcute", about = "HanhCute - Speech to Text")]
pub struct CliArgs {
    /// Start with the main window hidden
    #[arg(long)]
    pub start_hidden: bool,

    /// Disable the system tray icon
    #[arg(long)]
    pub no_tray: bool,

    /// Open the running instance (legacy recording toggle is disabled in file-only mode)
    #[arg(long)]
    pub toggle_transcription: bool,

    /// Open the running instance (legacy recording toggle is disabled in file-only mode)
    #[arg(long)]
    pub toggle_post_process: bool,

    /// Cancel the current operation (sent to running instance)
    #[arg(long)]
    pub cancel: bool,

    /// Enable debug mode with verbose logging
    #[arg(long)]
    pub debug: bool,
}
