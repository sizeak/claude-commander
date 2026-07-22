//! Headless, short-lived commander conversations.
//!
//! Where the interactive [`commander`](crate::commander) runs one long-lived
//! `claude` inside tmux, the **headless** commander answers one-shot requests
//! (from the Slack bridge, or the `POST /api/commander/ask` route) by driving a
//! `claude -p --input-format stream-json …` subprocess in the same primed
//! commander directory. It is *not* gated on `commander_enabled` — callers gate
//! it (the Slack bridge by `[slack]` config; the HTTP route serves whenever a
//! program is resolvable).
//!
//! # What [`HeadlessCommander`] owns
//!
//! - **Per-conversation processes.** A conversation is an opaque string key
//!   (the Slack bridge uses `slack:<channel>:<thread_ts>`). Asks on the *same*
//!   key are serialized (one process, in order); different keys run
//!   concurrently.
//! - **Linger + reuse.** After a turn completes the process is kept alive for
//!   `slack.linger_secs`, so an immediate follow-up on the same key reuses it
//!   with no cold start. After the linger window it is reaped and its Claude
//!   session id recorded so the *next* ask can `--resume` the conversation.
//! - **A warm pool of one.** When `slack.warm_pool` is set, one fresh (non-
//!   resume) process is kept ready so a brand-new key skips cold-start latency;
//!   it is respawned every `slack.warm_respawn_secs` to avoid staleness.
//! - **Timeout + one retry.** A turn exceeding `slack.invocation_timeout_secs`
//!   is killed and retried once (resuming the conversation) with a "be brief"
//!   nudge; a second timeout surfaces a single [`StreamEvent::Error`].
//! - **`key → claude session id` persistence** in `slack.json` (see
//!   [`SlackSessionStore`]).
//!
//! # Testability
//!
//! Process spawning sits behind the [`CommanderSpawn`] / [`CommanderChild`]
//! trait seam, so unit tests substitute a scripted fake and exercise the
//! linger/warm/timeout state machine with `tokio::time` paused — no real
//! `claude` process and no real filesystem beyond a `tempfile` dir.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout, Command};
use tokio::sync::{Mutex, Notify, OnceCell, mpsc};
use tracing::warn;

use crate::config::ConfigStore;
use crate::error::{Result, SessionError};
use crate::stream_json::{StreamEvent, parse_event, user_message_line};

/// Tools the headless commander is permitted to use. Deliberately locked down:
/// the `claude-commander` CLI (to inspect/create sessions) plus read-only
/// filesystem tools. No arbitrary `Bash`, no `Write`/`Edit` — a Slack-driven
/// agent must never mutate the host beyond the CLI's own guarded surface.
///
/// The `Read`/`Grep`/`Glob` entries are **not** redundant: within the commander
/// working directory those tools need no permission, but the worktrees the agent
/// must inspect live *outside* it, and only a standing allow rule lets a headless
/// (unpromptable) process read them. That allowance is broad, so read
/// *exfiltration* is fenced separately by the [`sensitive_read_denies`] deny
/// list — deny rules beat allow rules — not by omitting these entries (omitting
/// them would only break legitimate worktree reads, not stop secret reads).
const ALLOWED_TOOLS: &[&str] = &["Bash(claude-commander:*)", "Read", "Grep", "Glob"];

/// Permission mode for the headless process. `default` (NOT `bypassPermissions`)
/// combined with the [`ALLOWED_TOOLS`] allow-list is the safe posture: anything
/// outside the allow-list would prompt, and a headless process has no one to
/// answer the prompt, so it simply cannot act outside the list.
const PERMISSION_MODE: &str = "default";

/// Prepended to the prompt on the single automatic retry after a timeout.
const TIMEOUT_NUDGE: &str = "Your previous attempt timed out — be brief and direct.";

/// Current schema of the persisted `slack.json` map.
const SLACK_SCHEMA_VERSION: u32 = 1;

/// Per-process counter making each `CLAUDE.md` temp file name unique, so two
/// conversations priming the shared commander dir concurrently can't collide on
/// the same temp path (see [`HeadlessCommander::write_claude_md`]).
static PRIME_SEQ: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Process seam
// ---------------------------------------------------------------------------

/// How to spawn one headless `claude` process.
pub struct SpawnSpec {
    /// Program string (possibly with flags, e.g. `claude --model opus`).
    pub program: String,
    /// Working directory — the primed commander directory.
    pub cwd: PathBuf,
    /// Claude session id to `--resume`, when continuing a conversation.
    pub resume: Option<String>,
    /// Tools the process is allowed to use (`--allowedTools`).
    pub allowed_tools: Vec<String>,
    /// `permissions.deny` Read rules passed via `--settings`, fencing the agent
    /// off from credential/secret locations even though `Read` is broadly
    /// allowed (see [`sensitive_read_denies`]).
    pub deny_read: Vec<String>,
    /// `--permission-mode` value.
    pub permission_mode: String,
}

/// Build the `permissions.deny` Read rules that fence the headless Slack agent
/// off from credential and secret locations. These are *deny* rules, which take
/// precedence over the broad `Read`/`Grep`/`Glob` allowance ([`ALLOWED_TOOLS`]),
/// so an agent acting on prompt-injected thread text still cannot read them.
///
/// A `Read(...)` deny rule covers every built-in file-reading tool (`Read`,
/// `Grep`, `Glob`) plus recognised file commands in `Bash` (`cat`, `head`, …),
/// per the Claude Code permission model. Paths use its gitignore-style syntax:
/// `~` is home-relative and a leading `//` anchors an absolute filesystem path.
pub(crate) fn sensitive_read_denies(config_dir: Option<&Path>, data_dir: &Path) -> Vec<String> {
    let mut denies = vec![
        "Read(~/.ssh/**)".to_string(),
        "Read(~/.aws/**)".to_string(),
        "Read(~/.gnupg/**)".to_string(),
        // `.env` / `.env.*` files anywhere on the filesystem.
        "Read(//**/.env)".to_string(),
        "Read(//**/.env.*)".to_string(),
    ];
    // The commander config dir holds `config.toml` (Slack + other tokens) and is
    // never something the agent legitimately reads, so deny it wholesale.
    if let Some(dir) = config_dir {
        denies.push(deny_abs_glob(dir, "**"));
    }
    // The data-dir root holds `server-info.json` (server bearer token),
    // `state.json` and `slack.json`. Deny only the JSON files at the root — NOT
    // the whole dir, since the worktrees the agent must read live under it (and
    // `*` never crosses a path separator, so `worktrees/…` is untouched).
    denies.push(deny_abs_glob(data_dir, "*.json"));
    denies
}

/// Format an absolute-path Read deny rule. A leading `//` anchors to the
/// filesystem root; `dir` already begins with `/`, so one extra slash produces
/// the `//<abs>` form Claude Code expects.
fn deny_abs_glob(dir: &Path, tail: &str) -> String {
    format!("Read(/{}/{tail})", dir.display())
}

/// Spawns [`CommanderChild`] processes. The seam that lets tests substitute a
/// scripted fake for the real subprocess.
#[async_trait]
pub trait CommanderSpawn: Send + Sync {
    async fn spawn(&self, spec: SpawnSpec) -> Result<Box<dyn CommanderChild>>;
}

/// A running headless process: send a user turn, receive parsed events, kill it.
#[async_trait]
pub trait CommanderChild: Send {
    /// Send a user turn on stdin.
    async fn send(&mut self, text: &str) -> Result<()>;
    /// Receive the next parsed event, or `None` once the stream closes.
    async fn recv(&mut self) -> Option<StreamEvent>;
    /// Best-effort terminate the process now.
    async fn kill(&mut self);
}

/// The real spawner: launches `claude` with the stream-json protocol and the
/// locked-down permission flags.
pub struct RealSpawn;

#[async_trait]
impl CommanderSpawn for RealSpawn {
    async fn spawn(&self, spec: SpawnSpec) -> Result<Box<dyn CommanderChild>> {
        RealChild::spawn(spec).map(|c| Box::new(c) as Box<dyn CommanderChild>)
    }
}

/// A real `claude` subprocess driven over stream-json. Dropping it kills the
/// child (`kill_on_drop`).
pub struct RealChild {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    events: mpsc::UnboundedReceiver<StreamEvent>,
}

impl RealChild {
    fn spawn(spec: SpawnSpec) -> Result<Self> {
        let parts = shell_words::split(&spec.program).map_err(|e| {
            SessionError::InvalidProgram(format!("could not parse commander program: {e}"))
        })?;
        let (program, args) = parts
            .split_first()
            .ok_or_else(|| SessionError::InvalidProgram("commander program is empty".into()))?;

        let mut cmd = Command::new(program);
        cmd.current_dir(&spec.cwd)
            .args(args)
            .arg("-p")
            .args(["--input-format", "stream-json"])
            .args(["--output-format", "stream-json"])
            .args(["--permission-mode", &spec.permission_mode])
            .arg("--include-partial-messages")
            .arg("--verbose");
        if let Some(id) = spec.resume.as_deref().filter(|id| !id.is_empty()) {
            cmd.args(["--resume", id]);
        }
        // Read-deny rules ride in on `--settings` (accepts an inline JSON string).
        // Must precede the variadic `--allowedTools` below, which greedily
        // consumes the remaining argv.
        if !spec.deny_read.is_empty() {
            let settings = serde_json::json!({
                "permissions": { "deny": spec.deny_read }
            });
            cmd.args(["--settings", &settings.to_string()]);
        }
        // `--allowedTools` is variadic (`<tools...>`), so it must come last: it
        // greedily consumes the remaining argv. With stream-json input there is
        // no trailing positional prompt to be swallowed.
        if !spec.allowed_tools.is_empty() {
            cmd.arg("--allowedTools").args(&spec.allowed_tools);
        }

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                SessionError::InvalidProgram(format!("failed to start `{program}`: {e}"))
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SessionError::InvalidProgram("child stdout unavailable".into()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SessionError::InvalidProgram("child stdin unavailable".into()))?;

        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(log_stderr(stderr));
        }

        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(read_events(stdout, tx));

        Ok(Self {
            child,
            stdin,
            events: rx,
        })
    }
}

#[async_trait]
impl CommanderChild for RealChild {
    async fn send(&mut self, text: &str) -> Result<()> {
        // A broken pipe here is an IO failure, not a bad program string, so it
        // surfaces through the `Error::Io` variant (via `?`) rather than
        // `InvalidProgram`.
        self.stdin
            .write_all(user_message_line(text).as_bytes())
            .await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> Option<StreamEvent> {
        self.events.recv().await
    }

    async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }
}

/// Forward parsed stdout events on `tx`; emit [`StreamEvent::Exited`] at EOF.
async fn read_events(stdout: ChildStdout, tx: mpsc::UnboundedSender<StreamEvent>) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if let Some(ev) = parse_event(&line)
                    && tx.send(ev).is_err()
                {
                    break; // receiver gone
                }
            }
            Ok(None) => break, // EOF
            Err(e) => {
                warn!(target: "commander", "headless read error: {e}");
                break;
            }
        }
    }
    let _ = tx.send(StreamEvent::Exited);
}

/// Log the child's stderr so a failed launch (bad flag, stale `--resume`, auth)
/// is diagnosable rather than surfacing as a bare `Exited`.
async fn log_stderr(stderr: ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if !line.trim().is_empty() {
            warn!(target: "commander", "claude stderr: {line}");
        }
    }
}

// ---------------------------------------------------------------------------
// Persistent key → claude session id map (slack.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlackSessionData {
    /// Persisted schema version, so a future shape change can migrate on read.
    schema_version: u32,
    /// Conversation key → Claude Code session id (for `--resume`).
    sessions: BTreeMap<String, String>,
}

impl Default for SlackSessionData {
    fn default() -> Self {
        Self {
            schema_version: SLACK_SCHEMA_VERSION,
            sessions: BTreeMap::new(),
        }
    }
}

/// Persists the `conversation key → claude session id` map to `slack.json` in
/// the server data dir. Single-owner file (only the server writes it), so — like
/// `tui.json` — a schema counter is sound (unlike multi-writer `state.json`).
pub struct SlackSessionStore {
    path: PathBuf,
    data: StdMutex<SlackSessionData>,
}

impl SlackSessionStore {
    /// Load from `path`, tolerating a missing or unreadable file (starts empty).
    pub fn load(path: PathBuf) -> Self {
        let data = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<SlackSessionData>(&bytes).ok())
            .map(migrate)
            .unwrap_or_default();
        Self {
            path,
            data: StdMutex::new(data),
        }
    }

    /// Recorded Claude session id for `key`, if any.
    pub fn get(&self, key: &str) -> Option<String> {
        self.data.lock().unwrap().sessions.get(key).cloned()
    }

    /// Record `key → session_id` and persist. A write failure is logged, not
    /// propagated: losing the resume hint degrades to a cold start, it does not
    /// fail the ask.
    pub fn set(&self, key: &str, session_id: &str) {
        let snapshot = {
            let mut data = self.data.lock().unwrap();
            data.sessions
                .insert(key.to_string(), session_id.to_string());
            data.clone()
        };
        if let Err(e) = persist(&self.path, &snapshot) {
            warn!(target: "commander", "failed to persist slack.json: {e}");
        }
    }

    /// Drop `key`'s recorded session id and persist. Used to recover from a
    /// stale `--resume` id whose Claude session no longer exists: the resumed
    /// spawn dies immediately, so the mapping is cleared and the next ask cold-
    /// starts instead of resuming a dead id forever.
    pub fn remove(&self, key: &str) {
        let snapshot = {
            let mut data = self.data.lock().unwrap();
            if data.sessions.remove(key).is_none() {
                return; // nothing changed → nothing to persist
            }
            data.clone()
        };
        if let Err(e) = persist(&self.path, &snapshot) {
            warn!(target: "commander", "failed to persist slack.json after remove: {e}");
        }
    }
}

/// Migrate an older on-disk shape forward. No prior versions exist yet; the hook
/// is here so a future bump has a home (and stamps the current version).
fn migrate(mut data: SlackSessionData) -> SlackSessionData {
    if data.schema_version < SLACK_SCHEMA_VERSION {
        data.schema_version = SLACK_SCHEMA_VERSION;
    }
    data
}

/// Persist atomically (temp-file + rename) so a concurrent reader never sees a
/// torn file, and a crash mid-write can't truncate the existing map. The temp
/// name carries pid + a per-process sequence (shared with the `CLAUDE.md`
/// writer's [`PRIME_SEQ`]) so two writers can't collide on it.
fn persist(path: &Path, data: &SlackSessionData) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(data)?;
    let seq = PRIME_SEQ.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("slack.json");
    let tmp = path.with_file_name(format!(".{file_name}.tmp.{}.{seq}", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, &bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path)
}

// ---------------------------------------------------------------------------
// The manager
// ---------------------------------------------------------------------------

/// Per-conversation live process + its serialization lock.
#[derive(Default)]
struct KeyEntry {
    /// Holds the lingering live process between asks; also serializes asks on
    /// this key (a concurrent ask on the same key awaits this lock).
    proc: Mutex<KeyProc>,
    /// Bumped on every ask so a previously-scheduled linger reaper knows a newer
    /// ask has since claimed the key and must not kill its process.
    generation: AtomicU64,
}

#[derive(Default)]
struct KeyProc {
    live: Option<Box<dyn CommanderChild>>,
    session_id: Option<String>,
}

#[derive(Default)]
struct WarmState {
    child: Option<Box<dyn CommanderChild>>,
}

struct Inner {
    config_store: Arc<ConfigStore>,
    spawn: Arc<dyn CommanderSpawn>,
    sessions: Arc<SlackSessionStore>,
    /// Pre-rendered Slack-primed `CLAUDE.md` (written on first use).
    claude_md: String,
    commander_dir: PathBuf,
    /// `permissions.deny` Read rules applied to every spawn (see
    /// [`sensitive_read_denies`]). Computed once at construction from the
    /// process's config/data dirs, which do not change for its lifetime.
    deny_read: Vec<String>,
    /// Per-conversation entries. Grows by one small `Arc` per distinct thread
    /// key ever seen and is never evicted: the growth is bounded in practice
    /// (one entry per Slack thread the bot is used in), and eviction would have
    /// to race the per-key serialization lock for a negligible memory win, so it
    /// is deliberately omitted.
    keys: StdMutex<HashMap<String, Arc<KeyEntry>>>,
    warm: Mutex<WarmState>,
    /// Signalled when the warm process is consumed, so maintenance refills it.
    warm_refill: Notify,
    /// Guards the one-time directory prime.
    primed: OnceCell<()>,
}

/// Manager for headless commander conversations. Cheap to clone (`Arc` inside).
#[derive(Clone)]
pub struct HeadlessCommander {
    inner: Arc<Inner>,
}

/// Outcome of a single turn (internal). A failure carries its terminal message
/// so the caller ([`HeadlessCommander::run_ask`]) decides whether to surface it
/// as a [`StreamEvent::Error`] or first attempt a stale-resume recovery.
enum TurnOutcome {
    Complete,
    Failed(String),
}

/// Result of streaming one turn to completion (internal).
enum StreamResult {
    Complete,
    Failed(String),
    TimedOut,
}

/// A live ask: a receiver of [`StreamEvent`]s. The underlying turn runs on a
/// spawned task; the stream closes when the turn ends (complete or errored).
pub struct AskStream {
    rx: mpsc::UnboundedReceiver<StreamEvent>,
}

impl AskStream {
    /// Await the next event, or `None` once the turn has ended.
    pub async fn recv(&mut self) -> Option<StreamEvent> {
        self.rx.recv().await
    }

    /// Consume into the raw receiver (used by the HTTP route to build a body).
    pub fn into_receiver(self) -> mpsc::UnboundedReceiver<StreamEvent> {
        self.rx
    }
}

/// Map a core [`StreamEvent`] onto the public wire event. The internal
/// `Exited` lifecycle event never reaches a consumer (the manager converts it
/// to an `Error`), but is mapped defensively so the conversion is total.
pub fn stream_event_to_wire(ev: StreamEvent) -> claude_commander_protocol::api::CommanderEvent {
    use claude_commander_protocol::api::CommanderEvent as Wire;
    match ev {
        StreamEvent::Started { session_id } => Wire::Started { session_id },
        StreamEvent::Delta(text) => Wire::Delta { text },
        StreamEvent::Break => Wire::Break,
        StreamEvent::TurnComplete => Wire::TurnComplete,
        StreamEvent::Error(message) => Wire::Error { message },
        StreamEvent::Exited => Wire::Error {
            message: "commander process exited".to_string(),
        },
    }
}

impl HeadlessCommander {
    /// Construct the manager. `claude_md` is the shared `CLAUDE.md` body (built
    /// once by the caller from the live CLI — identical to what the interactive
    /// commander writes) written to `commander_dir` on first use. `deny_read` is
    /// the read-deny rule set applied to every spawn (see
    /// [`sensitive_read_denies`]). No IO happens here.
    pub fn new(
        config_store: Arc<ConfigStore>,
        claude_md: String,
        commander_dir: PathBuf,
        spawn: Arc<dyn CommanderSpawn>,
        sessions: Arc<SlackSessionStore>,
        deny_read: Vec<String>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                config_store,
                spawn,
                sessions,
                claude_md,
                commander_dir,
                deny_read,
                keys: StdMutex::new(HashMap::new()),
                warm: Mutex::new(WarmState::default()),
                warm_refill: Notify::new(),
                primed: OnceCell::new(),
            }),
        }
    }

    /// Ask the commander `prompt` in conversation `key`, returning a stream of
    /// events. The turn runs on a background task; drop the [`AskStream`] to
    /// stop consuming (the turn still finishes so the process can linger).
    pub fn ask(&self, key: &str, prompt: &str) -> AskStream {
        let (tx, rx) = mpsc::unbounded_channel();
        let this = self.clone();
        let key = key.to_string();
        let prompt = prompt.to_string();
        tokio::spawn(async move {
            this.run_ask(key, prompt, tx).await;
        });
        AskStream { rx }
    }

    /// Start the warm-pool maintenance loop: keep one fresh process ready (when
    /// `slack.warm_pool`), refill it when consumed, and respawn it every
    /// `slack.warm_respawn_secs` to avoid staleness. Long-lived callers (the
    /// server) start this once; it runs for the process lifetime.
    pub fn spawn_maintenance(&self) {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                this.ensure_warm().await;
                let respawn = Duration::from_secs(
                    this.inner
                        .config_store
                        .read()
                        .slack
                        .warm_respawn_secs
                        .max(1),
                );
                tokio::select! {
                    _ = tokio::time::sleep(respawn) => this.respawn_warm().await,
                    _ = this.inner.warm_refill.notified() => this.ensure_warm().await,
                }
            }
        });
    }

    // -- turn orchestration --

    async fn run_ask(self, key: String, prompt: String, tx: mpsc::UnboundedSender<StreamEvent>) {
        if let Err(e) = self.ensure_primed().await {
            let _ = tx.send(StreamEvent::Error(format!(
                "failed to prepare commander directory: {e}"
            )));
            return;
        }

        let entry = self.key_entry(&key);
        // Holding this lock for the whole turn serializes asks on the same key
        // while letting different keys proceed concurrently.
        let mut proc = entry.proc.lock().await;
        let generation = entry.generation.fetch_add(1, Ordering::SeqCst) + 1;

        let (mut child, resumed_store_id) = match proc.live.take() {
            Some(c) => (c, None), // reuse the lingering live process
            None => match self.acquire_fresh(&key).await {
                Ok(pair) => pair,
                Err(e) => {
                    let _ = tx.send(StreamEvent::Error(format!(
                        "failed to start commander: {e}"
                    )));
                    return;
                }
            },
        };

        let mut session_id = proc.session_id.clone();
        let mut outcome = self
            .run_turn(&mut child, &prompt, &mut session_id, &tx, &key)
            .await;

        // Stale-resume recovery: a process spawned by resuming a *stored* id that
        // failed before ever emitting `Started` (so `session_id` is still `None`)
        // almost always means that Claude session no longer exists — and left
        // untouched the dead mapping would fail every future ask identically.
        // Drop it and retry ONCE on a fresh (no-resume) process. This is
        // independent of `run_turn`'s timeout retries and bounded to one attempt
        // per ask, so it can't loop.
        if matches!(outcome, TurnOutcome::Failed(_))
            && resumed_store_id.is_some()
            && session_id.is_none()
        {
            self.inner.sessions.remove(&key);
            match self.spawn_child(None).await {
                Ok(fresh) => {
                    child = fresh;
                    outcome = self
                        .run_turn(&mut child, &prompt, &mut session_id, &tx, &key)
                        .await;
                }
                Err(e) => {
                    warn!(target: "commander", "stale-resume recovery spawn failed: {e}");
                }
            }
        }

        // A turn always advances the conversation, so record the session id for
        // a future `--resume` even when it failed/timed out. Done after recovery
        // so the fresh process's *new* id wins over the dead one we cleared.
        if let Some(id) = &session_id {
            self.inner.sessions.set(&key, id);
        }

        match outcome {
            TurnOutcome::Complete => {
                proc.session_id = session_id;
                proc.live = Some(child);
                drop(proc); // release the key so the reaper / next ask can run
                self.schedule_reap(&key, generation);
            }
            TurnOutcome::Failed(msg) => {
                // `run_turn` already killed the child on every failure path;
                // dropping it here also triggers `kill_on_drop` for the real
                // process, so we must not double-kill.
                let _ = tx.send(StreamEvent::Error(msg));
                proc.session_id = session_id;
                proc.live = None;
            }
        }
    }

    /// Acquire a process for a key with no lingering process: resume a recorded
    /// conversation, else take the warm process, else spawn fresh. The returned
    /// `Option<String>` is the store id we resumed from (`None` for a warm/fresh
    /// process), so [`run_ask`](Self::run_ask) can recover if a resume turns out
    /// to be stale.
    async fn acquire_fresh(&self, key: &str) -> Result<(Box<dyn CommanderChild>, Option<String>)> {
        if let Some(id) = self.inner.sessions.get(key) {
            return Ok((self.spawn_child(Some(&id)).await?, Some(id)));
        }
        if let Some(warm) = self.take_warm().await {
            return Ok((warm, None));
        }
        Ok((self.spawn_child(None).await?, None))
    }

    /// Drive one turn, enforcing the invocation timeout with a single automatic
    /// retry (resuming the conversation, with a "be brief" nudge). Forwards
    /// deltas/breaks/started/turn-complete on `tx`; the terminal error is *not*
    /// emitted here — it is returned in [`TurnOutcome::Failed`] so the caller can
    /// attempt stale-resume recovery before surfacing it.
    async fn run_turn(
        &self,
        child: &mut Box<dyn CommanderChild>,
        prompt: &str,
        session_id: &mut Option<String>,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        key: &str,
    ) -> TurnOutcome {
        let timeout = self.invocation_timeout();
        let mut prompt_to_send = prompt.to_string();
        let mut attempts: u32 = 0;

        loop {
            if let Err(e) = child.send(&prompt_to_send).await {
                child.kill().await;
                return TurnOutcome::Failed(format!("failed to send prompt: {e}"));
            }

            match self.stream_turn(child, session_id, tx, timeout).await {
                StreamResult::Complete => return TurnOutcome::Complete,
                StreamResult::Failed(msg) => {
                    child.kill().await;
                    return TurnOutcome::Failed(msg);
                }
                StreamResult::TimedOut => {
                    child.kill().await;
                    attempts += 1;
                    if attempts >= 2 {
                        return TurnOutcome::Failed("timed out".to_string());
                    }
                    // Retry once on a fresh process resuming the same conversation.
                    let resume = session_id.clone().or_else(|| self.inner.sessions.get(key));
                    match self.spawn_child(resume.as_deref()).await {
                        Ok(c) => *child = c,
                        Err(e) => {
                            return TurnOutcome::Failed(format!("retry spawn failed: {e}"));
                        }
                    }
                    prompt_to_send = format!("{TIMEOUT_NUDGE}\n\n{prompt}");
                }
            }
        }
    }

    /// Stream events until the turn completes, errors, or the wall-clock
    /// `timeout` elapses. Forwards deltas/breaks/started/turn-complete on `tx`;
    /// terminal errors are surfaced to the caller (not double-emitted here).
    async fn stream_turn(
        &self,
        child: &mut Box<dyn CommanderChild>,
        session_id: &mut Option<String>,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        timeout: Duration,
    ) -> StreamResult {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match tokio::time::timeout_at(deadline, child.recv()).await {
                Err(_) => return StreamResult::TimedOut,
                Ok(None) => {
                    return StreamResult::Failed("commander stream closed unexpectedly".into());
                }
                Ok(Some(ev)) => match ev {
                    StreamEvent::Started { session_id: id } => {
                        *session_id = Some(id.clone());
                        let _ = tx.send(StreamEvent::Started { session_id: id });
                    }
                    StreamEvent::TurnComplete => {
                        let _ = tx.send(StreamEvent::TurnComplete);
                        return StreamResult::Complete;
                    }
                    StreamEvent::Error(msg) => return StreamResult::Failed(msg),
                    StreamEvent::Exited => {
                        return StreamResult::Failed(
                            "commander process exited before completing the turn".into(),
                        );
                    }
                    other => {
                        let _ = tx.send(other);
                    }
                },
            }
        }
    }

    // -- linger reaping --

    fn schedule_reap(&self, key: &str, generation: u64) {
        let this = self.clone();
        let key = key.to_string();
        let linger = self.linger();
        tokio::spawn(async move {
            tokio::time::sleep(linger).await;
            this.reap(&key, generation).await;
        });
    }

    /// Reap the lingering process for `key` iff no newer ask has claimed it
    /// (its generation is unchanged). Exposed to the crate so tests can drive
    /// the state machine deterministically instead of racing the timer.
    async fn reap(&self, key: &str, generation: u64) {
        let entry = self.key_entry(key);
        if entry.generation.load(Ordering::SeqCst) != generation {
            return; // a newer ask owns the key now
        }
        let mut proc = entry.proc.lock().await;
        // Re-check under the lock: a new ask may have bumped the generation
        // between our pre-check and acquiring the lock.
        if entry.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        if let Some(mut child) = proc.live.take() {
            child.kill().await;
        }
    }

    // -- warm pool --

    async fn take_warm(&self) -> Option<Box<dyn CommanderChild>> {
        let taken = self.inner.warm.lock().await.child.take();
        if taken.is_some() {
            self.inner.warm_refill.notify_one();
        }
        taken
    }

    /// Ensure a warm process is ready (when enabled and none is held).
    async fn ensure_warm(&self) {
        if !self.inner.config_store.read().slack.warm_pool {
            return;
        }
        if self.ensure_primed().await.is_err() {
            return;
        }
        // Spawn outside the warm lock so a slow spawn doesn't block `take_warm`.
        if self.inner.warm.lock().await.child.is_some() {
            return;
        }
        match self.spawn_child(None).await {
            Ok(c) => {
                let mut w = self.inner.warm.lock().await;
                if w.child.is_none() {
                    w.child = Some(c);
                }
            }
            Err(e) => warn!(target: "commander", "failed to spawn warm commander: {e}"),
        }
    }

    /// Kill the current warm process and spawn a fresh replacement.
    async fn respawn_warm(&self) {
        let old = self.inner.warm.lock().await.child.take();
        if let Some(mut c) = old {
            c.kill().await;
        }
        self.ensure_warm().await;
    }

    // -- helpers --

    /// One-time directory prep: create the commander dir, write `CLAUDE.md`,
    /// and seed `NOTES.md`. Once per process is enough: the interactive
    /// commander writes byte-identical content (the Slack section is part of
    /// the single shared prime — see `commander::claude_md_content`), so a
    /// later interactive open can never leave the file meaningfully stale.
    async fn ensure_primed(&self) -> Result<()> {
        self.inner
            .primed
            .get_or_try_init(|| async {
                tokio::fs::create_dir_all(&self.inner.commander_dir).await?;
                self.write_claude_md().await?;
                crate::commander::seed_notes_md(&self.inner.commander_dir).await?;
                Ok::<(), crate::Error>(())
            })
            .await?;
        Ok(())
    }

    /// Atomically (temp-file + rename) write `CLAUDE.md` so a concurrently-
    /// launching `claude` never reads a torn file.
    ///
    /// Not `config::write_private_file`: that names its temp by pid alone, which
    /// collides when two managers prime the shared commander dir concurrently,
    /// so one rename would fail. A per-call sequence number keeps each temp
    /// unique.
    async fn write_claude_md(&self) -> Result<()> {
        let dir = &self.inner.commander_dir;
        let seq = PRIME_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".CLAUDE.md.tmp.{}.{seq}", std::process::id()));
        let target = dir.join("CLAUDE.md");
        let write_and_rename = async {
            tokio::fs::write(&tmp, &self.inner.claude_md).await?;
            tokio::fs::rename(&tmp, &target).await
        };
        if let Err(e) = write_and_rename.await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e.into());
        }
        Ok(())
    }

    async fn spawn_child(&self, resume: Option<&str>) -> Result<Box<dyn CommanderChild>> {
        let program = self.inner.config_store.read().commander_program();
        let spec = SpawnSpec {
            program,
            cwd: self.inner.commander_dir.clone(),
            resume: resume.map(|s| s.to_string()),
            allowed_tools: ALLOWED_TOOLS.iter().map(|s| s.to_string()).collect(),
            deny_read: self.inner.deny_read.clone(),
            permission_mode: PERMISSION_MODE.to_string(),
        };
        self.inner.spawn.spawn(spec).await
    }

    fn key_entry(&self, key: &str) -> Arc<KeyEntry> {
        let mut map = self.inner.keys.lock().unwrap();
        map.entry(key.to_string())
            .or_insert_with(|| Arc::new(KeyEntry::default()))
            .clone()
    }

    fn invocation_timeout(&self) -> Duration {
        Duration::from_secs(
            self.inner
                .config_store
                .read()
                .slack
                .invocation_timeout_secs
                .max(1),
        )
    }

    fn linger(&self) -> Duration {
        Duration::from_secs(self.inner.config_store.read().slack.linger_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;
    use tempfile::TempDir;

    // -- slack.json persistence --

    #[test]
    fn slack_store_roundtrips_and_stamps_schema_version() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("slack.json");

        let store = SlackSessionStore::load(path.clone());
        assert_eq!(store.get("slack:C1:T1"), None);
        store.set("slack:C1:T1", "sess-abc");
        store.set("slack:C2:T2", "sess-def");

        // Reload from disk: the mapping survives.
        let reloaded = SlackSessionStore::load(path.clone());
        assert_eq!(reloaded.get("slack:C1:T1").as_deref(), Some("sess-abc"));
        assert_eq!(reloaded.get("slack:C2:T2").as_deref(), Some("sess-def"));

        // The persisted file carries the current schema version.
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["schema_version"], SLACK_SCHEMA_VERSION);
    }

    #[test]
    fn slack_store_tolerates_missing_and_corrupt_file() {
        let dir = TempDir::new().unwrap();
        // Missing file → empty, current schema.
        let missing = SlackSessionStore::load(dir.path().join("nope.json"));
        assert_eq!(missing.get("x"), None);

        // Corrupt file → empty, does not panic.
        let corrupt = dir.path().join("bad.json");
        std::fs::write(&corrupt, b"not json at all").unwrap();
        let store = SlackSessionStore::load(corrupt);
        assert_eq!(store.get("x"), None);
    }

    #[test]
    fn slack_store_remove_drops_key_and_persists_atomically() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("slack.json");
        let store = SlackSessionStore::load(path.clone());
        store.set("k1", "v1");
        store.set("k2", "v2");

        store.remove("k1");
        assert_eq!(store.get("k1"), None);
        assert_eq!(store.get("k2").as_deref(), Some("v2"));

        // The removal is durable across a reload, and no temp file leaks.
        let reloaded = SlackSessionStore::load(path);
        assert_eq!(reloaded.get("k1"), None);
        assert_eq!(reloaded.get("k2").as_deref(), Some("v2"));
        let leftover: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftover.is_empty(), "atomic write left a temp file behind");

        // Removing an absent key is a no-op (and must not panic).
        store.remove("nope");
    }

    #[test]
    fn slack_store_migrates_old_schema_version_forward() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("slack.json");
        std::fs::write(
            &path,
            serde_json::json!({ "schema_version": 0, "sessions": { "k": "old" } }).to_string(),
        )
        .unwrap();

        let store = SlackSessionStore::load(path.clone());
        assert_eq!(store.get("k").as_deref(), Some("old"));
        // Writing back stamps the current version.
        store.set("k2", "v2");
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["schema_version"], SLACK_SCHEMA_VERSION);
    }

    // -- fake process seam --

    /// A scripted fake child: emits a queue of events across turns; when the
    /// queue empties it either hangs forever (to trigger the timeout) or returns
    /// `None` (stream closed).
    struct FakeChild {
        queue: VecDeque<StreamEvent>,
        hang_when_empty: bool,
        kills: Arc<AtomicUsize>,
        sends: Arc<StdMutex<Vec<String>>>,
    }

    #[async_trait]
    impl CommanderChild for FakeChild {
        async fn send(&mut self, text: &str) -> Result<()> {
            self.sends.lock().unwrap().push(text.to_string());
            Ok(())
        }
        async fn recv(&mut self) -> Option<StreamEvent> {
            match self.queue.pop_front() {
                Some(ev) => Some(ev),
                None => {
                    if self.hang_when_empty {
                        std::future::pending::<()>().await;
                        unreachable!()
                    } else {
                        None
                    }
                }
            }
        }
        async fn kill(&mut self) {
            self.kills.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Blueprint {
        events: Vec<StreamEvent>,
        hang_when_empty: bool,
    }

    /// A fake spawner handing out scripted children in order, recording every
    /// [`SpawnSpec`] and total kills for assertions.
    struct FakeSpawn {
        blueprints: StdMutex<VecDeque<Blueprint>>,
        specs: StdMutex<Vec<SpawnSpec>>,
        spawn_count: Arc<AtomicUsize>,
        kills: Arc<AtomicUsize>,
        sends: Arc<StdMutex<Vec<String>>>,
    }

    impl FakeSpawn {
        fn new(blueprints: Vec<Blueprint>) -> Arc<Self> {
            Arc::new(Self {
                blueprints: StdMutex::new(blueprints.into_iter().collect()),
                specs: StdMutex::new(Vec::new()),
                spawn_count: Arc::new(AtomicUsize::new(0)),
                kills: Arc::new(AtomicUsize::new(0)),
                sends: Arc::new(StdMutex::new(Vec::new())),
            })
        }
    }

    #[async_trait]
    impl CommanderSpawn for FakeSpawn {
        async fn spawn(&self, spec: SpawnSpec) -> Result<Box<dyn CommanderChild>> {
            self.spawn_count.fetch_add(1, Ordering::SeqCst);
            let bp = self
                .blueprints
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Blueprint {
                    events: vec![],
                    hang_when_empty: false,
                });
            self.specs.lock().unwrap().push(spec);
            Ok(Box::new(FakeChild {
                queue: bp.events.into_iter().collect(),
                hang_when_empty: bp.hang_when_empty,
                kills: self.kills.clone(),
                sends: self.sends.clone(),
            }))
        }
    }

    fn turn(session_id: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::Started {
                session_id: session_id.to_string(),
            },
            StreamEvent::Delta("Hello".into()),
            StreamEvent::Delta(" world".into()),
            StreamEvent::TurnComplete,
        ]
    }

    /// Build a manager over a temp dir with the given fake spawner and config.
    fn manager(dir: &TempDir, spawn: Arc<FakeSpawn>, config: Config) -> HeadlessCommander {
        let config_store = Arc::new(ConfigStore::with_path(
            config,
            dir.path().join("config.toml"),
        ));
        let sessions = Arc::new(SlackSessionStore::load(dir.path().join("slack.json")));
        // Realistic deny rules (config dir + data dir both the temp dir) so the
        // spec-assertion test sees what production would generate.
        let deny_read = sensitive_read_denies(Some(dir.path()), dir.path());
        HeadlessCommander::new(
            config_store,
            "# primed".to_string(),
            dir.path().join("commander"),
            spawn,
            sessions,
            deny_read,
        )
    }

    async fn drain(mut stream: AskStream) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        while let Some(ev) = stream.recv().await {
            out.push(ev);
        }
        out
    }

    #[tokio::test]
    async fn ask_streams_deltas_completes_and_persists_session_id() {
        let dir = TempDir::new().unwrap();
        let spawn = FakeSpawn::new(vec![Blueprint {
            events: turn("sess-1"),
            hang_when_empty: false,
        }]);
        let mgr = manager(&dir, spawn.clone(), Config::default());

        let events = drain(mgr.ask("slack:C:T", "hi")).await;
        assert_eq!(
            events,
            vec![
                StreamEvent::Started {
                    session_id: "sess-1".into()
                },
                StreamEvent::Delta("Hello".into()),
                StreamEvent::Delta(" world".into()),
                StreamEvent::TurnComplete,
            ]
        );
        // Session id recorded for future --resume.
        assert_eq!(
            mgr.inner.sessions.get("slack:C:T").as_deref(),
            Some("sess-1")
        );
        // Exactly one process spawned; CLAUDE.md was primed.
        assert_eq!(spawn.spawn_count.load(Ordering::SeqCst), 1);
        assert!(dir.path().join("commander/CLAUDE.md").exists());
    }

    #[tokio::test]
    async fn lingering_process_is_reused_on_next_ask_same_key() {
        let dir = TempDir::new().unwrap();
        // One child, two turns queued back to back.
        let mut events = turn("sess-1");
        events.extend([
            StreamEvent::Delta("again".into()),
            StreamEvent::TurnComplete,
        ]);
        let spawn = FakeSpawn::new(vec![Blueprint {
            events,
            hang_when_empty: false,
        }]);
        let mgr = manager(&dir, spawn.clone(), Config::default());

        drain(mgr.ask("slack:C:T", "first")).await;
        let second = drain(mgr.ask("slack:C:T", "second")).await;
        assert_eq!(second.last(), Some(&StreamEvent::TurnComplete));
        assert!(second.contains(&StreamEvent::Delta("again".into())));
        // Reused the lingering process — no second spawn.
        assert_eq!(spawn.spawn_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn recorded_session_id_is_resumed_on_a_fresh_ask() {
        let dir = TempDir::new().unwrap();
        // Pre-seed a recorded conversation id.
        let sessions = SlackSessionStore::load(dir.path().join("slack.json"));
        sessions.set("slack:C:T", "prior-sess");
        drop(sessions);

        let spawn = FakeSpawn::new(vec![Blueprint {
            events: turn("prior-sess"),
            hang_when_empty: false,
        }]);
        let mgr = manager(&dir, spawn.clone(), Config::default());

        drain(mgr.ask("slack:C:T", "resume please")).await;
        let specs = spawn.specs.lock().unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].resume.as_deref(), Some("prior-sess"));
        // Locked-down permissions are applied on every spawn.
        assert_eq!(specs[0].permission_mode, "default");
        assert!(
            specs[0]
                .allowed_tools
                .iter()
                .any(|t| t.contains("claude-commander"))
        );
        // Read-deny rules ride on every spawn, fencing off secret locations.
        assert!(specs[0].deny_read.iter().any(|d| d == "Read(~/.ssh/**)"));
        assert!(specs[0].deny_read.iter().any(|d| d == "Read(//**/.env)"));
    }

    #[tokio::test]
    async fn stale_resume_id_is_dropped_and_recovered_on_a_fresh_spawn() {
        let dir = TempDir::new().unwrap();
        // Pre-seed a dead recorded id: the first (resumed) spawn's stream closes
        // immediately — as `claude -p --resume <dead-id>` does when that Claude
        // session no longer exists.
        let sessions = SlackSessionStore::load(dir.path().join("slack.json"));
        sessions.set("slack:C:T", "dead-id");
        drop(sessions);

        let spawn = FakeSpawn::new(vec![
            // First child: resumed from the dead id — recv returns None at once.
            Blueprint {
                events: vec![],
                hang_when_empty: false,
            },
            // Second child: fresh (no resume) — plays a normal turn.
            Blueprint {
                events: turn("fresh-sess"),
                hang_when_empty: false,
            },
        ]);
        let mgr = manager(&dir, spawn.clone(), Config::default());

        let events = drain(mgr.ask("slack:C:T", "hi")).await;
        // The ask ultimately completes — no terminal error reaches the consumer.
        assert_eq!(events.last(), Some(&StreamEvent::TurnComplete));
        assert!(!events.iter().any(|e| matches!(e, StreamEvent::Error(_))));

        let specs = spawn.specs.lock().unwrap();
        assert_eq!(specs.len(), 2);
        // First spawn resumed the dead id; the recovery spawn is fresh.
        assert_eq!(specs[0].resume.as_deref(), Some("dead-id"));
        assert_eq!(specs[1].resume, None);
        drop(specs);

        // The dead mapping was replaced by the fresh session id.
        assert_eq!(
            mgr.inner.sessions.get("slack:C:T").as_deref(),
            Some("fresh-sess")
        );
    }

    #[test]
    fn sensitive_read_denies_covers_secrets_and_spares_worktrees() {
        let config_dir = Path::new("/home/u/.config/claude-commander");
        let data_dir = Path::new("/home/u/.local/share/claude-commander");
        let denies = sensitive_read_denies(Some(config_dir), data_dir);

        for expected in [
            "Read(~/.ssh/**)",
            "Read(~/.aws/**)",
            "Read(~/.gnupg/**)",
            "Read(//**/.env)",
            "Read(//**/.env.*)",
            // Config dir denied wholesale (leading `//` anchors an absolute path).
            "Read(//home/u/.config/claude-commander/**)",
            // Data-dir JSON denied at the root only.
            "Read(//home/u/.local/share/claude-commander/*.json)",
        ] {
            assert!(
                denies.iter().any(|d| d == expected),
                "missing deny rule {expected}; got {denies:?}"
            );
        }
        // The data dir is NOT denied wholesale — the worktrees under it must stay
        // readable.
        assert!(
            !denies
                .iter()
                .any(|d| d == "Read(//home/u/.local/share/claude-commander/**)"),
            "data dir must not be denied wholesale"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_retries_once_then_errors() {
        let dir = TempDir::new().unwrap();
        // Two hanging children: first attempt started (so we have a session id),
        // then hangs; retry hangs from the start.
        let spawn = FakeSpawn::new(vec![
            Blueprint {
                events: vec![StreamEvent::Started {
                    session_id: "sess-1".into(),
                }],
                hang_when_empty: true,
            },
            Blueprint {
                events: vec![],
                hang_when_empty: true,
            },
        ]);
        let mut config = Config::default();
        config.slack.invocation_timeout_secs = 60;
        let mgr = manager(&dir, spawn.clone(), config);

        let events = drain(mgr.ask("slack:C:T", "do the thing")).await;
        // The Started from the first attempt is forwarded; the turn ultimately
        // errors after the retry also times out.
        assert!(events.contains(&StreamEvent::Started {
            session_id: "sess-1".into()
        }));
        assert_eq!(events.last(), Some(&StreamEvent::Error("timed out".into())));
        // Exactly two spawns (initial + one retry) and both were killed.
        assert_eq!(spawn.spawn_count.load(Ordering::SeqCst), 2);
        assert_eq!(spawn.kills.load(Ordering::SeqCst), 2);
        // The retry resumed the conversation and carried the nudge.
        let specs = spawn.specs.lock().unwrap();
        assert_eq!(specs[1].resume.as_deref(), Some("sess-1"));
        let sends = spawn.sends.lock().unwrap();
        assert!(sends.iter().any(|s| s.contains(TIMEOUT_NUDGE)));
    }

    #[tokio::test]
    async fn reap_kills_idle_lingering_process_but_respects_newer_generation() {
        let dir = TempDir::new().unwrap();
        let spawn = FakeSpawn::new(vec![Blueprint {
            events: turn("sess-1"),
            hang_when_empty: false,
        }]);
        let mgr = manager(&dir, spawn.clone(), Config::default());

        drain(mgr.ask("slack:C:T", "hi")).await;
        // The completed ask used generation 1 and left a live process.
        // A stale reaper (wrong generation) must NOT kill it.
        mgr.reap("slack:C:T", 999).await;
        assert_eq!(spawn.kills.load(Ordering::SeqCst), 0);
        {
            let entry = mgr.key_entry("slack:C:T");
            assert!(entry.proc.lock().await.live.is_some());
        }

        // The matching reaper (generation 1) reaps it.
        mgr.reap("slack:C:T", 1).await;
        assert_eq!(spawn.kills.load(Ordering::SeqCst), 1);
        let entry = mgr.key_entry("slack:C:T");
        assert!(entry.proc.lock().await.live.is_none());
    }

    #[tokio::test]
    async fn warm_process_is_spawned_and_consumed_by_a_new_key() {
        let dir = TempDir::new().unwrap();
        let spawn = FakeSpawn::new(vec![
            // Warm process (fresh, no resume).
            Blueprint {
                events: vec![],
                hang_when_empty: false,
            },
            // The turn served by the consumed warm process.
            Blueprint {
                events: turn("sess-1"),
                hang_when_empty: false,
            },
        ]);
        let mgr = manager(&dir, spawn.clone(), Config::default());

        // Warm up: one fresh spawn, held in the pool.
        mgr.ensure_warm().await;
        assert_eq!(spawn.spawn_count.load(Ordering::SeqCst), 1);
        assert!(spawn.specs.lock().unwrap()[0].resume.is_none());

        // A brand-new key takes the warm process instead of cold-spawning.
        let taken = mgr.take_warm().await;
        assert!(taken.is_some());
        assert!(mgr.inner.warm.lock().await.child.is_none());
    }

    #[tokio::test]
    async fn warm_pool_disabled_spawns_nothing() {
        let dir = TempDir::new().unwrap();
        let spawn = FakeSpawn::new(vec![]);
        let mut config = Config::default();
        config.slack.warm_pool = false;
        let mgr = manager(&dir, spawn.clone(), config);

        mgr.ensure_warm().await;
        assert_eq!(spawn.spawn_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn respawn_warm_replaces_the_pooled_process() {
        let dir = TempDir::new().unwrap();
        let spawn = FakeSpawn::new(vec![
            Blueprint {
                events: vec![],
                hang_when_empty: false,
            },
            Blueprint {
                events: vec![],
                hang_when_empty: false,
            },
        ]);
        let mgr = manager(&dir, spawn.clone(), Config::default());

        mgr.ensure_warm().await;
        assert_eq!(spawn.spawn_count.load(Ordering::SeqCst), 1);
        mgr.respawn_warm().await;
        // Old warm killed, a fresh one spawned in its place.
        assert_eq!(spawn.kills.load(Ordering::SeqCst), 1);
        assert_eq!(spawn.spawn_count.load(Ordering::SeqCst), 2);
        assert!(mgr.inner.warm.lock().await.child.is_some());
    }

    #[test]
    fn stream_event_maps_onto_wire_events() {
        use claude_commander_protocol::api::CommanderEvent as Wire;
        assert_eq!(
            stream_event_to_wire(StreamEvent::Delta("x".into())),
            Wire::Delta { text: "x".into() }
        );
        assert_eq!(
            stream_event_to_wire(StreamEvent::TurnComplete),
            Wire::TurnComplete
        );
        // The internal `Exited` lifecycle event degrades to a wire error.
        assert!(matches!(
            stream_event_to_wire(StreamEvent::Exited),
            Wire::Error { .. }
        ));
    }

    #[test]
    fn key_entry_is_stable_per_key_and_distinct_across_keys() {
        // Same key → same entry (so asks share one lock → serialized). Different
        // keys → different entries (so they run concurrently).
        let dir = TempDir::new().unwrap();
        let spawn = FakeSpawn::new(vec![]);
        let mgr = manager(&dir, spawn, Config::default());
        let a1 = mgr.key_entry("slack:C:A");
        let a2 = mgr.key_entry("slack:C:A");
        let b = mgr.key_entry("slack:C:B");
        assert!(Arc::ptr_eq(&a1, &a2));
        assert!(!Arc::ptr_eq(&a1, &b));
    }

    /// A child that overlaps with concurrently-running children: it records the
    /// peak number simultaneously mid-turn, so a test can prove different keys
    /// run in parallel while same-key asks serialize.
    struct ConcurrencyChild {
        counters: Arc<Counters>,
        done: bool,
    }
    #[derive(Default)]
    struct Counters {
        current: AtomicUsize,
        peak: AtomicUsize,
    }
    #[async_trait]
    impl CommanderChild for ConcurrencyChild {
        async fn send(&mut self, _text: &str) -> Result<()> {
            Ok(())
        }
        async fn recv(&mut self) -> Option<StreamEvent> {
            if self.done {
                return None;
            }
            self.done = true;
            let cur = self.counters.current.fetch_add(1, Ordering::SeqCst) + 1;
            self.counters.peak.fetch_max(cur, Ordering::SeqCst);
            // Yield so a concurrently-running turn can also enter before we exit.
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
            self.counters.current.fetch_sub(1, Ordering::SeqCst);
            Some(StreamEvent::TurnComplete)
        }
        async fn kill(&mut self) {}
    }

    struct ConcurrencySpawn {
        counters: Arc<Counters>,
    }
    #[async_trait]
    impl CommanderSpawn for ConcurrencySpawn {
        async fn spawn(&self, _spec: SpawnSpec) -> Result<Box<dyn CommanderChild>> {
            Ok(Box::new(ConcurrencyChild {
                counters: self.counters.clone(),
                done: false,
            }))
        }
    }

    fn concurrency_manager(dir: &TempDir, counters: Arc<Counters>) -> HeadlessCommander {
        let mut config = Config::default();
        config.slack.warm_pool = false;
        let config_store = Arc::new(ConfigStore::with_path(
            config,
            dir.path().join("config.toml"),
        ));
        let sessions = Arc::new(SlackSessionStore::load(dir.path().join("slack.json")));
        HeadlessCommander::new(
            config_store,
            "# primed".to_string(),
            dir.path().join("commander"),
            Arc::new(ConcurrencySpawn {
                counters: counters.clone(),
            }),
            sessions,
            Vec::new(),
        )
    }

    #[tokio::test]
    async fn different_keys_run_concurrently() {
        let dir = TempDir::new().unwrap();
        let counters = Arc::new(Counters::default());
        let mgr = concurrency_manager(&dir, counters.clone());

        let a = mgr.ask("slack:C:A", "one");
        let b = mgr.ask("slack:C:B", "two");
        drain(a).await;
        drain(b).await;
        // Both turns overlapped mid-flight.
        assert_eq!(counters.peak.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn same_key_asks_are_serialized() {
        let dir = TempDir::new().unwrap();
        let counters = Arc::new(Counters::default());
        let mgr = concurrency_manager(&dir, counters.clone());

        let a = mgr.ask("slack:C:A", "one");
        let b = mgr.ask("slack:C:A", "two");
        drain(a).await;
        drain(b).await;
        // The second ask waited for the first to release the key: never overlapped.
        assert_eq!(counters.peak.load(Ordering::SeqCst), 1);
    }
}
