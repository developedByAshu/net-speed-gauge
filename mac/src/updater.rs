//! Check for updates on the Mac. A background thread asks GitHub for the latest release with the
//! Mac's own curl, downloads the app's zip and SHA256SUMS.txt, checks the sha256, unzips it with
//! ditto and checks it is NetSpeedGauge. The app runs on one thread, so each step waits here for the
//! next tick to pick it up. Once Settings is closed a small shell script waits for this copy to quit,
//! swaps the two apps and opens the new one. Nothing about the user is sent: only the app's name, as
//! the User-Agent.

use crate::clock::unix_now;
use netspeedgauge_shared::update::{asset_url, expected_sha, is_newer, is_sha_match, latest_release_url, mac_asset_name, release_tag, user_agent, UpdateStatus};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

/// The program's name in the release files, the app bundle and the User-Agent.
const PROGRAM_NAME: &str = "NetSpeedGauge";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const SUMS_NAME: &str = "SHA256SUMS.txt";
const RELEASE_ACCEPT: &str = "application/vnd.github+json";
const FILE_ACCEPT: &str = "application/octet-stream";
const CURL: &str = "/usr/bin/curl";
const DITTO: &str = "/usr/bin/ditto";
const PLIST_BUDDY: &str = "/usr/libexec/PlistBuddy";
const SHELL: &str = "/bin/sh";
const TIMEOUT_SECONDS: &str = "30";
// Far above the app's size, so a broken answer cannot fill the disk.
const MAX_DOWNLOAD_BYTES: &str = "67108864";
/// The folder under the Mac's temporary folder that holds the download until it goes in.
const WORK_FOLDER_NAME: &str = "NetSpeedGauge-update";
const UNPACKED_FOLDER_NAME: &str = "unpacked";
const SCRIPT_NAME: &str = "install.sh";
const PROBE_NAME: &str = ".NetSpeedGauge-update-probe";
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/**
 * Swaps the apps once this copy has quit. $1 is this copy's process id, $2 the installed app, $3 the
 * new app, $4 where the installed app waits aside, $5 the update folder. The old app goes back if the
 * new one cannot go in, and whichever app is in place opens again.
 */
const INSTALL_SCRIPT: &str = r#"#!/bin/sh
while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
if mv "$2" "$4"; then
    if mv "$3" "$2"; then
        rm -rf "$4"
    else
        mv "$4" "$2"
    fi
fi
open "$2"
rm -rf "$5"
"#;

/// What a finished check hands the main thread.
pub struct Outcome {
    /// When GitHub answered, to save as the last check; None when it could not be reached.
    pub checked_at: Option<u64>,
    /// The checked new app, unzipped in the update folder.
    pub new_copy: Option<PathBuf>,
}

static STATUS: Mutex<UpdateStatus> = Mutex::new(UpdateStatus::Idle);
static OUTCOME: Mutex<Option<Outcome>> = Mutex::new(None);
static IS_BUSY: AtomicBool = AtomicBool::new(false);
static HAS_NEW_STEP: AtomicBool = AtomicBool::new(false);

/// Returns where the Check now row stands.
pub fn status() -> UpdateStatus {
    STATUS.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Sets the row's state and leaves the step for the next tick, which saves it and redraws Settings.
fn set_status(status: UpdateStatus) {
    *STATUS.lock().unwrap_or_else(PoisonError::into_inner) = status;
    HAS_NEW_STEP.store(true, Ordering::SeqCst);
}

/// Returns true once after each step of a check, for the tick to pick up.
pub fn take_new_step() -> bool {
    HAS_NEW_STEP.swap(false, Ordering::SeqCst)
}

/// Takes the last finished check, once.
pub fn take_outcome() -> Option<Outcome> {
    OUTCOME.lock().unwrap_or_else(PoisonError::into_inner).take()
}

/// Returns the time now in seconds since 1970.
pub fn now_seconds() -> u64 {
    u64::try_from(unix_now()).unwrap_or(0)
}

/// Starts a check on a background thread, unless one is running. Each step reaches `app::on_update` on the next tick.
pub fn check_now() {
    if IS_BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        set_status(UpdateStatus::Checking);
        let (status, outcome) = run_check();
        *OUTCOME.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
        IS_BUSY.store(false, Ordering::SeqCst);
        set_status(status);
    });
}

/// Asks GitHub for the latest release and, when it is newer, downloads, checks and unzips it.
fn run_check() -> (UpdateStatus, Outcome) {
    let agent = user_agent(PROGRAM_NAME);
    let unreachable = (UpdateStatus::Failed, Outcome { checked_at: None, new_copy: None });
    let Some(json) = latest_release_url(REPOSITORY).and_then(|url| fetch(&url, &agent, RELEASE_ACCEPT)).and_then(|body| String::from_utf8(body).ok()) else {
        return unreachable;
    };
    let checked = Outcome { checked_at: Some(now_seconds()), new_copy: None };
    let Some(tag) = release_tag(&json) else { return (UpdateStatus::Failed, checked) };
    if !is_newer(VERSION, &tag) {
        return (UpdateStatus::UpToDate, checked);
    }
    set_status(UpdateStatus::Downloading(tag.clone()));
    match download_checked(&json, &agent).and_then(|zip| unpack(&zip)) {
        Some(new_copy) => (UpdateStatus::Ready(tag), Outcome { new_copy: Some(new_copy), ..checked }),
        None => (UpdateStatus::Failed, checked),
    }
}

/// Downloads the Mac zip and keeps it only when its sha256 matches SHA256SUMS.txt.
fn download_checked(json: &str, agent: &str) -> Option<Vec<u8>> {
    let asset = mac_asset_name(PROGRAM_NAME);
    let sums = fetch(&asset_url(json, SUMS_NAME)?, agent, FILE_ACCEPT).and_then(|body| String::from_utf8(body).ok())?;
    let expected = expected_sha(&sums, &asset)?;
    let zip = fetch(&asset_url(json, &asset)?, agent, FILE_ACCEPT)?;
    is_sha_match(&zip, &expected).then_some(zip)
}

/// Returns the folder that holds the download until it goes in.
fn work_folder() -> PathBuf {
    std::env::temp_dir().join(WORK_FOLDER_NAME)
}

/**
 * Unzips the checked zip into a fresh update folder and returns the new app, but only when its bundle
 * id is the running app's, so nothing else can be swapped in under NetSpeedGauge's name.
 */
fn unpack(zip: &[u8]) -> Option<PathBuf> {
    let folder = work_folder();
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).ok()?;
    let archive = folder.join(mac_asset_name(PROGRAM_NAME));
    let unpacked = folder.join(UNPACKED_FOLDER_NAME);
    let is_unzipped = std::fs::write(&archive, zip).is_ok()
        && Command::new(DITTO).args(["-x", "-k"]).arg(&archive).arg(&unpacked).stdin(Stdio::null()).status().is_ok_and(|status| status.success());
    let _ = std::fs::remove_file(&archive);
    let new_copy = unpacked.join(format!("{PROGRAM_NAME}.app"));
    let running_id = running_bundle().and_then(|bundle| bundle_identifier(&bundle));
    let is_ours = is_unzipped && running_id.is_some() && bundle_identifier(&new_copy) == running_id;
    if !is_ours {
        let _ = std::fs::remove_dir_all(&folder);
        return None;
    }
    Some(new_copy)
}

/// Returns the app bundle the running program sits in: …/NetSpeedGauge.app/Contents/MacOS/NetSpeedGauge gives …/NetSpeedGauge.app.
fn running_bundle() -> Option<PathBuf> {
    let program = std::env::current_exe().ok()?;
    let bundle = program.parent()?.parent()?.parent()?;
    bundle.extension().is_some_and(|extension| extension == "app").then(|| bundle.to_path_buf())
}

/// Reads an app's CFBundleIdentifier from its Info.plist, or None when it has none.
fn bundle_identifier(app: &Path) -> Option<String> {
    let plist = app.join("Contents").join("Info.plist");
    let output = Command::new(PLIST_BUDDY).args(["-c", "Print CFBundleIdentifier"]).arg(&plist).stdin(Stdio::null()).output().ok()?;
    let identifier = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && !identifier.is_empty()).then_some(identifier)
}

/// Returns true when a new folder can be made in `folder`, so the installed app can be moved there.
fn is_writable(folder: &Path) -> bool {
    let probe = folder.join(PROBE_NAME);
    let is_made = std::fs::create_dir(&probe).is_ok();
    let _ = std::fs::remove_dir(&probe);
    is_made
}

/**
 * Starts the install script, detached, and returns true; the caller quits right after. The new app
 * takes the installed app's exact path, so Start at login and the menu bar place stay as they are.
 * Returns false, with nothing changed, when this copy is not in an app bundle or its folder cannot
 * be written.
 */
pub fn install(new_copy: &Path) -> bool {
    let Some(installed) = running_bundle() else { return false };
    let Some(parent) = installed.parent() else { return false };
    if !is_writable(parent) {
        return false;
    }
    let folder = work_folder();
    let script = folder.join(SCRIPT_NAME);
    if std::fs::write(&script, INSTALL_SCRIPT).is_err() {
        return false;
    }
    let name = installed.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let set_aside = installed.with_file_name(format!("{name}.old"));
    let _ = std::fs::remove_dir_all(&set_aside);
    // Its own process group, so the script keeps running after this copy quits.
    Command::new(SHELL)
        .arg(&script)
        .arg(std::process::id().to_string())
        .arg(&installed)
        .arg(new_copy)
        .arg(&set_aside)
        .arg(&folder)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .is_ok()
}

/// Returns a time in seconds since 1970 as local "28 Nov 15:20", or None before the first check.
pub fn time_label(seconds: u64) -> Option<String> {
    if seconds == 0 {
        return None;
    }
    let time = libc::time_t::try_from(seconds).ok()?;
    // SAFETY: an all-zero tm is valid, and localtime_r only fills it in.
    let local = unsafe {
        let mut local: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut local).is_null() {
            return None;
        }
        local
    };
    let month = MONTHS[usize::try_from(local.tm_mon).ok()?.min(MONTHS.len() - 1)];
    Some(format!("{} {month} {:02}:{:02}", local.tm_mday, local.tm_hour, local.tm_min))
}

/// GETs an https address with the Mac's curl and returns the body of a successful answer. curl follows GitHub's redirect to its file store.
fn fetch(url: &str, agent: &str, accept: &str) -> Option<Vec<u8>> {
    let header = format!("Accept: {accept}");
    let output = Command::new(CURL)
        .args(["-fsSL", "--max-time", TIMEOUT_SECONDS, "--max-filesize", MAX_DOWNLOAD_BYTES, "-A", agent, "-H", header.as_str(), url])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}
