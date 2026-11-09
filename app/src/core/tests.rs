use super::*;
use crate::transcript::{DEAL, DONE, FANOUT_VIEW};

fn core() -> Core {
    Core::new(AppConfig::default(), PathBuf::from("/project"))
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
    assert_eq!(background.subtitle(), "elsewhere · builder · M · 4/5");
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
    core.streaming = true;
    core.session_file = Some("/current.jsonl".into());
    core.request_leave(Leave::New);
    assert!(core.streaming);
    assert!(core.dialog.is_none());
    assert!(matches!(core.relaunch, Some((_, None))));
    core.request_leave(Leave::Open(PathBuf::from("/current.jsonl")));
    assert!(
        matches!(core.relaunch, Some((_, None))),
        "selecting the current chat is a no-op"
    );
}
