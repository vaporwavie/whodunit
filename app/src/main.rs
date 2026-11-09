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
use whodunit::ui::window::{self, Window};
use whodunit::{fixtures, menu_bar, scoreboard};

const FONTS: [&[u8]; 4] = [
    rune_theme_opencode::FONTS[0],
    rune_theme_opencode::FONTS[1],
    rune_theme_opencode::FONTS[2],
    window::WORDMARK_FONT,
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

    let spec = AppSpec {
        on_key: Some(window::on_key),
        poll: Some(window::poll),
        setup: Some(window::setup),
        fonts: &FONTS,
        families: rune_theme_opencode::FAMILIES,
        scenes: &[
            ("empty", |s: &mut Window| fixtures::scene_empty(&s.core)),
            ("chat", |s: &mut Window| fixtures::scene_chat(&s.core)),
            ("end", |s: &mut Window| fixtures::scene_end(&s.core)),
            ("reveal", |s: &mut Window| fixtures::scene_reveal(&s.core)),
            ("sealing", window::scene_sealing),
            ("fanout", |s: &mut Window| fixtures::scene_fanout(&s.core)),
            ("dealing", |s: &mut Window| fixtures::scene_dealing(&s.core)),
            ("working", |s: &mut Window| fixtures::scene_working(&s.core)),
            ("thinking", |s: &mut Window| {
                fixtures::scene_thinking(&s.core)
            }),
            ("fanning", |s: &mut Window| fixtures::scene_fanning(&s.core)),
            ("fanout-long", |s: &mut Window| {
                fixtures::scene_fanout_long(&s.core)
            }),
            ("fanout-picked", |s: &mut Window| {
                fixtures::scene_fanout_picked(&s.core)
            }),
            ("viewer", |s: &mut Window| {
                fixtures::scene_attach(&s.core);
                s.viewing = Some(fixtures::screenshot());
                s.composer.focused = false;
            }),
            ("attach", |s: &mut Window| {
                fixtures::scene_attach(&s.core);
                s.attachments = vec![fixtures::screenshot(), fixtures::screenshot()];
                s.composer
                    .replace_text("And these two, from the settings page.");
            }),
            ("run", |s: &mut Window| fixtures::scene_run(&s.core)),
            ("tasks", |s: &mut Window| fixtures::scene_tasks(&s.core)),
            ("long", |s: &mut Window| fixtures::scene_long(&s.core)),
            ("wide", |s: &mut Window| fixtures::scene_wide(&s.core)),
            ("busy", |s: &mut Window| fixtures::scene_busy(&s.core)),
            ("slash", |s: &mut Window| {
                fixtures::scene_slash(&s.core);
                s.composer.replace_text("/");
            }),
            ("slash-empty", |s: &mut Window| {
                fixtures::scene_empty(&s.core);
                s.composer.replace_text("/");
            }),
            ("shell", |s: &mut Window| {
                fixtures::scene_chat(&s.core);
                s.composer.replace_text("!cargo test");
            }),
            ("growing", |s: &mut Window| fixtures::scene_growing(&s.core)),
            ("picker", |s: &mut Window| {
                fixtures::scene_chat(&s.core);
                s.picker = fixtures::picker();
                s.composer.focused = false;
            }),
            ("picker-cmd", |s: &mut Window| {
                fixtures::scene_chat(&s.core);
                s.picker = fixtures::picker().holding_cmd();
                s.composer.focused = false;
            }),
            ("board", |s: &mut Window| {
                fixtures::scene_chat(&s.core);
                s.board = Some(fixtures::board());
                s.page = window::Page::Scoreboard;
            }),
        ],
        min_size: Some((720.0, 480.0)),
        ..AppSpec::new(
            "Whodunit",
            (1180.0, 780.0),
            || unreachable!("state comes from App::init"),
            window::view,
        )
    };
    let app = App::new(spec)
        .compositor(whodunit::ui::dither::Waves::default())
        .chrome(Chrome {
            full_size_content: true,
            traffic_lights: Some((16.0, 25.0)),
            drag_regions: true,
        })
        .modifier_keys();
    #[cfg(target_os = "macos")]
    let app = app.keep_running();
    let handle = app.handle();
    let window_core = shared.clone();
    let app = app.init(move || Window::new(window_core));

    rune::run_windows(move |_| {
        if !headless {
            menu_bar::install(move || handle.open());
        }
        let windows: Vec<rune::AnyWindowSpec> = vec![WindowSpec::from_app("main", app).into()];
        windows
    });
}
