use super::*;
use crate::transcript::{DEAL, DONE, FANOUT_VIEW};

fn core() -> Core {
    Core::new(AppConfig::default(), PathBuf::from("/project"))
}

#[test]
fn folderless_chats_have_isolated_workspaces_until_the_app_exits() {
    let mut core = core();
    core.request_leave(Leave::Temporary);
    let first = core.cwd.clone();
    assert!(config::is_temporary(&first));
    assert!(first.is_dir());
    assert_eq!(config::short_path(&first), "No folder");
    std::fs::write(first.join("scratch.txt"), "keep while switching").unwrap();
    core.config.remember(&first);
    assert!(core.config.recent.is_empty());

    core.request_leave(Leave::New);
    let second = core.cwd.clone();
    assert_ne!(first, second);
    assert!(second.is_dir());
    assert!(!second.join("scratch.txt").exists());
    assert!(first.join("scratch.txt").exists());

    core.request_leave(Leave::NewIn("/project".into()));
    core.request_leave(Leave::New);
    assert_eq!(core.cwd, Path::new("/project"));
    assert!(first.is_dir() && second.is_dir());
    drop(core);
    assert!(!first.exists() && !second.exists());
    assert!(!config::is_temporary(Path::new("/project")));
}

#[test]
fn saved_folderless_chats_recreate_their_workspace_without_deleting_history() {
    let mut original = core();
    original.request_leave(Leave::Temporary);
    let cwd = original.cwd.clone();
    std::fs::write(cwd.join("scratch.txt"), "disposable").unwrap();
    let history = tempfile::NamedTempFile::new().unwrap();
    let summary = Summary {
        path: history.path().to_owned(),
        cwd: cwd.clone(),
        title: "A saved chat".into(),
        week: Default::default(),
        updated: std::time::SystemTime::now(),
    };
    assert_eq!(summary.subtitle(), "No folder");
    drop(original);
    assert!(!cwd.exists());

    let mut reopened = core();
    reopened.sessions.push(summary);
    reopened.request_leave(Leave::Open(history.path().to_owned()));
    assert_eq!(reopened.cwd, cwd);
    assert!(cwd.is_dir());
    assert!(!cwd.join("scratch.txt").exists());
    assert!(reopened.exited.is_none());
    drop(reopened);
    assert!(!cwd.exists());
    assert!(history.path().exists());
}

#[test]
fn the_welcome_does_not_return_between_dealing_and_the_first_message() {
    use crate::ui::window::{self, Window};
    use rune::{Env, Runtime};

    let shared = Arc::new(Mutex::new(core()));
    let mut rt = Runtime::new(
        Window::new(shared.clone()),
        window::view,
        (1180.0, 780.0),
        Env::new(true, 1.0),
    );
    rt.poll = Some(window::poll);
    rt.frame(0.0);
    assert!(
        rt.elements()
            .iter()
            .any(|e| e.key.as_deref() == Some("dither"))
    );

    let shot = crate::fixtures::screenshot();
    lock(&shared).submit_with("Check this screen", false, std::slice::from_ref(&shot));
    rt.frame(0.016);
    rt.frame(1.0); // The welcome's one exit fade has finished before classification returns.

    let message = json!({"role": "user", "content": [
        {"type": "text", "text": "Check this screen"}, shot.to_block()
    ]});
    let records = [
        json!({"type": "entry_appended", "entry": {
            "type": "custom", "customType": DEAL,
            "data": {"role": "builder", "size": "S", "agent": "secret/model"}
        }}),
        json!({"type": "agent_start"}),
        json!({"type": "message_start", "message": message}),
        json!({"type": "message_end", "message": message}),
    ];
    for record in records {
        let kind = str_of(&record, "type");
        lock(&shared).apply(record);
        rt.frame(0.016);
        let elements = rt.elements();
        assert!(
            !elements
                .iter()
                .any(|e| matches!(e.key.as_deref(), Some("dither" | "empty-intro"))),
            "the welcome returned after {kind}"
        );
        assert!(
            elements
                .iter()
                .any(|e| e.text.as_deref() == Some("Check this screen")),
            "the prompt disappeared after {kind}"
        );
        assert!(lock(&shared).reveal().is_none());
    }
    let core = lock(&shared);
    assert!(core.pending_prompt.is_none());
    assert!(core.pending_shots.is_empty());
    assert!(
        matches!(core.t.items.as_slice(), [Item::User { text, shots }]
        if text == "Check this screen" && shots == &[shot])
    );
}

#[test]
fn cancelling_a_deal_restores_the_prompt_and_retires_its_process_generation() {
    let mut core = core();
    core.request_leave(Leave::New);
    let background = core.task_id();
    core.streaming = true;
    core.session_file = Some("/background.jsonl".into());
    core.t.first_prompt = Some("Another task".into());
    core.request_leave(Leave::New);
    let cancelled = core.task_id();
    core.session_file = Some("/not-written-yet.jsonl".into());
    let shot = crate::fixtures::screenshot();
    core.submit_with("Check this screen", false, std::slice::from_ref(&shot));
    assert!(core.can_stop());
    assert!(!core.streaming);
    assert_eq!(core.sessions.len(), 2);
    assert!(!core.outbox.is_empty());

    let restored = core.cancel_deal().unwrap();
    assert_eq!(restored, ("Check this screen".into(), vec![shot.clone()]));
    assert_ne!(core.task_id(), cancelled);
    assert_eq!(core.cwd, Path::new("/project"));
    assert!(!core.working());
    assert!(!core.can_stop());
    assert!(core.pending_prompt.is_none() && core.pending_shots.is_empty());
    assert!(core.pending.is_empty() && core.outbox.is_empty());
    assert!(core.dealing_since.is_none());
    assert!(core.session_file.is_none());
    assert_eq!(core.unstarted, [background, core.task_id()]);
    assert_eq!(core.sessions.len(), 1);
    assert!(core.tasks[&background].streaming);
    assert!(!core.tasks.contains_key(&cancelled));
    core.for_task(cancelled, |_| {
        panic!("late deal or exit must not reach the new task")
    });
    assert!(core.cancel_deal().is_none(), "stopping twice is a no-op");

    core.submit_with(&restored.0, false, &restored.1);
    assert!(core.dealing());
    assert_eq!(core.outbox.len(), 1);
    assert_eq!(core.outbox[0]["message"], "Check this screen");
    assert_eq!(Shot::from_block(&core.outbox[0]["images"][0]), Some(shot));
}

#[test]
fn cancelling_a_deal_never_discards_a_dealt_agent_or_a_fanout() {
    let mut core = core();
    core.submit("A task", false);
    core.t.week.deal = Some(crate::transcript::Deal::default());
    assert!(core.cancel_deal().is_none());
    core.streaming = true;
    assert!(core.can_stop());
    core.abort();
    assert_eq!(core.outbox.last().unwrap()["type"], "abort");
    assert!(core.t.week.deal.is_some());

    core.streaming = false;
    core.t.week.deal = None;
    core.widgets
        .insert("fanout".into(), vec!["fan-out · A running".into()]);
    assert!(!core.can_stop());
    assert!(core.cancel_deal().is_none());
    core.widgets.clear();
    core.apply(json!({"type": "extension_ui_request", "id": "role", "method": "select", "title": "Which role?", "options": ["builder"]}));
    assert!(core.cancel_deal().is_none());
    assert!(core.dialog.is_some());
}

#[test]
fn classification_and_fanouts_are_listed_before_a_session_file_exists() {
    let mut core = core();
    core.task.generation = 1;
    core.session_file = Some("/not-written-yet.jsonl".into());
    core.pending_prompt = Some("Research a task".into());
    core.sync_live_sessions();
    assert_eq!(core.sessions.len(), 1);
    assert_eq!(core.session_state(&core.sessions[0]), SessionState::Working);
    assert_eq!(core.sessions[0].subtitle(), "project");

    core.task.pending_prompt = None;
    core.widgets
        .insert("fanout".into(), vec!["Running 3 answers".into()]);
    assert_eq!(core.session_state(&core.sessions[0]), SessionState::Working);
    core.apply(json!({"type": "entry_appended", "entry": {
        "type": "custom", "customType": FANOUT_VIEW,
        "data": {"role": "scout", "answers": []}
    }}));
    core.widgets.clear();
    core.apply(json!({"type": "extension_ui_request", "id": "pick", "method": "select", "title": "Which answer is best?", "options": ["A", "B"]}));
    assert_eq!(core.session_state(&core.sessions[0]), SessionState::Idle);
    assert!(core.tasks_waiting(&core.sessions[0]));
    assert!(core.reveal().is_none());
}

#[test]
fn background_records_update_only_their_task_and_unknown_processes_are_ignored() {
    let mut core = core();
    core.task.generation = 2;
    core.task.session_file = Some("/selected.jsonl".into());
    core.task.pending_prompt = Some("Selected task".into());
    core.tasks.insert(
        1,
        Task {
            generation: 1,
            session_file: Some("/background.jsonl".into()),
            cwd: PathBuf::from("/elsewhere"),
            pending_prompt: Some("Background task".into()),
            ..Task::default()
        },
    );
    core.sync_live_sessions();
    core.for_task(1, |core| {
        core.t.first_prompt = Some("Background task".into());
        core.apply(json!({"type": "agent_start"}));
        core.apply(json!({"type": "queue_update", "steering": ["background steer"], "followUp": ["background follow-up"]}));
        core.apply(json!({"type": "extension_ui_request", "method": "set_editor_text", "text": "background draft"}));
        core.apply(json!({"type": "entry_appended", "entry": {
            "type": "custom", "customType": DEAL,
            "data": {"role": "builder", "size": "M", "agent": "secret/model"}
        }}));
        core.apply(json!({"type": "extension_ui_request", "id": "question", "method": "input", "title": "Need input"}));
    });
    assert_eq!(core.task_id(), 2);
    assert!(!core.streaming);
    assert!(core.dialog.is_none());
    assert!(core.t.week.deal.is_none());
    assert!(core.queue.0.is_empty() && core.queue.1.is_empty());
    assert!(core.editor_text.is_none());
    assert_eq!(core.tasks[&1].queue.1, ["background follow-up"]);
    assert_eq!(
        core.tasks[&1].editor_text.as_ref().unwrap().1,
        "background draft"
    );
    assert!(core.tasks[&1].streaming);
    assert!(core.tasks[&1].turn_started.is_some());
    assert_eq!(core.tasks[&1].dialog.as_ref().unwrap().id, "question");
    core.for_task(999, |_| panic!("obsolete process must not apply records"));
    core.for_task(1, |core| {
        core.apply(json!({"type": "entry_appended", "entry": {
            "type": "custom", "customType": DONE, "data": {"score": 4}
        }}))
    });
    assert!(core.reveal().is_none());
    let background = core
        .sessions
        .iter()
        .find(|s| s.path == Path::new("/background.jsonl"))
        .unwrap();
    assert_eq!(core.session_state(background), SessionState::Ended);
    assert_eq!(background.subtitle(), "elsewhere · M · 4/5");
    assert_eq!(
        background.revealed().map(|d| d.role.as_str()),
        Some("builder")
    );
}

#[test]
fn parking_keeps_a_dealt_task_even_before_its_first_message_and_drops_unused_chats() {
    let mut core = core();
    let mut dealt = Task {
        generation: 1,
        ready: true,
        ..Task::default()
    };
    dealt.t.week.deal = Some(crate::transcript::Deal::default());
    core.park_task(dealt);
    assert!(core.tasks.contains_key(&1));
    core.park_task(Task {
        generation: 2,
        ready: true,
        session_file: Some("/unused.jsonl".into()),
        ..Task::default()
    });
    assert!(!core.tasks.contains_key(&2));
}

#[test]
fn navigation_neither_ends_nor_aborts_a_working_task() {
    let mut core = core();
    let working = core.open_task(
        "/project".into(),
        Some(Path::new("/current.jsonl")),
        Transcript::default(),
    );
    core.streaming = true;
    core.request_leave(Leave::New);
    assert_ne!(core.task_id(), working, "the new task shows at once");
    assert!(core.t.items.is_empty());
    assert_eq!(core.unstarted, [core.task_id()]);
    assert!(core.tasks[&working].streaming && core.tasks[&working].dialog.is_none());
    core.request_leave(Leave::Open(PathBuf::from("/current.jsonl")));
    assert_eq!(core.task_id(), working, "back to the same process");
    assert!(core.streaming);
    core.request_leave(Leave::Open(PathBuf::from("/current.jsonl")));
    assert_eq!(
        core.task_id(),
        working,
        "selecting the current chat is a no-op"
    );
    assert_eq!(core.unstarted.len(), 1);
}

fn stats_response(core: &mut Core, usage: Value) {
    core.pending.insert("stats".into(), Pending::Stats);
    core.apply(json!({"type": "response", "id": "stats", "command": "get_session_stats", "success": true, "data": {"cost": 0.1, "contextUsage": usage}}));
}

fn asks_for_stats(core: &Core) -> bool {
    core.pending.values().any(|p| matches!(p, Pending::Stats))
}

#[test]
fn the_context_meter_reads_against_compaction_and_follows_the_turn() {
    let mut core = core();
    core.pi = None;
    stats_response(
        &mut core,
        json!({"tokens": 128_000, "contextWindow": 272_000, "percent": 47}),
    );
    let limit = 272_000.0 - core.stats.reserve.unwrap_or(0.0);
    let percent = core.stats.context_percent().unwrap();
    assert!((percent - 128_000.0 / limit * 100.0).abs() < 1e-9);

    // Each response and each batch of tool results asks again, without waiting for the end.
    for record in [
        json!({"type": "message_end", "message": {"role": "assistant", "content": []}}),
        json!({"type": "turn_end"}),
    ] {
        core.pending.clear();
        core.apply(record);
        assert!(asks_for_stats(&core));
    }

    // After a compaction pi counts nothing until the next response; its estimate stands in.
    core.pending.clear();
    core.apply(json!({"type": "compaction_end", "reason": "threshold", "result": {"estimatedTokensAfter": 32_000}, "aborted": false}));
    assert!(asks_for_stats(&core));
    assert_eq!(core.stats.context_tokens, Some(32_000.0));
    assert!(core.stats.predicted);
    stats_response(
        &mut core,
        json!({"tokens": null, "contextWindow": 272_000, "percent": null}),
    );
    assert_eq!(
        core.stats.context_tokens,
        Some(32_000.0),
        "the estimate holds"
    );
    stats_response(
        &mut core,
        json!({"tokens": 41_000, "contextWindow": 272_000, "percent": 15}),
    );
    assert_eq!(core.stats.context_tokens, Some(41_000.0));
    assert!(!core.stats.predicted, "pi's count replaces the estimate");
}

#[test]
fn compaction_settings_layer_like_pi() {
    let dir = std::env::temp_dir().join(format!("whodunit-settings-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".pi")).unwrap();
    let settings = dir.join(".pi/settings.json");
    std::fs::write(&settings, r#"{"compaction": {"reserveTokens": 1000, "modelOverrides": {"whodunit/auto": {"reserveTokens": 2000}}}}"#).unwrap();
    assert_eq!(
        config::compaction_reserve(&dir),
        Some(2000.0),
        "the override for the selection wins"
    );
    std::fs::write(&settings, r#"{"compaction": {"enabled": false}}"#).unwrap();
    assert_eq!(config::compaction_reserve(&dir), None);
    let stats = Stats {
        context_tokens: Some(68_000.0),
        context_window: Some(272_000.0),
        ..Stats::default()
    };
    assert_eq!(
        stats.context_percent(),
        Some(25.0),
        "with it off, the whole window"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn images_go_with_prompts_but_not_commands() {
    let mut core = core();
    let shot = crate::fixtures::screenshot();
    let shots = std::slice::from_ref(&shot);
    assert!(!core.submit_with("/name Tidy up", false, shots));
    assert!(core.outbox.last().unwrap().get("images").is_none());
    assert!(core.submit_with("What's off here?", false, shots));
    let record = core.outbox.last().unwrap();
    assert_eq!(record["images"][0]["mimeType"], "image/png");
    assert_eq!(
        crate::transcript::Shot::from_block(&record["images"][0]).as_ref(),
        Some(&shot),
        "base64 round-trips"
    );
    assert_eq!(core.pending_shots, std::slice::from_ref(&shot));

    // A fan-out's prompt and images come back with its view.
    core.apply(json!({"type": "entry_appended", "entry": {
        "type": "custom", "customType": FANOUT_VIEW,
        "data": {"role": "scout", "prompt": "What's off here?", "images": 1, "answers": []}
    }}));
    let Some(Item::Fanout { prompt, shots, .. }) = core.t.items.last() else {
        panic!("a fan-out");
    };
    assert_eq!(prompt.as_deref(), Some("What's off here?"));
    assert_eq!(shots, std::slice::from_ref(&shot));
}

#[test]
fn a_saved_fanout_reads_its_images_from_the_pick() {
    let shot = crate::fixtures::screenshot();
    let entries = [
        json!({"type": "custom", "id": "1", "parentId": null, "customType": FANOUT_VIEW, "data": {"role": "scout", "answers": []}}),
        json!({"type": "custom_message", "id": "2", "parentId": "1", "customType": crate::transcript::FANOUT_PICK, "display": false, "content": [
            {"type": "text", "text": "User request:\n\nWhat's off here?"},
            shot.to_block(),
            {"type": "text", "text": "\n\nYour answer:\n\nThe padding."},
        ]}),
    ];
    let t = Transcript::from_entries(&entries, Some("2"));
    let Some(Item::Fanout { prompt, shots, .. }) = t.items.first() else {
        panic!("a fan-out");
    };
    assert_eq!(prompt.as_deref(), Some("What's off here?"));
    assert_eq!(shots, &[shot]);
}

#[test]
fn image_sizes_come_from_their_headers() {
    use crate::transcript::Shot;
    assert_eq!(crate::fixtures::screenshot().dimensions(), Some((240, 160)));
    let gif = [b"GIF89a".as_slice(), &[0x40, 0x01, 0xF0, 0x00]].concat();
    assert_eq!(
        Shot::from_bytes(gif).unwrap().dimensions(),
        Some((320, 240))
    );
    // A JFIF segment to skip, then the baseline frame: 1080 high, 1920 wide.
    let jpeg = [
        &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00][..],
        &[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x04, 0x38, 0x07, 0x80],
    ]
    .concat();
    assert_eq!(
        Shot::from_bytes(jpeg).unwrap().dimensions(),
        Some((1920, 1080))
    );
    let mut webp = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
    webp.extend([0; 8]);
    webp.extend([0x7F, 0x07, 0x00, 0x37, 0x04, 0x00]); // 1919 + 1, 1079 + 1
    assert_eq!(
        Shot::from_bytes(webp).unwrap().dimensions(),
        Some((1920, 1080))
    );
}

#[test]
fn a_last_updated_label_lasts_until_it_reads_differently() {
    use crate::folders::{ago, ago_lasts};
    use std::time::{Duration, SystemTime};
    let at = SystemTime::UNIX_EPOCH;
    for secs in [
        0, 59, 60, 61, 3_599, 3_600, 86_399, 90_000, 1_209_599, 2_000_000,
    ] {
        let now = at + Duration::from_secs(secs) + Duration::from_millis(250);
        let lasts = ago_lasts(at, now);
        assert!(lasts > Duration::ZERO, "{secs}s");
        let label = ago(at, now);
        assert_eq!(
            ago(at, now + lasts - Duration::from_millis(1)),
            label,
            "{secs}s"
        );
        assert_ne!(ago(at, now + lasts), label, "{secs}s");
    }
}
