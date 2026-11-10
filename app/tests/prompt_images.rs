use serde_json::{Value, json};
use whodunit::transcript::{Block, DEAL, Item, Shot, Transcript, content_text};

const RESIZE_NOTE: &str = "[Image: original 2880x972, displayed at 2000x675. Multiply coordinates by 1.44 to map to original image.]";

fn user(text: &str, images: usize) -> Value {
    let mut content = vec![json!({"type": "text", "text": text})];
    content.extend((0..images).map(|_| whodunit::fixtures::screenshot().to_block()));
    json!({"role": "user", "content": content})
}

fn assert_prompt(t: &Transcript, expected: &str, images: usize) {
    let [Item::User { text, shots }] = t.items.as_slice() else {
        panic!("expected one user message: {:?}", t.items);
    };
    assert_eq!(text, expected);
    assert_eq!(t.first_prompt.as_deref(), Some(expected));
    assert_eq!(shots, &vec![whodunit::fixtures::screenshot(); images]);
}

#[test]
fn resize_notes_stay_in_model_content_but_not_the_transcript() {
    for (prompt, images, notes) in [
        ("Check this screen", 1, RESIZE_NOTE.to_owned()),
        ("", 1, RESIZE_NOTE.to_owned()),
        (
            "Compare these",
            2,
            format!(
                "{RESIZE_NOTE}\n[Image: original 4096x2048, displayed at 2000x1000. Multiply coordinates by 2.05 to map to original image.]"
            ),
        ),
        ("  Keep my spacing\n\n", 1, RESIZE_NOTE.to_owned()),
    ] {
        let raw = format!("{prompt}\n\n{notes}");
        let message = user(&raw, images);
        let mut t = Transcript::default();
        t.add_message(&message);
        assert_prompt(&t, prompt, images);
        assert_eq!(content_text(&message["content"]), raw);
        assert_eq!(
            Shot::from_block(&message["content"][1]),
            Some(whodunit::fixtures::screenshot())
        );
    }
}

#[test]
fn ordinary_text_and_image_errors_are_not_resize_metadata() {
    for (text, images) in [
        ("Check this screen".to_owned(), 1),
        (format!("Literal example:\n\n{RESIZE_NOTE}"), 0),
        (format!("Explain this:\n\n```text\n{RESIZE_NOTE}\n```"), 1),
        (format!("{RESIZE_NOTE}\n\nWhy does this appear?"), 1),
        (format!("An inline example: {RESIZE_NOTE}"), 1),
        (
            format!("Check this\n\n{}", RESIZE_NOTE.replace("1.44", "some amount")),
            1,
        ),
        ("Check this\n\n[Image: a user-written caption.]".to_owned(), 1),
        (
            "Check this\n\n[Image omitted: could not be resized below the inline image size limit.]".to_owned(),
            0,
        ),
    ] {
        let mut t = Transcript::default();
        t.add_message(&user(&text, images));
        assert_prompt(&t, &text, images);
    }
    let text = format!("Literal example:\n\n{RESIZE_NOTE}");
    let mut t = Transcript::default();
    t.add_message(&json!({"role": "user", "content": text}));
    assert_prompt(&t, &text, 0);
}

#[test]
fn assistant_text_and_tool_results_are_unchanged() {
    let mut t = Transcript::default();
    let mut message = user(&format!("Explanation:\n\n{RESIZE_NOTE}"), 1);
    message["role"] = json!("assistant");
    t.add_message(&message);
    let [Item::Assistant { blocks, .. }] = t.items.as_slice() else {
        panic!("expected one assistant message");
    };
    assert!(
        matches!(&blocks[0], Some(Block::Text(text)) if text == &format!("Explanation:\n\n{RESIZE_NOTE}"))
    );
    message["role"] = json!("toolResult");
    message["toolCallId"] = json!("read-image");
    t.add_message(&message);
    assert_eq!(
        t.tools["read-image"].output,
        format!("Explanation:\n\n{RESIZE_NOTE}")
    );
}

#[test]
fn reopened_chats_and_sidebar_titles_hide_resize_notes() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("--project--");
    std::fs::create_dir(&folder).unwrap();
    for prompt in ["Check this screen", ""] {
        let path = folder.join("chat.jsonl");
        let entries = [
            json!({"type": "session", "cwd": "/project"}),
            json!({"type": "custom", "id": "deal", "parentId": null, "customType": DEAL,
                "data": {"role": "builder", "size": "S", "agent": "secret/model"}}),
            json!({"type": "message", "id": "prompt", "parentId": "deal",
                "message": user(&format!("{prompt}\n\n{RESIZE_NOTE}"), 1)}),
        ];
        std::fs::write(
            &path,
            entries
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let t = whodunit::sessions::transcript(&path).unwrap();
        assert_prompt(&t, prompt, 1);
        let expected_title = if prompt.is_empty() {
            "Untitled"
        } else {
            prompt
        };
        assert_eq!(t.title().as_deref(), Some(expected_title));
        let summaries = whodunit::sessions::list(root.path());
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].title, expected_title);
    }
}
