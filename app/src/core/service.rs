//! The service uses the same reducer as the UI. Attach takes a snapshot and subscribes
//! under one lock, then streams commands and records in their original order.

use super::*;
use crate::service::{self, Event};

impl Core {
    pub(crate) fn publish(&mut self, event: Event) {
        if self.subscribers.is_empty() {
            return;
        }
        let bytes = Arc::new(service::encode(&event).expect("service event is JSON"));
        // A stalled UI must never backpressure pi. Its writer closes the connection after
        // draining the bounded queue; reopening attaches from a fresh snapshot.
        self.subscribers
            .retain(|s| s.sender.try_send(bytes.clone()).is_ok());
    }

    pub(crate) fn process_exit(&mut self, stderr: String) {
        self.pi = None;
        self.streaming = false;
        self.turn_started = None;
        self.pending_prompt = None;
        let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
        let error = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
        self.exited = Some(error.clone());
        self.touch();
        self.publish(Event::Exit(error));
    }

    /// Outbound commands matter too: input hooks can wait before echoing a user message,
    /// and bash begins in the UI rather than with a pi message_start event.
    fn remember_command(&mut self, record: &Value) {
        let id = str_of(record, "id");
        if record["type"] == "extension_ui_response" {
            if self.dialog.as_ref().is_some_and(|d| d.id == id) {
                self.dialog = None;
            }
            return;
        }
        if self.pending.contains_key(&id) {
            return;
        }
        let pending = Pending::of(record);
        match pending {
            Pending::Prompt => {
                let text = str_of(record, "message");
                if self.t.week.deal.is_none() && !self.t.has_answer() && !text.starts_with('/') {
                    self.pending_prompt = Some(text);
                    self.pending_shots = record["images"]
                        .as_array()
                        .map(|images| images.iter().filter_map(Shot::from_block).collect())
                        .unwrap_or_default();
                    self.dealing_since = Some(Instant::now());
                }
            }
            Pending::Bash => self.t.items.push(Item::Bash {
                command: str_of(record, "command"),
                output: String::new(),
                exit_code: None,
                running: true,
            }),
            _ => {}
        }
        self.pending.insert(id, pending);
    }

    pub(crate) fn service_command(&mut self, record: Value) {
        if record["type"] == "extension_ui_response"
            && !self
                .dialog
                .as_ref()
                .is_some_and(|dialog| record["id"] == dialog.id)
        {
            return;
        }
        self.remember_command(&record);
        let sent = match &self.pi {
            Some(pi) => pi.send(&record),
            None if self.exited.is_none() => {
                self.outbox.push(record.clone());
                true
            }
            None => false,
        };
        if sent {
            self.publish(Event::Sent(record));
            self.touch();
        } else {
            self.process_exit("pi is no longer running in this chat.".into());
        }
    }

    pub(crate) fn stop_hosted(&mut self) {
        self.unstarted.clear();
        self.outbox.clear();
        if self.pi.is_none() {
            self.process_exit("Background service stopped.".into());
        } else {
            self.pi = None;
        }
    }

    pub(crate) fn service_kill(&mut self) -> std::io::Result<()> {
        if let Some(pi) = &mut self.pi {
            pi.kill()?;
        }
        self.unstarted.clear();
        self.process_exit("Dealing stopped.".into());
        Ok(())
    }

    pub(crate) fn hosted(
        config: AppConfig,
        cwd: PathBuf,
        session: Option<PathBuf>,
        id: u64,
    ) -> Shared {
        let mut core = Self::new(config, cwd.clone());
        let transcript = session
            .as_deref()
            .and_then(sessions::transcript)
            .unwrap_or_default();
        let generation = core.open_task(cwd, session.as_deref(), transcript);
        core.unstarted.push(generation);
        core.service_id = Some(id);
        Arc::new(Mutex::new(core))
    }

    pub(crate) fn disposable(&self) -> bool {
        self.subscribers.is_empty()
            && (self.exited.is_some()
                || (self.t.week.done.is_some()
                    && !self.streaming
                    && self.pending.is_empty()
                    && self.dialog.is_none()
                    && self.statuses.is_empty()
                    && self.widgets.is_empty()
                    && self
                        .session_file
                        .as_ref()
                        .is_some_and(|file| Path::new(file).is_file())))
    }

    pub(crate) fn recoverable(&self) -> bool {
        self.working()
            || self.dialog.is_some()
            || !self.t.items.is_empty()
            || self.t.week.deal.is_some()
            || self.t.title().is_some()
    }

    fn remote_event(&mut self, event: Event) {
        match event {
            Event::Sent(record) => {
                self.remember_command(&record);
                self.sync_live_sessions();
                self.touch();
            }
            Event::Record(record) => self.apply(record),
            Event::Exit(error) => self.process_exit(error),
        }
    }
}

/// Prepare and attach without a window. An explicit --cwd still opens a new chat;
/// otherwise select the newest recovered one, including chats without a session file.
pub fn connect_background(shared: &Shared, cwd: Option<PathBuf>) {
    prepare_background(shared, cwd);
    start_unstarted(shared);
}

/// Lists the service before the first window exists, so input cannot land in a placeholder
/// chat that recovery then replaces. Starting pi (including the login shell) stays off-thread.
pub fn prepare_background(shared: &Shared, cwd: Option<PathBuf>) {
    let result = service::ensure_started().and_then(|()| service::list());
    let sessions = match result {
        Ok(sessions) => sessions,
        Err(error) => {
            let mut core = lock(shared);
            core.exited = Some(format!(
                "Couldn’t connect to the background service: {error}"
            ));
            core.touch();
            let wakers = core.wakers();
            drop(core);
            wakers.iter().for_each(|wake| wake());
            return;
        }
    };
    {
        let mut core = lock(shared);
        core.refresh_sessions();
        for session in sessions {
            let transcript = session
                .session
                .as_deref()
                .and_then(sessions::transcript)
                .unwrap_or_default();
            let generation = core.open_task(session.cwd, session.session.as_deref(), transcript);
            core.service_id = Some(session.id);
            core.unstarted.push(generation);
            // A chat can be pre-file and waiting on Jev. Keep it until its snapshot arrives.
            core.ready = false;
        }
        if cwd.is_some() || core.service_id.is_none() {
            core.request_leave(cwd.map_or(Leave::Temporary, Leave::NewIn));
        }
    }
}

pub(super) fn start_remote(shared: &Shared, generation: u64) {
    let (id, cwd, session, bin) = {
        let core = lock(shared);
        let Some(task) = core
            .tasks
            .values()
            .chain(std::iter::once(&core.task))
            .find(|task| task.generation == generation)
        else {
            return;
        };
        (
            task.service_id,
            task.cwd.clone(),
            task.session_file.clone(),
            core.config.pi_bin.clone(),
        )
    };
    let for_records = Arc::downgrade(shared);
    let for_exit = Arc::downgrade(shared);
    let attached = service::attach(id, cwd, session.map(PathBuf::from), bin);
    let mut core = lock(shared);
    core.for_task(generation, |core| match attached {
        Ok((connection, snapshot, reader)) => {
            let outbox = std::mem::take(&mut core.outbox);
            let loads = core.loads + 1;
            core.task = snapshot;
            core.generation = generation;
            core.loads = loads;
            core.pi = Some(Pi::Remote(connection));
            core.config.remember(&core.cwd.clone());
            for record in outbox {
                core.remember_command(&record);
                if !core.pi.as_ref().unwrap().send(&record) {
                    core.process_exit("Lost the background service connection. Reopen the app to reconnect.".into());
                    break;
                }
            }
            service::read_events(reader, move |event| {
                let Some(shared) = for_records.upgrade() else { return };
                let wakers = {
                    let mut core = lock(&shared);
                    core.for_task(generation, |core| core.remote_event(event));
                    core.wakers()
                };
                wakers.iter().for_each(|wake| wake());
            }, move || {
                let Some(shared) = for_exit.upgrade() else { return };
                let wakers = {
                    let mut core = lock(&shared);
                    core.for_task(generation, |core| {
                        if core.exited.is_none() {
                            core.process_exit("Lost the background service connection. Reopen the app to reconnect; work may still be running.".into());
                        }
                    });
                    core.wakers()
                };
                wakers.iter().for_each(|wake| wake());
            });
        }
        Err(error) => {
            core.outbox.clear();
            core.pending_prompt = None;
            core.exited = Some(format!("Couldn’t attach to pi: {error}"));
        }
    });
    core.touch();
    let wakers = core.wakers();
    drop(core);
    wakers.iter().for_each(|wake| wake());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_slow_client_is_disconnected_without_stalling_the_reducer() {
        let mut core = Core::new(AppConfig::default(), PathBuf::from("/project"));
        let (sender, receiver) = mpsc::sync_channel(1);
        core.subscribers.push(service::Subscriber { id: 1, sender });
        core.publish(Event::Record(json!({"type": "agent_start"})));
        core.publish(Event::Record(json!({"type": "agent_settled"})));
        assert!(core.subscribers.is_empty());
        assert!(receiver.try_recv().is_ok());
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
}
