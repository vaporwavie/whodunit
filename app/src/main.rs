//! Whodunit: a desktop frontend for the whodunit pi extension (see app/README.md).
//!
//! `whodunit` opens the chat window and puts its icon in the menu bar while it runs;
//! `whodunit -- --cwd <dir>` runs pi in that folder; `whodunit -- --scoreboard` prints the
//! scoreboard as Markdown and exits. Headless: `--snap out.png --scene fanout`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rune::{App, AppSpec, Chrome, WindowSpec};

use whodunit::config::{self, AppConfig};
use whodunit::core::{Core, Leave, Shared, lock};
use whodunit::ui::full::{self, FullUi};
use whodunit::{fixtures, menu_bar, scoreboard};

const FONTS: [&[u8]; 4] = [
    rune_theme_opencode::FONTS[0],
    rune_theme_opencode::FONTS[1],
    rune_theme_opencode::FONTS[2],
    full::WORDMARK_FONT,
];

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
    rune::set_app_id("whodunit");
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
        .or_else(|| config.last_cwd())
        .unwrap_or_else(config::home);
    let shared: Shared = Arc::new(Mutex::new(Core::new(config, cwd)));
    if !headless {
        let mut core = lock(&shared);
        core.refresh_sessions();
        core.request_leave(Leave::New);
        drop(core);
        let shared = shared.clone();
        std::thread::spawn(move || whodunit::core::start_unstarted(&shared));
    }

    let full_spec = AppSpec {
        on_key: Some(full::on_key),
        poll: Some(full::poll),
        setup: Some(full::setup),
        fonts: &FONTS,
        families: rune_theme_opencode::FAMILIES,
        scenes: &[
            ("empty", |s: &mut FullUi| fixtures::scene_empty(&s.core)),
            ("chat", |s: &mut FullUi| fixtures::scene_chat(&s.core)),
            ("end", |s: &mut FullUi| fixtures::scene_end(&s.core)),
            ("reveal", |s: &mut FullUi| fixtures::scene_reveal(&s.core)),
            ("fanout", |s: &mut FullUi| fixtures::scene_fanout(&s.core)),
            ("run", |s: &mut FullUi| fixtures::scene_run(&s.core)),
            ("tasks", |s: &mut FullUi| fixtures::scene_tasks(&s.core)),
            ("long", |s: &mut FullUi| fixtures::scene_long(&s.core)),
            ("wide", |s: &mut FullUi| fixtures::scene_wide(&s.core)),
            ("busy", |s: &mut FullUi| fixtures::scene_busy(&s.core)),
            ("slash", |s: &mut FullUi| {
                fixtures::scene_slash(&s.core);
                s.composer.replace_text("/");
            }),
            ("slash-empty", |s: &mut FullUi| {
                fixtures::scene_empty(&s.core);
                s.composer.replace_text("/");
            }),
            ("shell", |s: &mut FullUi| {
                fixtures::scene_chat(&s.core);
                s.composer.replace_text("!cargo test");
            }),
            ("growing", |s: &mut FullUi| fixtures::scene_growing(&s.core)),
            ("picker", |s: &mut FullUi| {
                fixtures::scene_chat(&s.core);
                s.picker = fixtures::picker();
                s.composer.focused = false;
            }),
            ("picker-cmd", |s: &mut FullUi| {
                fixtures::scene_chat(&s.core);
                s.picker = fixtures::picker().holding_cmd();
                s.composer.focused = false;
            }),
            ("board", |s: &mut FullUi| {
                fixtures::scene_chat(&s.core);
                s.board = Some(fixtures::board());
                s.page = full::Page::Scoreboard;
            }),
        ],
        min_size: Some((720.0, 480.0)),
        ..AppSpec::new(
            "Whodunit",
            (1180.0, 780.0),
            || unreachable!("state comes from App::init"),
            full::view,
        )
    };
    let full_app = App::new(full_spec)
        .compositor(whodunit::ui::dither::Waves::default())
        .chrome(Chrome {
            full_size_content: true,
            traffic_lights: Some((16.0, 25.0)),
            drag_regions: true,
        })
        .modifier_keys();
    #[cfg(target_os = "macos")]
    let full_app = full_app.keep_running();
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
