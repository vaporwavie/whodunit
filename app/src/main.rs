//! Agent Week: a desktop frontend for pi's agent week (see PROPOSAL.md).
//!
//! `agent-week` opens the chat window and puts its icon in the menu bar while it runs;
//! `agent-week -- --cwd <dir>` runs pi in that folder; `agent-week -- --scoreboard` prints the
//! scoreboard as Markdown and exits. Headless: `--snap out.png --scene fanout`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rune::{App, AppSpec, Chrome, WindowSpec};

use agent_week::config::{self, AppConfig};
use agent_week::core::{Core, Shared, lock};
use agent_week::ui::full::{self, FullUi};
use agent_week::{fixtures, menu_bar, scoreboard};

const HEADLESS_FLAGS: [&str; 6] = [
    "--snap",
    "--tree",
    "--script",
    "--video",
    "--list-windows",
    "--trace",
];

fn flag_value(args: &[String], name: &str) -> Option<String> {
    let index = args.iter().position(|a| a == name)?;
    args.get(index + 1).cloned()
}

fn main() {
    rune_mac::launch();
    rune::prewarm_gpu();
    let args = rune::extra_args();
    if args.iter().any(|a| a == "--scoreboard") {
        print!("{}", scoreboard::markdown(&scoreboard::compute()));
        return;
    }
    let headless = std::env::args().any(|a| HEADLESS_FLAGS.contains(&a.as_str()));

    let config = if headless {
        AppConfig::default()
    } else {
        AppConfig::load()
    };
    let cwd = flag_value(&args, "--cwd")
        .map(PathBuf::from)
        .or_else(|| config.last_cwd.clone().filter(|p| p.is_dir()))
        .unwrap_or_else(config::home);
    let shared: Shared = Arc::new(Mutex::new(Core::new(config, cwd)));
    if !headless {
        lock(&shared).refresh_sessions();
        let shared = shared.clone();
        std::thread::spawn(move || agent_week::core::start_pi(&shared, None));
    }

    let full_spec = AppSpec {
        on_key: Some(full::on_key),
        poll: Some(full::poll),
        setup: Some(full::setup),
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        scenes: &[
            ("chat", |s: &mut FullUi| fixtures::scene_chat(&s.core)),
            ("rate", |s: &mut FullUi| fixtures::scene_rate(&s.core)),
            ("reveal", |s: &mut FullUi| fixtures::scene_reveal(&s.core)),
            ("fanout", |s: &mut FullUi| fixtures::scene_fanout(&s.core)),
            ("run", |s: &mut FullUi| fixtures::scene_run(&s.core)),
            ("guard", |s: &mut FullUi| fixtures::scene_guard(&s.core)),
            ("long", |s: &mut FullUi| fixtures::scene_long(&s.core)),
            ("busy", |s: &mut FullUi| fixtures::scene_busy(&s.core)),
            ("growing", |s: &mut FullUi| fixtures::scene_growing(&s.core)),
            ("board", |s: &mut FullUi| {
                fixtures::scene_chat(&s.core);
                s.board = Some(fixtures::board());
                s.page = full::Page::Scoreboard;
            }),
        ],
        min_size: Some((720.0, 480.0)),
        ..AppSpec::new(
            "Agent Week",
            (1180.0, 780.0),
            || unreachable!("state comes from App::init"),
            full::view,
        )
    };
    let full_app = App::new(full_spec)
        .chrome(Chrome {
            full_size_content: true,
            traffic_lights: Some((16.0, 25.0)),
            drag_regions: true,
        })
        .keep_running();
    let full_handle = full_app.handle();
    let full_core = shared.clone();
    let full_app = full_app.init(move || FullUi::new(full_core));

    rune::run_windows(move |_| {
        if !headless {
            menu_bar::install(move || full_handle.open());
        }
        let windows: Vec<rune::AnyWindowSpec> = vec![WindowSpec::from_app("full", full_app).into()];
        windows
    });
}
