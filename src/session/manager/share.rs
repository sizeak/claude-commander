//! Multi-user session sharing — invite/join orchestration.
//!
//! Two halves of the same flow:
//!
//! - **Inviter side** ([`SessionManager::provision_invite`]): runs a
//!   sequence of commands on the codespace through the existing
//!   [`crate::remote::RemoteRunner`] — install/start `cloudflared`
//!   exposing port 22, generate a fresh ed25519 keypair locally, create
//!   a one-shot Linux user with the pubkey installed, share the
//!   inviter's tmux socket via group permissions. Returns a
//!   [`crate::share::JoinCode`] the UI can display + auto-copy.
//!
//! - **Joiner side** ([`SessionManager::join_shared_session`]): writes
//!   the embedded private key to a 0600 tempfile, starts a local
//!   `cloudflared access tcp` process binding the codespace tunnel to
//!   `localhost:<port>`, returns a [`JoinedShareTarget`] that the
//!   attach loop spawns `ssh -i <key> -p <port>` against.
//!
//! Spike-quality: cleanup of provisioned users + tunnels happens
//! implicitly when the codespace stops; no explicit "Uninvite" yet.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use chrono::Utc;
use tempfile::NamedTempFile;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::error::{Result, SessionError};
use crate::session::{RemoteTransport, SessionId, SharedUser};
use crate::share::JoinCode;
use crate::tmux::AttachTarget;

/// The result of `SessionManager::join_shared_session`. Owns the
/// short-lived resources that have to outlive the call site:
///
/// - `_key_file` keeps the on-disk SSH private key alive (and deletes it
///   on drop) until the attach loop finishes with it.
/// - `cloudflared_child` is the joiner-side `cloudflared access tcp`
///   process that bridges `localhost:<port>` to the codespace's tunnel.
///   Killed on drop so we don't leak background tunnels when the user
///   detaches.
///
/// `target` is what the attach loop in `app/mod.rs` reads — a
/// pre-populated [`AttachTarget::SharedSshTunnel`].
pub struct JoinedShareTarget {
    pub target: AttachTarget,
    /// The fresh `JoinCode` parsed from the user's input — kept for
    /// status-bar display ("Joined as ccshare-…").
    pub code: JoinCode,
    /// Tempfile holding the private key that backs `target`'s `key_path`.
    /// Field name starts with `_` so the borrow checker doesn't complain
    /// about it being unused — its Drop is the whole point.
    #[allow(dead_code)]
    pub(crate) _key_file: NamedTempFile,
    /// Local cloudflared `access tcp` process. Killed on drop.
    #[allow(dead_code)]
    pub(crate) cloudflared_child: ChildHandle,
}

impl std::fmt::Debug for JoinedShareTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinedShareTarget")
            .field("target", &self.target)
            .field("code.user", &self.code.user)
            .field("code.host", &self.code.host)
            .finish()
    }
}

impl Clone for JoinedShareTarget {
    fn clone(&self) -> Self {
        // `JoinedShareTarget` owns runtime resources that can't be
        // cloned (a `NamedTempFile` and a child process handle). Cloning
        // the wrapper would silently drop those on the original, killing
        // the tunnel and deleting the key out from under whoever held
        // the original. We only impl `Clone` because `StateUpdate`
        // derives it for trait-bound reasons; nothing in the codebase
        // actually invokes it at runtime.
        panic!(
            "JoinedShareTarget cannot be cloned — its tempfile + cloudflared \
             handle must have a single owner. Move it instead."
        )
    }
}

/// Wrapper that kills the held child process on drop. Used so the
/// joiner-side `cloudflared access tcp` doesn't outlive the attach.
pub struct ChildHandle(pub Option<tokio::process::Child>);

impl Drop for ChildHandle {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            // Best-effort. tokio's Child::start_kill returns immediately;
            // we don't await reaping (the OS will do it).
            let _ = c.start_kill();
        }
    }
}

impl super::SessionManager {
    /// Provision a one-shot share user + cloudflared tunnel on the
    /// codespace owning `session_id` and return a join code the inviter
    /// can hand to a collaborator.
    pub async fn provision_invite(&self, session_id: &SessionId) -> Result<JoinCode> {
        // ── Resolve the host from the session's project ─────────────────
        let (project_id, transport, tmux_session_name) = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            let project = state
                .get_project(&session.project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(session.project_id.to_string()))?;
            let transport = project
                .remote
                .clone()
                .ok_or(SessionError::InvalidState(*session_id))?;
            (
                session.project_id,
                transport,
                session.tmux_session_name.clone(),
            )
        };

        let runner = self.ssh_pool.get_or_connect(&transport).await?;

        // ── Generate ed25519 keypair locally ────────────────────────────
        // We write to a fresh tempfile and read the public/private halves
        // straight back. The tempfile dies as soon as this function
        // returns (private key has already been embedded in the
        // returned `JoinCode`; public half pushed remotely).
        let (private_key, public_key) = generate_ed25519_keypair().await?;

        // ── Pick a random username + push pubkey ────────────────────────
        let username = random_share_username();
        info!("Provisioning share user `{}` on remote", username);
        run_or_err(
            &*runner,
            &["sudo", "useradd", "-m", "-s", "/bin/bash", &username],
            "useradd",
        )
        .await?;

        let auth_dir = format!("/home/{username}/.ssh");
        let auth_keys = format!("/home/{username}/.ssh/authorized_keys");
        run_or_err(
            &*runner,
            &[
                "sudo", "install", "-d", "-o", &username, "-g", &username, "-m", "0700", &auth_dir,
            ],
            "install -d .ssh",
        )
        .await?;
        // Use `tee` (rather than `>`) because our remote runner doesn't
        // resolve shell redirection — argv goes straight to env.
        let pubkey_pipe = format!(
            "printf '%s\\n' {} | sudo tee {} >/dev/null",
            shell_quote(&public_key),
            shell_quote(&auth_keys)
        );
        run_or_err(&*runner, &["bash", "-c", &pubkey_pipe], "push pubkey").await?;
        run_or_err(
            &*runner,
            &[
                "sudo",
                "chown",
                &format!("{}:{}", username, username),
                &auth_keys,
            ],
            "chown authorized_keys",
        )
        .await?;
        run_or_err(
            &*runner,
            &["sudo", "chmod", "0600", &auth_keys],
            "chmod authorized_keys",
        )
        .await?;

        // ── Resolve the inviter's identity for the sudoers entry ────────
        // The share user can't attach to the inviter's tmux directly
        // (tmux refuses cross-uid access regardless of file perms — see
        // the sudoers note below). Capture the inviter's uid + username
        // so we can write a tightly-scoped sudoers rule that lets the
        // share user run *exactly* `tmux -S <socket> attach -t cc-*`
        // as the inviter, and nothing else.
        let inviter_uid = run_capture(&*runner, &["id", "-u"], "id -u").await?;
        let inviter_uid = inviter_uid.trim().to_string();
        let inviter_user = run_capture(&*runner, &["id", "-un"], "id -un").await?;
        let inviter_user = inviter_user.trim().to_string();
        let socket_path = format!("/tmp/tmux-{inviter_uid}/default");

        // ── Cloudflared bootstrap + start ───────────────────────────────
        // Detect the actual sshd listen port BEFORE starting the tunnel.
        // On a Codespace running the `ghcr.io/devcontainers/features/sshd:1`
        // feature, sshd binds 2222 (port 22 is GitHub's gh-tunnel daemon,
        // not real sshd — pointing cloudflared at 22 yields
        // `kex_exchange_identification: Connection reset by peer` on the
        // joiner side because the TCP connection lands on a non-SSH
        // service that immediately RSTs).
        let sshd_port = detect_sshd_port(&*runner).await.unwrap_or(2222);
        info!("Detected codespace sshd on port {}", sshd_port);

        let tunnel_host = ensure_cloudflared_running(&*runner, sshd_port).await?;

        // ── Set up the share user's environment ─────────────────────────
        // Two prerequisites for the joiner-side `sudo -u <inviter> tmux`:
        //
        // 1. Symlink the inviter's tmux into `/usr/local/bin/tmux` so
        //    the sudoers rule can name a stable absolute path that
        //    works on Nix-based and apt-based codespaces alike.
        //    `readlink -f` resolves user-home symlinks
        //    (`~/.nix-profile/bin/tmux`) to the underlying `/nix/store`
        //    entry, which is world-readable + dynamically self-
        //    contained.
        //
        // 2. Drop a `/etc/sudoers.d/cc-share-<user>` fragment that
        //    grants the share user permission to run *only* the
        //    specific `tmux -S <socket> attach -t cc-*` command as the
        //    inviter — no shell escape, no other tmux subcommand, no
        //    other user. tmux's `st_uid != getuid()` self-check rejects
        //    cross-uid attaches even with chmod, so running tmux *as*
        //    the inviter is the only way that doesn't fork tmux/tmate.
        //    `Defaults env_keep += "TERM"` preserves the terminal type
        //    through sudo so tmux gets correct color/key handling.
        let publish_tmux =
            "sudo ln -sf \"$(readlink -f \"$(command -v tmux)\")\" /usr/local/bin/tmux";
        run_or_err(
            &*runner,
            &["bash", "-ilc", publish_tmux],
            "symlink tmux into /usr/local/bin",
        )
        .await?;

        let sudoers_body = format!(
            "Defaults env_keep += \"TERM\"\n\
             {username} ALL=({inviter_user}) NOPASSWD: \
             /usr/local/bin/tmux -S {socket_path} attach -t cc-*\n"
        );
        // Use `tee` to write the file, then chmod 0440 (sudoers refuses
        // anything looser). Filename has no `.` (sudo skips
        // dot-prefixed/dotted files in /etc/sudoers.d).
        let sudoers_path = format!("/etc/sudoers.d/cc-share-{username}");
        let write_sudoers = format!(
            "printf '%s' {} | sudo tee {} > /dev/null && sudo chmod 0440 {}",
            shell_quote(&sudoers_body),
            shell_quote(&sudoers_path),
            shell_quote(&sudoers_path),
        );
        run_or_err(
            &*runner,
            &["bash", "-c", &write_sudoers],
            "write share user sudoers entry",
        )
        .await?;

        // ── Persist the share user on the project for accounting ────────
        let pid_for_mut = project_id;
        let new_user = SharedUser {
            username: username.clone(),
            created_at: Utc::now(),
            tunnel_url: Some(tunnel_host.clone()),
        };
        let _ = self
            .store
            .mutate(move |state| {
                if let Some(p) = state.get_project_mut(&pid_for_mut) {
                    p.shared_users.push(new_user);
                }
            })
            .await;

        let _transport_label = match &transport {
            RemoteTransport::Codespace { name } => format!("codespace {name}"),
            RemoteTransport::Ssh { host } => format!("ssh {host}"),
        };
        debug!("Invite ready: user={} tunnel={}", username, tunnel_host);

        Ok(JoinCode {
            host: tunnel_host,
            // Carry the sshd port for diagnostics — the joiner picks its
            // own free local port for the cloudflared client to bind, so
            // this value isn't load-bearing on the wire. Useful in the
            // log line + future "show invite details" UI.
            port: sshd_port,
            user: username,
            inviter_user,
            private_key,
            socket: PathBuf::from(socket_path),
            tmux_session: tmux_session_name,
        })
    }

    /// Joiner-side: turn a parsed `JoinCode` into a `JoinedShareTarget`
    /// the attach loop can consume.
    pub async fn join_shared_session(&self, code: JoinCode) -> Result<JoinedShareTarget> {
        // ── Write the private key to a 0600 tempfile ────────────────────
        let mut key_file = tempfile::Builder::new()
            .prefix("cc-share-key-")
            .tempfile()
            .map_err(|e| {
                SessionError::ShareFailed(format!("Failed to create SSH key tempfile: {e}"))
            })?;
        std::io::Write::write_all(key_file.as_file_mut(), code.private_key.as_bytes())
            .map_err(|e| SessionError::ShareFailed(format!("Write SSH key tempfile: {e}")))?;
        // chmod 0600 — ssh refuses to use a key file with looser perms.
        let mut perms = key_file
            .as_file()
            .metadata()
            .map_err(|e| SessionError::ShareFailed(format!("stat SSH key tempfile: {e}")))?
            .permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(key_file.path(), perms)
            .map_err(|e| SessionError::ShareFailed(format!("chmod SSH key tempfile: {e}")))?;

        let key_path = key_file.path().to_path_buf();

        // ── Pick a free local port + start joiner-side cloudflared ──────
        let local_port = pick_free_port().ok_or_else(|| {
            SessionError::ShareFailed("Could not find a free local port".to_string())
        })?;

        let cloudflared_bin = which_cloudflared().ok_or_else(|| {
            SessionError::ShareFailed(
                "cloudflared not found on PATH. Install from \
                 https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/"
                    .to_string(),
            )
        })?;

        info!(
            "Starting joiner cloudflared: {} access tcp --hostname {} --url localhost:{}",
            cloudflared_bin.display(),
            code.host,
            local_port
        );
        // Pipe cloudflared's stderr to a log file so failed joins leave
        // forensic data — when ssh exits non-zero in milliseconds it's
        // usually because the local port closed *before* ssh could read
        // anything, and cloudflared's stderr is the only place that
        // records why (e.g. "no such tunnel", "tunnel error", auth).
        let cloudflared_log_path = "/tmp/cc-share-cloudflared-client.log";
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(cloudflared_log_path)
            .map_err(|e| SessionError::ShareFailed(format!("open {cloudflared_log_path}: {e}")))?;
        let log_file_dup = log_file
            .try_clone()
            .map_err(|e| SessionError::ShareFailed(format!("clone log fd: {e}")))?;
        let child = Command::new(&cloudflared_bin)
            .args([
                "access",
                "tcp",
                "--hostname",
                &code.host,
                "--url",
                &format!("localhost:{local_port}"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_file_dup))
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| SessionError::ShareFailed(format!("Spawn cloudflared: {e}")))?;

        // Give cloudflared a moment to bind the local port. Crude but
        // adequate for a spike: wait until either the port accepts TCP
        // or we time out. Real impl would parse stderr for the listening
        // line.
        wait_for_port(local_port, Duration::from_secs(10)).await?;

        let target = AttachTarget::SharedSshTunnel {
            local_port,
            user: code.user.clone(),
            inviter_user: code.inviter_user.clone(),
            key_path,
            socket: code.socket.clone(),
            tmux_session: code.tmux_session.clone(),
        };

        Ok(JoinedShareTarget {
            target,
            code,
            _key_file: key_file,
            cloudflared_child: ChildHandle(Some(child)),
        })
    }
}

/// Generate a random `ccshare-<6-hex>` username.
pub(crate) fn random_share_username() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("ccshare-{}", &id[..6])
}

/// Single-quote a value for safe inclusion in a remote `bash -c` script.
fn shell_quote(s: &str) -> String {
    shell_escape::unix::escape(s.into()).into_owned()
}

/// Run `argv` over the remote runner, mapping a non-zero exit status to
/// an error with the supplied label so the user sees which step failed.
async fn run_or_err(
    runner: &dyn crate::remote::RemoteRunner,
    argv: &[&str],
    label: &str,
) -> Result<()> {
    let out = runner.run(argv).await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            format!("exit {:?}", out.status.code())
        };
        return Err(SessionError::ShareFailed(format!("{label} failed: {detail}")).into());
    }
    Ok(())
}

/// Run `argv` and return its stdout on success.
async fn run_capture(
    runner: &dyn crate::remote::RemoteRunner,
    argv: &[&str],
    label: &str,
) -> Result<String> {
    let out = runner.run(argv).await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(SessionError::ShareFailed(format!("{label} failed: {}", stderr.trim())).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Detect what port sshd is listening on inside the codespace.
///
/// Walks (in order) `ss -tlnp` output, then `/etc/ssh/sshd_config_development`
/// (the dev-container feature's config), then the fallback default
/// 2222. Port 22 on a codespace is GitHub's tunnel daemon, NOT sshd —
/// pointing cloudflared at port 22 produces
/// `kex_exchange_identification: Connection reset by peer` on the
/// joiner because the TCP connection lands on a non-SSH service.
async fn detect_sshd_port(runner: &dyn crate::remote::RemoteRunner) -> Result<u16> {
    // Strategy 1: `ss -tlnp` — most reliable, shows the actual bound
    // port. Output line for sshd looks like:
    //   LISTEN 0 128 0.0.0.0:2222 0.0.0.0:* users:(("sshd",pid=1234,fd=4))
    // We pull the port from the local-address column.
    if let Ok(out) = runner.run(&["bash", "-c", "ss -tlnp 2>/dev/null"]).await
        && out.status.success()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if !line.contains("sshd") {
                continue;
            }
            // Tokens: state recv-q send-q local addr peer addr [users]
            for tok in line.split_whitespace() {
                if let Some(port_str) = tok.rsplit(':').next()
                    && let Ok(port) = port_str.parse::<u16>()
                    && port != 0
                {
                    return Ok(port);
                }
            }
        }
    }

    // Strategy 2: parse the dev-container sshd feature's config file.
    // The feature writes `Port 2222` (or whatever SSHD_PORT was set to)
    // into `/etc/ssh/sshd_config_development`.
    for path in ["/etc/ssh/sshd_config_development", "/etc/ssh/sshd_config"] {
        if let Ok(out) = runner
            .run(&[
                "bash",
                "-c",
                &format!("grep -E '^Port ' {path} 2>/dev/null | head -1"),
            ])
            .await
            && out.status.success()
        {
            let line = String::from_utf8_lossy(&out.stdout);
            if let Some(port_str) = line.split_whitespace().nth(1)
                && let Ok(port) = port_str.parse::<u16>()
            {
                return Ok(port);
            }
        }
    }

    // Strategy 3: codespace devcontainer sshd-feature default.
    Ok(2222)
}

/// Make sure cloudflared is installed and a `quick tunnel` is running on
/// the remote, forwarding `localhost:<sshd_port>`. Returns the public
/// hostname the tunnel exposes.
///
/// Steady state: a single cloudflared process per codespace, log file
/// at `/tmp/cc-cloudflared.log`. If a previous invocation is still
/// running we reuse it by parsing the log instead of starting a new
/// tunnel — quick tunnels expire when their process dies.
async fn ensure_cloudflared_running(
    runner: &dyn crate::remote::RemoteRunner,
    sshd_port: u16,
) -> Result<String> {
    // Reuse case: if cloudflared is already running AND it's tunneling
    // to the right sshd port, parse its log for the existing hostname.
    // A leftover process from a prior invite that targeted a different
    // port is unsafe to reuse — kill it instead so we start fresh on
    // the right port.
    if let Ok(out) = runner.run(&["pgrep", "-af", "cloudflared tunnel"]).await
        && out.status.success()
        && !out.stdout.is_empty()
    {
        let cmdline = String::from_utf8_lossy(&out.stdout);
        let want = format!("--url tcp://localhost:{sshd_port}");
        if cmdline.contains(&want) {
            if let Ok(host) = parse_cloudflared_hostname_from_log(runner).await {
                debug!("Reusing existing cloudflared tunnel: {}", host);
                return Ok(host);
            }
            warn!("cloudflared running but log unreadable — restarting");
        } else {
            warn!(
                "cloudflared running on a different port (expected {}) — restarting",
                sshd_port
            );
        }
        let _ = runner.run(&["pkill", "-f", "cloudflared tunnel"]).await;
        // Wait briefly for the kill to settle before relaunching.
        tokio::time::sleep(Duration::from_millis(500)).await;
        // Truncate the log so we don't pick up the stale URL on the
        // next parse.
        let _ = runner
            .run(&["bash", "-c", ": >/tmp/cc-cloudflared.log"])
            .await;
    }

    // Install if missing.
    let installed = runner
        .run(&["bash", "-c", "command -v cloudflared >/dev/null"])
        .await?;
    if !installed.status.success() {
        info!("cloudflared not found — downloading");
        let install = "mkdir -p ~/.local/bin && \
             curl -sSL https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 \
                 -o ~/.local/bin/cloudflared && \
             chmod +x ~/.local/bin/cloudflared";
        run_or_err(runner, &["bash", "-c", install], "install cloudflared").await?;
    }

    // Start tunnel in background. nohup + & + log redirection so it
    // outlives the SSH command we used to launch it. The `--url
    // tcp://localhost:<port>` forwards arriving TCP connections to the
    // detected sshd port (NOT 22 — see `detect_sshd_port`).
    let start = format!(
        "nohup cloudflared tunnel --url tcp://localhost:{sshd_port} --no-autoupdate \
         >/tmp/cc-cloudflared.log 2>&1 &"
    );
    run_or_err(runner, &["bash", "-c", &start], "start cloudflared").await?;

    // Poll the log for the public hostname.
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(host) = parse_cloudflared_hostname_from_log(runner).await {
            return Ok(host);
        }
    }
    Err(SessionError::ShareFailed(
        "cloudflared started but didn't print a tunnel URL within 10s".to_string(),
    )
    .into())
}

/// Extract the `*.trycloudflare.com` URL from the cloudflared log file.
///
/// The log file is shared across ALL cloudflared invocations in a
/// codespace's lifetime — codespace stop+start cycles, manual kills,
/// OOM-restarts can all leave dead URLs in the log. Cloudflare quick
/// tunnels are bound to a specific cloudflared process so dead URLs
/// resolve to NXDOMAIN. We want the *most recent* URL, which is the
/// one belonging to the currently running cloudflared. Walk the log
/// bottom-up (or remember the last match) so we return the live one.
async fn parse_cloudflared_hostname_from_log(
    runner: &dyn crate::remote::RemoteRunner,
) -> Result<String> {
    let out = runner.run(&["cat", "/tmp/cc-cloudflared.log"]).await?;
    let log = String::from_utf8_lossy(&out.stdout);
    // Quick tunnel banner looks like:
    //   |  https://abc-def-ghi.trycloudflare.com                            |
    let mut latest: Option<String> = None;
    for line in log.lines() {
        if let Some(start) = line.find("https://")
            && let Some(rest) = line.get(start + "https://".len()..)
            && let Some(end) = rest.find(".trycloudflare.com")
        {
            let host_part = &rest[..end];
            latest = Some(format!("{host_part}.trycloudflare.com"));
        }
    }
    latest.ok_or_else(|| {
        SessionError::ShareFailed("no trycloudflare.com URL in log".to_string()).into()
    })
}

/// Locally generate an ed25519 keypair via `ssh-keygen` and read both
/// halves back. Returns `(private_key_pem, public_key_openssh)`.
async fn generate_ed25519_keypair() -> Result<(String, String)> {
    let dir = tempfile::tempdir()
        .map_err(|e| SessionError::ShareFailed(format!("tempdir for keygen: {e}")))?;
    let key_path = dir.path().join("id_ed25519");
    let key_path_str = key_path
        .to_str()
        .ok_or_else(|| SessionError::ShareFailed("non-UTF-8 tempdir for keygen".to_string()))?;
    let out = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-N",
            "",
            "-q",
            "-C",
            "claude-commander-share",
            "-f",
            key_path_str,
        ])
        .output()
        .await
        .map_err(|e| SessionError::ShareFailed(format!("spawn ssh-keygen: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(SessionError::ShareFailed(format!("ssh-keygen failed: {stderr}")).into());
    }
    let private = std::fs::read_to_string(&key_path)
        .map_err(|e| SessionError::ShareFailed(format!("read private key: {e}")))?;
    let public = std::fs::read_to_string(key_path.with_extension("pub"))
        .map_err(|e| SessionError::ShareFailed(format!("read public key: {e}")))?;
    Ok((private, public.trim().to_string()))
}

/// Pick a free local port by binding `localhost:0`, reading the port,
/// and dropping the socket. There's a tiny race where another process
/// could grab the port before cloudflared does, but for a spike this is
/// fine.
fn pick_free_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    listener.local_addr().ok().map(|a| a.port())
}

/// Wait until `localhost:port` accepts TCP connections, or `timeout`
/// elapses. Used to confirm cloudflared has bound the port before we
/// try to ssh through it.
async fn wait_for_port(port: u16, timeout: Duration) -> Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            return Err(SessionError::ShareFailed(format!(
                "cloudflared didn't open localhost:{port} within {timeout:?}"
            ))
            .into());
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Locate the `cloudflared` binary on PATH. Returns `None` if not found,
/// in which case the caller surfaces an install hint.
fn which_cloudflared() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("cloudflared");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

// `BufReader`/`AsyncBufReadExt` are imported for a future "tail the log
// over a streaming runner" follow-up; silence the unused warning until
// we wire that up.
#[allow(dead_code)]
fn _force_imports(reader: BufReader<tokio::io::Empty>) -> impl AsyncBufReadExt {
    reader
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_share_username_format() {
        let name = random_share_username();
        assert!(name.starts_with("ccshare-"));
        assert_eq!(name.len(), "ccshare-".len() + 6);
        let hex = &name[8..];
        assert!(
            hex.chars().all(|c| c.is_ascii_hexdigit()),
            "expected hex tail, got {hex:?}"
        );
    }

    #[test]
    fn random_share_username_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            seen.insert(random_share_username());
        }
        assert!(seen.len() > 80, "too many collisions: {}", seen.len());
    }

    #[test]
    fn pick_free_port_returns_a_port() {
        let port = pick_free_port().expect("should find a free port");
        assert!(port > 1024, "port {port} suspiciously low");
    }
}
