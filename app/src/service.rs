//! A local, per-user owner for pi RPC processes. No model logic lives here.
//! The wire protocol is private to this app version; session files remain pi's own.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::config::{self, AppConfig};
use crate::core::{Core, Shared, Task, lock, start_unstarted};

const VERSION: u32 = 1;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RECORD: u64 = 128 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Hello {
    version: u32,
    request: Request,
}

#[derive(Serialize, Deserialize)]
enum Request {
    List,
    Open {
        cwd: PathBuf,
        session: Option<PathBuf>,
        bin: String,
    },
    Attach {
        id: u64,
    },
    Send(Value),
    Kill {
        id: u64,
    },
    Shutdown,
}

#[derive(Serialize, Deserialize)]
pub struct Info {
    pub id: u64,
    pub cwd: PathBuf,
    pub session: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
enum Reply {
    Sessions(Vec<Info>),
    Attached(Box<Task>),
    Error(String),
    Stopped,
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Event {
    Sent(Value),
    Record(Value),
    Exit(String),
}

pub(crate) fn encode(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn write(stream: &mut UnixStream, value: &impl Serialize) -> io::Result<()> {
    stream.write_all(&encode(value)?)
}

fn read<T: DeserializeOwned>(reader: &mut BufReader<UnixStream>) -> io::Result<T> {
    // LF only, including when JSON strings contain Unicode paragraph separators.
    let mut bytes = Vec::new();
    let size = reader.take(MAX_RECORD + 1).read_until(b'\n', &mut bytes)?;
    if size == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    if size as u64 > MAX_RECORD || bytes.last() != Some(&b'\n') {
        return Err(io::Error::other(
            "service record is too large or incomplete",
        ));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// A short socket path works even for HOME paths longer than sockaddr_un permits.
fn directory() -> PathBuf {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    config::week_dir().hash(&mut hash);
    PathBuf::from("/tmp").join(format!(
        "whodunit-{}-{:016x}",
        unsafe { libc::geteuid() },
        hash.finish()
    ))
}

fn socket() -> PathBuf {
    directory().join("rpc.sock")
}

pub(crate) fn live_path(id: u64) -> PathBuf {
    config::week_dir().join("live").join(id.to_string())
}

fn secure_directory() -> io::Result<()> {
    let path = directory();
    match fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err(io::Error::other(
            "background service directory is not private to this user",
        ));
    }
    Ok(())
}

fn handshake(request: Request) -> io::Result<(UnixStream, BufReader<UnixStream>, Reply)> {
    secure_directory()?;
    let mut stream = UnixStream::connect(socket())?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    write(
        &mut stream,
        &Hello {
            version: VERSION,
            request,
        },
    )?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let reply = read(&mut reader)?;
    if let Reply::Error(error) = reply {
        return Err(io::Error::other(error));
    }
    Ok((stream, reader, reply))
}

pub fn list() -> io::Result<Vec<Info>> {
    match handshake(Request::List)?.2 {
        Reply::Sessions(sessions) => Ok(sessions),
        _ => Err(io::Error::other("unexpected service reply")),
    }
}

/// Spawn this same executable before it initializes rune. A separate session and null
/// stdio keep the service independent of both the GUI and a launching terminal.
pub fn ensure_started() -> io::Result<()> {
    ensure_started_with(&std::env::current_exe()?)
}

pub fn ensure_started_with(executable: &Path) -> io::Result<()> {
    match list() {
        Ok(_) => return Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) => {}
        Err(error) => return Err(error),
    }
    let mut command = Command::new(executable);
    command
        .arg("--background-service")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        match list() {
            Ok(_) => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(error)
                if !matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Err(error);
            }
            Err(_) => thread::sleep(Duration::from_millis(25)),
        }
    }
}

pub fn stop() -> io::Result<()> {
    if !matches!(handshake(Request::Shutdown)?.2, Reply::Stopped) {
        return Err(io::Error::other("unexpected service reply"));
    }
    let owner = OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory().join("owner.lock"))?;
    let deadline = Instant::now() + IO_TIMEOUT;
    while owner.try_lock().is_err() {
        if Instant::now() >= deadline {
            return Err(io::Error::other("service shutdown timed out"));
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

pub struct Connection {
    id: u64,
    writer: Mutex<UnixStream>,
}

impl Connection {
    pub fn send(&self, record: &Value) -> io::Result<()> {
        write(
            &mut self.writer.lock().unwrap(),
            &Request::Send(record.clone()),
        )
    }

    pub fn kill(&self) -> io::Result<()> {
        match handshake(Request::Kill { id: self.id })?.2 {
            Reply::Stopped => Ok(()),
            _ => Err(io::Error::other("unexpected stop reply")),
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.writer.get_mut().unwrap().shutdown(Shutdown::Both);
    }
}

pub(crate) fn attach(
    id: Option<u64>,
    cwd: PathBuf,
    session: Option<PathBuf>,
    bin: String,
) -> io::Result<(Connection, Task, BufReader<UnixStream>)> {
    let request = id.map_or(Request::Open { cwd, session, bin }, |id| Request::Attach {
        id,
    });
    let (stream, reader, reply) = handshake(request)
        .map_err(|error| io::Error::other(format!("attach handshake: {error}")))?;
    let Reply::Attached(task) = reply else {
        return Err(io::Error::other("unexpected attach reply"));
    };
    stream
        .set_read_timeout(None)
        .map_err(|error| io::Error::other(format!("attach timeout: {error}")))?;
    let id = task
        .service_id
        .ok_or_else(|| io::Error::other("missing service task identity"))?;
    Ok((
        Connection {
            id,
            writer: Mutex::new(stream),
        },
        *task,
        reader,
    ))
}

pub(crate) fn read_events(
    mut reader: BufReader<UnixStream>,
    on_record: impl Fn(Event) + Send + 'static,
    on_exit: impl FnOnce() + Send + 'static,
) {
    thread::spawn(move || {
        while let Ok(event) = read(&mut reader) {
            on_record(event);
        }
        on_exit();
    });
}

pub(crate) struct Subscriber {
    pub id: u64,
    pub sender: mpsc::SyncSender<Arc<Vec<u8>>>,
}

struct Service {
    sessions: Mutex<BTreeMap<u64, Shared>>,
    workspaces: Mutex<BTreeMap<PathBuf, config::TemporaryWorkspace>>,
    next_id: AtomicU64,
    next_client: AtomicU64,
    stopping: AtomicBool,
}

impl Service {
    fn open(&self, cwd: PathBuf, session: Option<PathBuf>, bin: String) -> io::Result<Shared> {
        if config::is_temporary(&cwd) {
            let mut workspaces = self.workspaces.lock().unwrap();
            if !workspaces.contains_key(&cwd) {
                workspaces.insert(cwd.clone(), config::TemporaryWorkspace::reopen(&cwd)?);
            }
        }
        let mut sessions = self.sessions.lock().unwrap();
        if self.stopping.load(Ordering::Acquire) {
            return Err(io::Error::other("background service is stopping"));
        }
        if let Some(path) = &session
            && let Some(core) = sessions.values().find(|core| {
                let core = lock(core);
                core.exited.is_none() && core.session_file.as_deref() == path.to_str()
            })
        {
            return Ok(core.clone());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let core = Core::hosted(
            AppConfig {
                pi_bin: bin,
                ..AppConfig::load()
            },
            cwd,
            session,
            id,
        );
        sessions.insert(id, core.clone());
        drop(sessions);
        start_unstarted(&core);
        Ok(core)
    }

    fn reap(&self) {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, core| !lock(core).disposable());
        // A stopped deal and its retry can share a folder. The service owns each folder
        // once, rather than letting the old task delete it underneath the new process.
        let used: Vec<_> = sessions
            .values()
            .map(|core| lock(core).cwd.clone())
            .collect();
        self.workspaces
            .lock()
            .unwrap()
            .retain(|path, _| used.contains(path));
    }

    fn client(self: &Arc<Self>, mut stream: UnixStream) -> io::Result<()> {
        // macOS inherits O_NONBLOCK from the listener on accepted sockets.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let hello: Hello = read(&mut reader)?;
        if hello.version != VERSION && !matches!(hello.request, Request::Shutdown) {
            return write(&mut stream, &Reply::Error("Background service version differs. Finish running tasks, then use --service-stop before reopening the app.".into()));
        }
        let core = match hello.request {
            Request::List => {
                let sessions = self
                    .sessions
                    .lock()
                    .unwrap()
                    .iter()
                    .filter_map(|(id, core)| {
                        let core = lock(core);
                        (core.exited.is_none() && core.recoverable()).then(|| Info {
                            id: *id,
                            cwd: core.cwd.clone(),
                            session: core.session_file.as_ref().map(PathBuf::from),
                        })
                    })
                    .collect();
                return write(&mut stream, &Reply::Sessions(sessions));
            }
            Request::Kill { id } => {
                let mut sessions = self.sessions.lock().unwrap();
                let result = sessions
                    .get(&id)
                    .map_or(Ok(()), |core| lock(core).service_kill());
                if let Err(error) = result {
                    drop(sessions);
                    return write(&mut stream, &Reply::Error(error.to_string()));
                }
                sessions.remove(&id);
                drop(sessions);
                return write(&mut stream, &Reply::Stopped);
            }
            Request::Shutdown => {
                self.stopping.store(true, Ordering::Release);
                return write(&mut stream, &Reply::Stopped);
            }
            Request::Open { cwd, session, bin } => match self.open(cwd, session, bin) {
                Ok(core) => core,
                Err(error) => return write(&mut stream, &Reply::Error(error.to_string())),
            },
            Request::Attach { id } => {
                let Some(core) = self.sessions.lock().unwrap().get(&id).cloned() else {
                    return write(&mut stream, &Reply::Error("This chat is no longer in the service. Open its saved history instead.".into()));
                };
                core
            }
            _ => {
                return write(
                    &mut stream,
                    &Reply::Error("Expected a service handshake.".into()),
                );
            }
        };
        // Snapshot and subscription share the reducer lock. Never write a socket under it.
        let client_id = self.next_client.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::sync_channel::<Arc<Vec<u8>>>(256);
        {
            let mut core = lock(&core);
            // Serialize by reference so images and transcript buffers aren't cloned.
            #[derive(Serialize)]
            enum Attached<'a> {
                Attached(&'a Task),
            }
            sender
                .try_send(Arc::new(encode(&Attached::Attached(&core))?))
                .unwrap();
            core.subscribers.push(Subscriber {
                id: client_id,
                sender,
            });
        }
        stream.set_read_timeout(None)?;
        let mut output = stream.try_clone()?;
        thread::spawn(move || {
            for bytes in receiver {
                if output.write_all(&bytes).is_err() {
                    break;
                }
            }
            let _ = output.shutdown(Shutdown::Both);
        });
        while let Ok(request) = read::<Request>(&mut reader) {
            if self.stopping.load(Ordering::Acquire) {
                break;
            }
            match request {
                Request::Send(record) => lock(&core).service_command(record),
                _ => break,
            }
        }
        let empty = {
            let mut core = lock(&core);
            core.subscribers
                .retain(|subscriber| subscriber.id != client_id);
            (core.subscribers.is_empty() && !core.recoverable()).then_some(core.service_id.unwrap())
        };
        if let Some(id) = empty {
            self.sessions.lock().unwrap().remove(&id);
        }
        let _ = stream.shutdown(Shutdown::Both);
        Ok(())
    }
}

/// The advisory lock prevents competing launchers from removing a live socket. A stale
/// socket is removed only after acquiring that lock; no PID-file guesses are involved.
pub fn run() -> io::Result<()> {
    secure_directory()?;
    let owner: File = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory().join("owner.lock"))?;
    match owner.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Ok(()),
        Err(fs::TryLockError::Error(error)) => return Err(error),
    }
    let _ = fs::remove_file(socket());
    let listener = UnixListener::bind(socket())?;
    fs::set_permissions(socket(), fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let service = Arc::new(Service {
        sessions: Mutex::new(BTreeMap::new()),
        workspaces: Mutex::new(BTreeMap::new()),
        // IDs from a replaced service must never attach an old UI to an unrelated chat.
        next_id: AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
        ),
        next_client: AtomicU64::new(1),
        stopping: AtomicBool::new(false),
    });
    let mut last_reap = Instant::now();
    while !service.stopping.load(Ordering::Acquire) {
        if last_reap.elapsed() >= Duration::from_secs(60) {
            service.reap();
            last_reap = Instant::now();
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let service = service.clone();
                thread::spawn(move || {
                    if let Err(error) = service.client(stream) {
                        eprintln!("service connection: {error}");
                    }
                });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20))
            }
            Err(error) => return Err(error),
        }
    }
    drop(listener);
    let _ = fs::remove_file(socket());
    let sessions: Vec<_> = service.sessions.lock().unwrap().values().cloned().collect();
    for core in &sessions {
        lock(core).stop_hosted();
    }
    // ChildPi's reaper allows three seconds for pi's extension shutdown hooks.
    let deadline = Instant::now() + Duration::from_secs(4);
    while sessions.iter().any(|core| lock(core).exited.is_none()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    service.workspaces.lock().unwrap().clear();
    Ok(())
}

/// Monotonic instants are process-local; snapshots carry elapsed durations instead.
pub(crate) mod elapsed {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &Option<Instant>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.map(|at| at.elapsed()).serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Instant>, D::Error> {
        Ok(Option::<Duration>::deserialize(deserializer)?
            .and_then(|elapsed| Instant::now().checked_sub(elapsed)))
    }
}
