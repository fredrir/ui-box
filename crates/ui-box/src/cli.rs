use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::config::{Overrides, Surface};
use crate::flow::SnapMode;

#[derive(Debug, Parser)]
#[command(
    name = "ui-box",
    version,
    about = "Harness-agnostic live UI testing",
    long_about = "Drive a real UI through a driver process, record every step as it lands, \
                  and replay it. Machine summaries go to stdout, detail to stderr. \
                  Exit 0 means the thing under test passed, 1 means it failed, \
                  2 means ui-box itself could not run."
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Args)]
pub struct GlobalArgs {
    #[arg(
        long,
        global = true,
        value_name = "URL",
        help = "ssh://user@host or local://"
    )]
    pub backend: Option<String>,

    #[arg(long, global = true, value_name = "WxHxD")]
    pub display: Option<String>,

    #[arg(long, global = true, value_name = "DIR")]
    pub artifacts: Option<PathBuf>,

    #[arg(long, global = true, value_name = "GIT")]
    pub goldens: Option<String>,

    #[arg(long, global = true, value_name = "SECONDS")]
    pub session_ttl: Option<u64>,

    #[arg(
        long,
        global = true,
        value_name = "SPEC",
        action = clap::ArgAction::Append,
        help = "Publish a local port into the lab: REMOTE, REMOTE:LOCAL or REMOTE:HOST:LOCAL"
    )]
    pub forward: Vec<String>,

    #[arg(
        long = "app-arg",
        global = true,
        value_name = "ARG",
        action = clap::ArgAction::Append,
        allow_hyphen_values = true,
        help = "One argument for the app under test, repeatable, passed through verbatim"
    )]
    pub app_args: Vec<String>,

    #[arg(long, global = true, help = "Set DLAB_FORCE=1 on the ssh backend")]
    pub force: bool,

    #[arg(
        long,
        short,
        global = true,
        help = "Suppress human-readable detail on stderr"
    )]
    pub quiet: bool,
}

impl GlobalArgs {
    pub fn overrides(&self) -> Overrides {
        Overrides {
            backend: self.backend.clone(),
            display: self.display.clone(),
            artifacts: self.artifacts.clone(),
            goldens: self.goldens.clone(),
            session_ttl: self.session_ttl,
            forward: self.forward.clone(),
            app_args: self.app_args.clone(),
            force: self.force,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "Check backend reachability, drivers and config resolution")]
    Doctor,

    #[command(about = "Start a lab without waiting for it, never fails the caller")]
    Wake(WakeArgs),

    #[command(about = "Open a live session and keep the driver running")]
    Open(OpenArgs),

    #[command(about = "Send one step to a live session")]
    Act(ActArgs),

    #[command(about = "Snapshot a live session (text by default)")]
    Snap(SnapArgs),

    #[command(about = "Evaluate an expression in a live session")]
    Eval(EvalArgs),

    #[command(about = "Close a live session and release its driver")]
    Close(CloseArgs),

    #[command(about = "Emit a replayable flow from a recorded run")]
    Record(RecordArgs),

    #[command(about = "Place an artifact and replay a flow end to end")]
    Run(RunArgs),

    #[command(about = "Replay flows and compare snapshots against goldens")]
    Verify(VerifyArgs),

    #[command(about = "List recorded runs")]
    Runs(RunsArgs),

    #[command(about = "Show one recorded run")]
    Show(ShowArgs),
}

#[derive(Debug, Args)]
pub struct WakeArgs {
    #[arg(
        long,
        value_name = "NAME",
        help = "Lab to wake, defaults to the backend host"
    )]
    pub lab: Option<String>,

    #[arg(long, default_value_t = 2, value_name = "SECONDS")]
    pub wait: u64,
}

#[derive(Debug, Args)]
pub struct OpenArgs {
    #[arg(
        value_name = "TARGET",
        help = "http://host:3000, exec:/path/to/bin or tui:name"
    )]
    pub target: Option<String>,

    #[arg(long, value_enum)]
    pub surface: Option<Surface>,

    #[arg(long, value_name = "WxH")]
    pub viewport: Option<String>,

    #[arg(long, value_name = "NAME", help = "Flow name recorded in meta.json")]
    pub flow: Option<String>,
}

#[derive(Debug, Args)]
pub struct ActArgs {
    #[arg(value_name = "SESSION")]
    pub session: String,

    #[arg(
        value_name = "STEP",
        num_args = 0..,
        help = "click SELECTOR | type SELECTOR TEXT | key KEY | wait_for SELECTOR | \
                assert_text SELECTOR | open TARGET | snap [NAME]. \
                Put -- before a value that starts with a hyphen"
    )]
    pub step: Vec<String>,

    #[arg(
        long = "yaml",
        value_name = "YAML",
        help = "A raw step, e.g. '{click: \"css=#go\"}'"
    )]
    pub raw: Option<String>,
}

#[derive(Debug, Args)]
pub struct SnapArgs {
    #[arg(value_name = "SESSION")]
    pub session: String,

    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    #[arg(long, value_enum, default_value_t = SnapMode::Text)]
    pub mode: SnapMode,

    #[arg(
        long,
        value_name = "SEL",
        help = "Crop the png to this element, e.g. 'css=#chart'"
    )]
    pub clip: Option<String>,

    #[arg(long, value_name = "PX", help = "Pixels of margin around the crop")]
    pub clip_padding: Option<u32>,

    #[arg(long, value_name = "PX", help = "Grow a crop smaller than this")]
    pub clip_min_side: Option<u32>,
}

#[derive(Debug, Args)]
pub struct EvalArgs {
    #[arg(value_name = "SESSION")]
    pub session: String,

    #[arg(value_name = "EXPR", allow_hyphen_values = true)]
    pub expr: String,
}

#[derive(Debug, Args)]
pub struct CloseArgs {
    #[arg(value_name = "SESSION")]
    pub session: String,

    #[arg(
        long,
        help = "Keep the driver channel directory and its log for debugging"
    )]
    pub keep_channel: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum RecordFormat {
    #[default]
    Uibox,
    Playwright,
}

#[derive(Debug, Args)]
pub struct RecordArgs {
    #[arg(value_name = "SESSION", help = "Session id, or the run id they share")]
    pub id: String,

    #[arg(long, value_enum, default_value_t = RecordFormat::Uibox)]
    pub format: RecordFormat,

    #[arg(
        long = "out",
        short = 'o',
        value_name = "FILE",
        help = "Write here, - for stdout"
    )]
    pub out: Option<PathBuf>,

    #[arg(long, value_name = "NAME")]
    pub flow: Option<String>,

    #[arg(long, value_name = "TARGET")]
    pub target: Option<String>,
}

#[derive(Debug, Args, Clone)]
pub struct RunArgs {
    #[arg(value_name = "FLOW")]
    pub flow: Option<PathBuf>,

    #[arg(
        long,
        value_name = "NAME",
        help = "Build lab holding the checkout under test"
    )]
    pub lab: Option<String>,

    #[arg(long, value_name = "NAME")]
    pub project: Option<String>,

    #[arg(long, value_name = "COMMAND")]
    pub build: Option<String>,

    #[arg(long, value_name = "PATH")]
    pub artifact: Option<PathBuf>,

    #[arg(
        long,
        value_name = "DIR",
        help = "Local tree to sync into the build lab, defaults to the project root"
    )]
    pub source: Option<PathBuf>,

    #[arg(
        long,
        help = "Build from the lab's own checkout instead of syncing a local tree"
    )]
    pub lab_checkout: bool,

    #[arg(long, value_name = "LAB", help = "Lab the artifact is placed into")]
    pub target_lab: Option<String>,

    #[arg(long, value_enum)]
    pub surface: Option<Surface>,

    #[arg(long, value_name = "TARGET")]
    pub target: Option<String>,

    #[arg(long, value_name = "WxH")]
    pub viewport: Option<String>,

    #[arg(
        long,
        help = "Skip the pipeline and replay against the target as it stands"
    )]
    pub no_place: bool,

    #[arg(long, help = "Keep going after a failing step")]
    pub keep_going: bool,
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    #[command(flatten)]
    pub run: RunArgs,

    #[arg(
        long,
        value_name = "GIT_REF",
        help = "Verify only if the tree moved since this ref"
    )]
    pub since: Option<String>,

    #[arg(
        long,
        value_name = "DIR",
        help = "Where flows live, defaults to flows/"
    )]
    pub flows: Option<PathBuf>,

    #[arg(long, help = "Approve every candidate as the new golden")]
    pub update_goldens: bool,

    #[arg(
        long,
        value_name = "PREFIX",
        help = "Golden name prefix, defaults to project/flow"
    )]
    pub golden_prefix: Option<String>,
}

#[derive(Debug, Args)]
pub struct RunsArgs {
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    #[arg(value_name = "RUNID")]
    pub id: String,

    #[arg(long, value_enum, default_value_t = ShowWhat::Meta)]
    pub what: ShowWhat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum ShowWhat {
    Meta,
    Steps,
    Report,
    Snaps,
    All,
}
