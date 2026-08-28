use clap::Parser;

fn main() {
    let cli = ui_box::cli::Cli::parse();
    std::process::exit(ui_box::dispatch(cli));
}
