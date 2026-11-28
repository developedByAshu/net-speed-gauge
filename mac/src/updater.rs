//! Check for updates on the Mac. A background thread asks GitHub for the latest release with the
//! Mac's own curl, downloads SHA256SUMS.txt and its signature, checks the signature against the
//! release key, then downloads the app's zip, checks its sha256, unzips it with ditto and checks it
//! is NetSpeedGauge, signed by the same certificate as this copy. The app runs on one thread, so each
//! step waits here for the next tick to pick it up. Once Settings is closed the app asks before
//! installing; on Install a small shell script waits for this copy to quit, swaps the two apps and
//! opens the new one. Nothing about the user is sent: only the app's name, as the User-Agent.

use crate::clock::unix_now;
use crate::surface::main_thread;
use netspeedgauge_shared::update::{
    asset_url, expected_sha, install_question, is_allowed_url, is_newer, is_sha_match, is_signature_valid, latest_release_url, mac_asset_name,
    release_tag, signature_url, user_agent, UpdateStatus,
};
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSApplication};
use objc2_foundation::NSString;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
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
const CODESIGN: &str = "/usr/bin/codesign";
const PLIST_BUDDY: &str = "/usr/libexec/PlistBuddy";
const SHELL: &str = "/bin/sh";
const TIMEOUT_SECONDS: &str = "30";
// Far above the app's size, so a broken answer cannot fill the disk.
const MAX_DOWNLOAD_BYTES: &str = "67108864";
/// The app's own folder under ~/Library/Caches, readable only by the user, and the update folder in it.
const CACHE_FOLDER: &str = "Library/Caches/NetSpeedGauge";
const WORK_FOLDER_NAME: &str = "update";
const PRIVATE_FOLDER_MODE: u32 = 0o700;
const UNPACKED_FOLDER_NAME: &str = "unpacked";
const PROBE_NAME: &str = ".NetSpeedGauge-update-probe";
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/**
 * Swaps the apps once this copy has quit. It runs inline with sh -c, so no script file sits on disk. $1 is this copy's process id, $2 the installed app, $3 the
 * new app, $4 where the installed app waits aside, $5 the update folder. The old app goes back if the
 * new one cannot go in, and whichever app is in place opens again.
 */
const INSTALL_SCRIPT: &str = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
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
    /// The new copy's release tag, such as "v1.4.0", for the question before it goes in.
    pub tag: Option<String>,
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
    let unreachable = (UpdateStatus::Failed, Outcome { checked_at: None, new_copy: None, tag: None });
    let Some(json) = latest_release_url(REPOSITORY).and_then(|url| fetch(&url, &agent, RELEASE_ACCEPT)).and_then(|body| String::from_utf8(body).ok()) else {
        return unreachable;
    };
    let checked = Outcome { checked_at: Some(now_seconds()), new_copy: None, tag: None };
    let Some(tag) = release_tag(&json) else { return (UpdateStatus::Failed, checked) };
    if !is_newer(VERSION, &tag) {
        return (UpdateStatus::UpToDate, checked);
    }
    set_status(UpdateStatus::Downloading(tag.clone()));
    match download_checked(&json, &tag, &agent).and_then(|zip| unpack(&zip)) {
        Some(new_copy) => (UpdateStatus::Ready(tag.clone()), Outcome { new_copy: Some(new_copy), tag: Some(tag), ..checked }),
        None => (UpdateStatus::Failed, checked),
    }
}

/**
 * Downloads SHA256SUMS.txt and its signature, and only when the signature is the release key's
 * downloads the Mac zip, keeping it when its sha256 matches the signed sums. A missing or wrong
 * signature means no update; the next check tries again.
 */
fn download_checked(json: &str, tag: &str, agent: &str) -> Option<Vec<u8>> {
    let asset = mac_asset_name(PROGRAM_NAME);
    let sums = download(&asset_url(json, SUMS_NAME)?, agent, FILE_ACCEPT)?;
    let signature = download(&signature_url(REPOSITORY, tag)?, agent, FILE_ACCEPT).and_then(|body| String::from_utf8(body).ok())?;
    if !is_signature_valid(&sums, &signature) {
        return None;
    }
    let expected = expected_sha(std::str::from_utf8(&sums).ok()?, &asset)?;
    let zip = download(&asset_url(json, &asset)?, agent, FILE_ACCEPT)?;
    is_sha_match(&zip, &expected).then_some(zip)
}

/// Makes a folder readable only by the user, tightening it when it is already there.
fn make_private_folder(folder: &Path) -> Option<()> {
    std::fs::DirBuilder::new().recursive(true).mode(PRIVATE_FOLDER_MODE).create(folder).ok()?;
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(PRIVATE_FOLDER_MODE)).ok()
}

/// Returns the folder that holds the download until it goes in, under the user's own ~/Library/Caches.
fn work_folder() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(CACHE_FOLDER).join(WORK_FOLDER_NAME))
}

/**
 * Unzips the checked zip into a fresh update folder and returns the new app, but only when its bundle
 * id is the running app's and its code signature is valid and made by the same certificate, so
 * nothing else can be swapped in under NetSpeedGauge's name.
 */
fn unpack(zip: &[u8]) -> Option<PathBuf> {
    let folder = work_folder()?;
    let _ = std::fs::remove_dir_all(&folder);
    make_private_folder(folder.parent()?)?;
    make_private_folder(&folder)?;
    let archive = folder.join(mac_asset_name(PROGRAM_NAME));
    let unpacked = folder.join(UNPACKED_FOLDER_NAME);
    let is_unzipped = std::fs::write(&archive, zip).is_ok()
        && Command::new(DITTO).args(["-x", "-k"]).arg(&archive).arg(&unpacked).stdin(Stdio::null()).status().is_ok_and(|status| status.success());
    let _ = std::fs::remove_file(&archive);
    let new_copy = unpacked.join(format!("{PROGRAM_NAME}.app"));
    let running = running_bundle();
    let running_id = running.as_deref().and_then(bundle_identifier);
    let is_ours = is_unzipped && running_id.is_some() && bundle_identifier(&new_copy) == running_id && running.is_some_and(|running| is_same_signer(&running, &new_copy));
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

/// Returns an app's designated requirement, the rule that names its bundle id and signing certificate.
fn designated_requirement(app: &Path) -> Option<String> {
    let output = Command::new(CODESIGN).args(["-d", "-r-"]).arg(app).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let requirement = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && !requirement.is_empty()).then_some(requirement)
}

/**
 * Returns true when the new app's code signature is intact and it carries the running app's exact
 * designated requirement, so only an app signed by the same certificate can replace this one.
 */
fn is_same_signer(running: &Path, new_copy: &Path) -> bool {
    let is_intact = Command::new(CODESIGN)
        .args(["--verify", "--deep", "--strict"])
        .arg(new_copy)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    is_intact && designated_requirement(running).is_some_and(|wanted| designated_requirement(new_copy) == Some(wanted))
}

/// Asks "NetSpeedGauge 1.4.0 is ready. Install now?" with Install and Later, and returns true for Install.
pub fn ask_to_install(tag: &str) -> bool {
    let mtm = main_thread();
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&install_question(PROGRAM_NAME, tag)));
    alert.setInformativeText(&NSString::from_str("NetSpeedGauge restarts with the new version. Later asks again at the next check."));
    alert.addButtonWithTitle(&NSString::from_str("Install"));
    alert.addButtonWithTitle(&NSString::from_str("Later"));
    // A menu bar app is not in front, so it comes forward for the question.
    // SAFETY: the selector exists on every macOS version.
    let _: () = unsafe { objc2::msg_send![&*NSApplication::sharedApplication(mtm), activateIgnoringOtherApps: true] };
    alert.runModal() == NSAlertFirstButtonReturn
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
    let Some(folder) = work_folder() else { return false };
    let name = installed.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let set_aside = installed.with_file_name(format!("{name}.old"));
    let _ = std::fs::remove_dir_all(&set_aside);
    // Its own process group, so the script keeps running after this copy quits. "sh" is the script's $0.
    Command::new(SHELL)
        .arg("-c")
        .arg(INSTALL_SCRIPT)
        .arg("sh")
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

/// Downloads a release file, but only from the owner's GitHub pages or GitHub's file stores.
fn download(url: &str, agent: &str, accept: &str) -> Option<Vec<u8>> {
    is_allowed_url(url).then(|| fetch(url, agent, accept)).flatten()
}

/**
 * GETs an https address with the Mac's curl and returns the body of a successful answer. curl
 * follows GitHub's redirect to its file store, over https only, and the answer counts only when it
 * came from the address asked for or an allowed GitHub host. -q comes first so no ~/.curlrc applies.
 */
fn fetch(url: &str, agent: &str, accept: &str) -> Option<Vec<u8>> {
    let header = format!("Accept: {accept}");
    let output = Command::new(CURL)
        .args(["-q", "-fsSL", "--proto", "=https", "--proto-redir", "=https", "--max-time", TIMEOUT_SECONDS, "--max-filesize", MAX_DOWNLOAD_BYTES])
        .args(["-A", agent, "-H", header.as_str(), "-w", "%{stderr}%{url_effective}", "--", url])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let final_url = String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or_default().trim().to_string();
    (output.status.success() && (final_url == url || is_allowed_url(&final_url))).then_some(output.stdout)
}
