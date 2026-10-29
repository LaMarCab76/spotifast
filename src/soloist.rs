//! An optional, separately downloaded Spotify Connect audio receiver.
//!
//! The existing Web API remains the control path. No audio, credentials or
//! library requests are forwarded from librespot to Soloist. The receiver's
//! local event stream is used only to report pairing and lifecycle state.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout};
use tokio::sync::mpsc;

use crate::backend::Event;
use crate::backend::Waker;
use crate::credentials::{Grant, Slot, Store};
use crate::http::Http;
use crate::paths::AppDirs;

pub const GUIDE: &str =
    "https://developer.spotify.com/documentation/soloist/tutorials/getting-started";
pub const DASHBOARD: &str = "https://developer.spotify.com/dashboard/soloist";
const BUILD_LIFETIME: u64 = 90 * 24 * 60 * 60;
const MAX_ARCHIVE: usize = 64 * 1024 * 1024;

// AppKit's Quit can call exit without returning to the resident shell.
// Keep only our still-unreaped receiver here, never a pid read from disk.
#[cfg(unix)]
mod exit_cleanup {
    use std::sync::{Mutex, Once};
    static CHILD: Mutex<Option<u32>> = Mutex::new(None);
    unsafe extern "C" {
        fn atexit(callback: extern "C" fn()) -> std::ffi::c_int;
        fn kill(pid: std::ffi::c_int, signal: std::ffi::c_int) -> std::ffi::c_int;
    }

    extern "C" fn at_exit() {
        stop(None);
    }

    pub fn track(pid: u32) {
        static REGISTER: Once = Once::new();
        REGISTER.call_once(|| {
            // SAFETY: this C callback takes no arguments and does not unwind.
            if unsafe { atexit(at_exit) } != 0 {
                log::warn!("Could not register Soloist exit cleanup");
            }
        });
        *CHILD.lock().unwrap_or_else(|p| p.into_inner()) = Some(pid);
    }

    pub fn stop(only: Option<u32>) {
        let mut child = CHILD.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(pid) = *child
            && only.is_none_or(|only| only == pid)
        {
            // SAFETY: this pid is still our unreaped child. Holding the same
            // lock as poll prevents signalling a pid after it can be reused.
            unsafe {
                kill(pid as std::ffi::c_int, 15);
            }
            *child = None;
        }
    }

    pub fn poll(child: &mut tokio::process::Child) -> Option<std::process::ExitStatus> {
        let mut tracked = CHILD.lock().unwrap_or_else(|p| p.into_inner());
        let pid = child.id();
        let exited = child.try_wait().ok().flatten();
        if exited.is_some() && pid == *tracked {
            *tracked = None;
        }
        exited
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlaybackEngine {
    #[default]
    Librespot,
    Soloist,
}

/// Usable secret. Debug deliberately reveals neither its contents nor length.
#[derive(Clone)]
pub struct ApiKey(pub String);

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SoloistApiKey([redacted])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    pub version: String,
    pub expires_at: u64,
}

impl Build {
    pub fn days_left(&self) -> u64 {
        self.expires_at.saturating_sub(now()) / 86_400
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Stopped,
    Preparing,
    NeedsProgram,
    NeedsKey,
    Pairing,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub build: Option<Build>,
    pub key_saved: bool,
    pub device_name: String,
    pub phase: Phase,
}

impl Status {
    pub fn running(&self) -> bool {
        matches!(self.phase, Phase::Pairing | Phase::Ready)
    }
}

#[derive(Debug)]
pub enum Request {
    Inspect,
    Install,
    SaveKey(ApiKey),
    Start(String, u8),
    Stop,
    ForgetSession,
}

pub fn supported() -> bool {
    download_url().is_some()
}

fn download_url() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => {
            Some("https://soloist-builds.spotifycdn.com/soloist_release_osx_arm64.tar.gz")
        }
        ("windows", "x86_64") => {
            Some("https://soloist-builds.spotifycdn.com/soloist_release_windows_x86_64.zip")
        }
        ("linux", "aarch64") => {
            Some("https://soloist-builds.spotifycdn.com/soloist_release_arm64.tar.gz")
        }
        ("linux", "arm") => {
            Some("https://soloist-builds.spotifycdn.com/soloist_release_arm32.tar.gz")
        }
        ("linux", "x86_64") => {
            Some("https://soloist-builds.spotifycdn.com/soloist_release_x86_64.tar.gz")
        }
        _ => None,
    }
}

fn executable() -> &'static str {
    if cfg!(windows) {
        "soloist.exe"
    } else {
        "soloist"
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parse_build(output: &str) -> Result<Build, String> {
    let words: Vec<_> = output.split_whitespace().collect();
    if words.first() != Some(&"soloist") {
        return Err("The downloaded program did not identify itself as Soloist.".into());
    }
    let version = words.get(1).ok_or("Soloist did not report its version.")?;
    let built: u64 = words
        .windows(2)
        .find_map(|w| (w[0] == "build").then(|| w[1].parse().ok()).flatten())
        .ok_or("Soloist did not report its build date.")?;
    let expires_at = built
        .checked_add(BUILD_LIFETIME)
        .ok_or("Invalid Soloist build date.")?;
    if built > now().saturating_add(86_400) {
        return Err("The Soloist build date is in the future. Check the computer's clock.".into());
    }
    Ok(Build {
        version: (*version).into(),
        expires_at,
    })
}

fn quiet(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}

fn verify_binary(path: &Path) -> Result<(), String> {
    if !path
        .symlink_metadata()
        .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
    {
        return Err("Soloist is missing or is not a regular executable.".into());
    }
    #[cfg(target_os = "macos")]
    {
        let result = std::process::Command::new("/usr/bin/codesign")
            .args([
                "--verify",
                "--strict",
                "-R",
                "=anchor apple generic and certificate leaf[subject.OU] = \"2FNC3A47ZF\"",
            ])
            .arg(path)
            .output()
            .map_err(|_| "Could not verify Spotify's code signature.")?;
        if !result.status.success() {
            return Err("Spotify's Soloist signature could not be verified. Download it again or check macOS trust services.".into());
        }
    }
    #[cfg(windows)]
    {
        let result = quiet(std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command",
                "$s = Get-AuthenticodeSignature -LiteralPath $env:SPOTIFAST_VERIFY; if ($s.Status -ne 'Valid' -or $s.SignerCertificate.Subject -notmatch 'Spotify') { exit 1 }"])
            .env("SPOTIFAST_VERIFY", path))
            .output().map_err(|_| "Could not verify Spotify's code signature.")?;
        if !result.status.success() {
            return Err("Spotify's Soloist signature could not be verified.".into());
        }
    }
    Ok(())
}

fn read_build(path: &Path) -> Result<Build, String> {
    verify_binary(path)?;
    let result = quiet(std::process::Command::new(path).arg("--version"))
        .output()
        .map_err(|_| "Soloist could not run on this computer.")?;
    if !result.status.success() {
        return Err("Soloist could not report its version. Check its system requirements.".into());
    }
    parse_build(&String::from_utf8_lossy(&result.stdout))
}

async fn inspect(path: PathBuf) -> Result<Option<Build>, String> {
    tokio::task::spawn_blocking(move || {
        if !path
            .try_exists()
            .map_err(|_| "Could not inspect the Soloist installation.")?
        {
            return Ok(None);
        }
        read_build(&path).map(Some)
    })
    .await
    .map_err(|_| "Soloist inspection stopped unexpectedly.")?
}

async fn install(dirs: AppDirs, http: Http) -> Result<Build, String> {
    let url = download_url().ok_or("Spotify does not publish Soloist for this platform.")?;
    let mut response = http
        .client()?
        .get(url)
        .send()
        .await
        .map_err(|_| "Could not download Soloist from Spotify.")?
        .error_for_status()
        .map_err(|_| "Spotify could not supply the Soloist download.")?;
    if response.url().scheme() != "https"
        || response.url().host_str() != Some("soloist-builds.spotifycdn.com")
    {
        return Err("The Soloist download left Spotify's official HTTPS server.".into());
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_ARCHIVE as u64)
    {
        return Err("The Soloist archive exceeds the download limit.".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Soloist download was interrupted.")?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_ARCHIVE {
            return Err("The Soloist archive exceeds the download limit.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    tokio::task::spawn_blocking(move || {
        let root = dirs.state.join("soloist-program");
        std::fs::create_dir_all(&root)
            .map_err(|_| "Could not create the Soloist program folder.")?;
        let stage = root.join(format!("download-{:016x}", rand::random::<u64>()));
        std::fs::create_dir(&stage).map_err(|_| "Could not prepare the Soloist download.")?;
        let result = (|| {
            let archive = stage.join("archive");
            fastframe_file::write(&archive, &bytes)
                .map_err(|_| "Could not save the Soloist archive.")?;
            // Extract exactly one root member. Never unpack arbitrary paths,
            // links, helper scripts or the whole archive into the app's state.
            let result = quiet(
                std::process::Command::new("tar")
                    .arg("-xf")
                    .arg(&archive)
                    .arg("-C")
                    .arg(&stage)
                    .arg(executable()),
            )
            .output()
            .map_err(|_| "The system archive extractor is unavailable.")?;
            if !result.status.success() {
                return Err(
                    "The Spotify archive did not contain the expected Soloist executable.".into(),
                );
            }
            let candidate = stage.join(executable());
            let build = read_build(&candidate)?;
            if build.expires_at <= now() {
                return Err("Spotify supplied an expired Soloist build. Try again later.".into());
            }
            let destination = root.join(executable());
            // fastframe-file atomically replaces the executable on all targets.
            let program = std::fs::read(&candidate)
                .map_err(|_| "Could not read the verified Soloist program.")?;
            fastframe_file::write(&destination, &program)
                .map_err(|_| "Could not install Soloist.")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| "Could not make Soloist executable.")?;
            }
            verify_binary(&destination)?;
            Ok(build)
        })();
        let _ = std::fs::remove_dir_all(stage);
        result
    })
    .await
    .map_err(|_| "Soloist installation stopped unexpectedly.")?
}

/// One actor owns both children. Cancellation drops kill-on-drop handles,
/// including when the backend runtime shuts down during pairing or startup.
pub(crate) struct Controller {
    tx: mpsc::UnboundedSender<Request>,
    task: tokio::task::JoinHandle<()>,
}

impl Controller {
    pub fn spawn(
        dirs: AppDirs,
        http: Http,
        store: Store,
        events: std::sync::mpsc::Sender<Event>,
        waker: Waker,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run(dirs, http, store, events, waker, rx));
        Self { tx, task }
    }

    pub fn send(&self, request: Request) {
        let _ = self.tx.send(request);
    }

    pub async fn shutdown(self) {
        let Self { tx, mut task } = self;
        let _ = tx.send(Request::Stop);
        drop(tx);
        if tokio::time::timeout(Duration::from_secs(5), &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

type Reader = Lines<BufReader<ChildStdout>>;

/// Clear exit registration before Tokio can reap a child during cancellation.
/// Otherwise a later process exit could signal a reused process id.
struct Receiver(Option<Child>);

impl Drop for Receiver {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0.as_ref().and_then(Child::id) {
            exit_cleanup::stop(Some(pid));
        }
    }
}

async fn stop_receiver(receiver: &mut Option<Receiver>) {
    if let Some(mut receiver) = receiver.take() {
        terminate(&mut receiver.0).await;
    }
}

async fn next_event(reader: &mut Option<Reader>) -> Option<String> {
    match reader {
        Some(reader) => reader.next_line().await.ok().flatten(),
        None => std::future::pending().await,
    }
}

fn logged_in(line: &str) -> Option<bool> {
    if line.len() > 65_536 {
        return None;
    }
    let start = line.find('{')?;
    let event: serde_json::Value = serde_json::from_str(&line[start..]).ok()?;
    (event["type"] == "auth_state")
        .then(|| event["logged_in"].as_bool())
        .flatten()
}

async fn terminate(child: &mut Option<Child>) {
    if let Some(mut child) = child.take() {
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            exit_cleanup::stop(Some(pid));
        }
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            let _ = tokio::process::Command::new("/bin/kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .await;
        }
        #[cfg(windows)]
        let _ = child.start_kill();
        if tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
        }
    }
}

fn spawn_command(path: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(path);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command
}

async fn run(
    dirs: AppDirs,
    http: Http,
    store: Store,
    events: std::sync::mpsc::Sender<Event>,
    waker: Waker,
    mut requests: mpsc::UnboundedReceiver<Request>,
) {
    let path = dirs.state.join("soloist-program").join(executable());
    let data = dirs.state.join("soloist-session");
    let cache = dirs.cache.join("soloist");
    let mut status = Status::default();
    let mut daemon: Option<Receiver> = None;
    let mut trace: Option<Child> = None;
    let mut reader: Option<Reader> = None;
    let mut wanted: Option<(String, u8)> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let emit = |status: &Status| {
        let _ = events.send(Event::Soloist(status.clone()));
        waker.wake();
    };
    loop {
        tokio::select! {
            request = requests.recv() => {
                let Some(request) = request else { break };
                let start = match request {
                    Request::Stop | Request::ForgetSession => {
                        let previous = status.clone();
                        wanted = None;
                        reader = None;
                        terminate(&mut trace).await;
                        stop_receiver(&mut daemon).await;
                        if matches!(request, Request::ForgetSession)
                            && let Err(error) = tokio::fs::remove_dir_all(&data).await
                                && error.kind() != std::io::ErrorKind::NotFound
                            {
                                status.phase = Phase::Failed("Could not remove the old Soloist session. Check folder permissions before pairing another account.".into());
                                emit(&status);
                                continue;
                        }
                        status.phase = Phase::Stopped;
                        if status != previous { emit(&status); }
                        false
                    }
                    Request::Inspect => {
                        match inspect(path.clone()).await {
                            Ok(build) => status.build = build,
                            Err(error) => status.phase = Phase::Failed(error),
                        }
                        match store.lease(Slot::Soloist).load().await {
                            Ok(loaded) => status.key_saved = matches!(loaded.grant, Some(Grant::Soloist(_))),
                            Err(error) => status.phase = Phase::Failed(error.to_string()),
                        }
                        emit(&status);
                        false
                    }
                    Request::Install => {
                        if daemon.is_some() {
                            status.phase = Phase::Failed("Select librespot before updating Soloist.".into());
                            emit(&status);
                            continue;
                        }
                        status.phase = Phase::Preparing;
                        emit(&status);
                        match install(dirs.clone(), http.clone()).await {
                            Ok(build) => { status.build = Some(build); status.phase = Phase::Stopped; }
                            Err(error) => { status.phase = Phase::Failed(error); emit(&status); continue; }
                        }
                        emit(&status);
                        wanted.is_some()
                    }
                    Request::SaveKey(key) => {
                        match store.lease(Slot::Soloist).save(Grant::Soloist(key.0.trim().into())).await {
                            Ok(()) => status.key_saved = true,
                            Err(error) => { status.phase = Phase::Failed(error.to_string()); emit(&status); continue; }
                        }
                        reader = None;
                        terminate(&mut trace).await;
                        stop_receiver(&mut daemon).await;
                        status.phase = Phase::Stopped;
                        emit(&status);
                        wanted.is_some()
                    }
                    Request::Start(name, volume) => { wanted = Some((name, volume.min(100))); true }
                };
                if !start || daemon.is_some() { continue; }
                status.phase = Phase::Preparing;
                emit(&status);
                let build = match inspect(path.clone()).await {
                    Ok(Some(build)) if build.expires_at > now() => build,
                    Ok(Some(_)) => { status.phase = Phase::Failed("Soloist has expired. Select librespot, then download a newer build.".into()); emit(&status); continue; }
                    Ok(None) => { status.phase = Phase::NeedsProgram; emit(&status); continue; }
                    Err(error) => { status.phase = Phase::Failed(error); emit(&status); continue; }
                };
                status.build = Some(build);
                let key = match store.lease(Slot::Soloist).load().await {
                    Ok(loaded) => match loaded.grant {
                        Some(Grant::Soloist(key)) => key,
                        _ => { status.key_saved = false; status.phase = Phase::NeedsKey; emit(&status); continue; }
                    },
                    Err(error) => { status.phase = Phase::Failed(error.to_string()); emit(&status); continue; }
                };
                status.key_saved = true;
                let hostname = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| {
                    std::process::Command::new("hostname").output().ok()
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                        .unwrap_or_else(|| "this computer".into())
                });
                status.device_name = format!("{} Soloist ({})", wanted.as_ref().map_or("Spotifast", |(name, _)| name.as_str()), hostname)
                    .chars().filter(|c| !c.is_control()).take(100).collect();
                if tokio::fs::create_dir_all(&data).await.is_err() || tokio::fs::create_dir_all(&cache).await.is_err() {
                    status.phase = Phase::Failed("Could not create Soloist's private folders.".into()); emit(&status); continue;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if tokio::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).await.is_err() {
                        status.phase = Phase::Failed("Could not protect the Soloist session folder.".into());
                        emit(&status);
                        continue;
                    }
                }
                match spawn_command(&path).args(["--device-name", &status.device_name, "--api-key", &key])
                    .arg("--data-dir").arg(&data).arg("--cache-dir").arg(&cache)
                    .args(["--cache-size", "512", "--initial-volume", &wanted.as_ref().map_or(0, |(_, volume)| *volume).to_string(), "--ws", "127.0.0.1:0"])
                    .stdout(Stdio::null()).spawn() {
                    Ok(child) => {
                        #[cfg(unix)]
                        if let Some(pid) = child.id() { exit_cleanup::track(pid); }
                        daemon = Some(Receiver(Some(child)));
                        status.phase = Phase::Pairing;
                    }
                    Err(_) => status.phase = Phase::Failed("Soloist could not start on this computer.".into()),
                }
                emit(&status);
            }
            line = next_event(&mut reader) => {
                match line {
                    Some(line) => if let Some(paired) = logged_in(&line) {
                        status.phase = if paired { Phase::Ready } else { Phase::Pairing };
                        emit(&status);
                    },
                    None => { reader = None; terminate(&mut trace).await; }
                }
            }
            _ = tick.tick(), if daemon.is_some() => {
                let exited = daemon.as_mut().and_then(|receiver| receiver.0.as_mut()).and_then(|child| {
                    #[cfg(unix)]
                    { exit_cleanup::poll(child) }
                    #[cfg(not(unix))]
                    { child.try_wait().ok().flatten() }
                });
                if let Some(exit) = exited {
                    daemon = None;
                    reader = None;
                    terminate(&mut trace).await;
                    status.phase = Phase::Failed(if exit.code() == Some(10) {
                        "Soloist has expired. Download a newer build.".into()
                    } else {
                        "Soloist stopped. Check the API key, Spotify Premium and network, then try again.".into()
                    });
                    emit(&status);
                } else if reader.is_none()
                    && tokio::fs::read_to_string(data.join("ws.addr")).await.is_ok_and(|addr| addr.trim() == "127.0.0.1")
                    && let Ok(mut child) = spawn_command(&path).arg("ctl").arg("--data-dir").arg(&data).arg("trace")
                        .stdout(Stdio::piped()).spawn() {
                        reader = child.stdout.take().map(|out| BufReader::new(out).lines());
                        trace = Some(child);
                }
            }
        }
    }
    terminate(&mut trace).await;
    stop_receiver(&mut daemon).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_even_inside_requests() {
        assert_eq!(
            format!("{:?}", Request::SaveKey(ApiKey("private-key".into()))),
            "SaveKey(SoloistApiKey([redacted]))"
        );
    }

    #[test]
    fn only_authentication_events_finish_pairing() {
        assert_eq!(
            logged_in(r#"123 {"type":"auth_state","logged_in":true}"#),
            Some(true)
        );
        assert_eq!(
            logged_in(r#"123 {"type":"auth_state","logged_in":false}"#),
            Some(false)
        );
        assert_eq!(
            logged_in(r#"123 {"type":"playback_state","status":"playing"}"#),
            None
        );
        assert_eq!(logged_in("not json"), None);
        assert_eq!(
            logged_in(r#"{"type":"auth_state","logged_in":"true"}"#),
            None
        );
    }

    #[test]
    fn build_expiry_uses_compilation_date() {
        let build =
            parse_build("soloist 1.3.10.24 build 1793210468 (20261028) (macos/aarch64)").unwrap();
        assert_eq!(build.version, "1.3.10.24");
        assert_eq!(build.expires_at, 1793210468 + BUILD_LIFETIME);
        assert!(parse_build("other 1.0 build 1793210468").is_err());
        assert!(parse_build("soloist 1.0 build invalid").is_err());
    }

    #[test]
    fn existing_settings_keep_librespot() {
        let settings: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.playback_engine, PlaybackEngine::Librespot);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stopping_a_child_waits_for_its_exit() {
        let child = spawn_command(Path::new("/bin/sleep"))
            .arg("30")
            .spawn()
            .unwrap();
        let mut child = Some(child);
        terminate(&mut child).await;
        assert!(child.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn appkit_exit_stops_the_receiver_even_without_backend_shutdown() {
        const FLAG: &str = "SPOTIFAST_TEST_SOLOIST_EXIT";
        if std::env::var_os(FLAG).is_some() {
            let child = std::process::Command::new("/bin/sleep")
                .arg("30")
                .spawn()
                .unwrap();
            exit_cleanup::track(child.id());
            println!("receiver started");
            std::process::exit(0);
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "soloist::tests::appkit_exit_stops_the_receiver_even_without_backend_shutdown",
                "--nocapture",
            ])
            .env(FLAG, "1")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut text = String::new();
            output.read_to_string(&mut text).unwrap();
            let _ = tx.send(text);
        });
        let output = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("receiver outlived its parent");
        assert!(output.contains("receiver started"));
        assert!(child.wait().unwrap().success());
    }
}
