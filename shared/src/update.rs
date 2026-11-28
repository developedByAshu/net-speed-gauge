//! Looking for a newer release on GitHub: the parts every platform shares, so they are tested once.
//! The Mac and Windows apps only fetch, ask, swap the program and draw the row.

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

const DAY_SECONDS: u64 = 86_400;
const SHA256_HEX_LENGTH: usize = 64;
const GITHUB_PREFIX: &str = "https://github.com/";
const SIGNATURE_HEX_LENGTH: usize = 128;

/**
 * The Ed25519 public key that signs every release's SHA256SUMS.txt. Its private half never sits on
 * GitHub, so a release someone else uploads cannot pass, even with matching checksums.
 */
pub const RELEASE_PUBLIC_KEY: &str = "bbd08975bd6d26ecc33be00d62d3dd0f68f98d8856e7e8447896fa8bc96d02a4";

/// The only places the updater downloads from: the owner's GitHub pages and GitHub's file stores.
const ALLOWED_URL_PREFIXES: [&str; 4] = [
    "https://github.com/developedByAshu/",
    "https://objects.githubusercontent.com/",
    "https://release-assets.githubusercontent.com/",
    "https://raw.githubusercontent.com/developedByAshu/",
];

/// How often the app looks for a new release, as the "Check for updates" row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UpdateCheck {
    OnLaunch,
    /// The default: looks daily, and always asks before installing what it finds.
    #[default]
    Daily,
    Weekly,
    Monthly,
    Never,
}

/// The row's options, in the order of `UpdateCheck`.
pub const UPDATE_CHECK_LABELS: [&str; 5] = ["Launch", "Daily", "Weekly", "Monthly", "Never"];

impl UpdateCheck {
    /// Reads the number saved in settings.txt; anything unknown is Daily, the default.
    pub fn from_number(number: i64) -> Self {
        match number {
            0 => Self::OnLaunch,
            2 => Self::Weekly,
            3 => Self::Monthly,
            4 => Self::Never,
            _ => Self::Daily,
        }
    }

    pub fn number(self) -> i64 {
        self as i64
    }

    /// Returns the gap between checks, or None when only launch decides.
    fn gap_seconds(self) -> Option<u64> {
        match self {
            Self::Daily => Some(DAY_SECONDS),
            Self::Weekly => Some(7 * DAY_SECONDS),
            Self::Monthly => Some(30 * DAY_SECONDS),
            Self::OnLaunch | Self::Never => None,
        }
    }
}

/**
 * Returns true when the app should look for an update now. On launch checks once per start, the
 * others once their gap has passed since the last check, and Never not at all. A last check in the
 * future (the clock was set back) counts as due, so a wrong clock cannot stop updates for good.
 */
pub fn is_check_due(choice: UpdateCheck, last_check: u64, now: u64, is_launch: bool) -> bool {
    match choice {
        UpdateCheck::Never => false,
        UpdateCheck::OnLaunch => is_launch,
        _ => last_check > now || choice.gap_seconds().is_some_and(|gap| now - last_check >= gap),
    }
}

/// Reads "1.2.3" or "v1.2.3" as numbers. A pre-release or build suffix is dropped; missing parts are 0.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let text = text.trim();
    let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
    let core = text.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |part| part.parse().ok())?;
    let patch = parts.next().map_or(Some(0), |part| part.parse().ok())?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// Returns true when the release tag is a higher version than the running app. An unreadable tag is never newer.
pub fn is_newer(current: &str, tag: &str) -> bool {
    match (parse_version(current), parse_version(tag)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

/// Turns the crate's repository link into the GitHub API link for its latest release.
pub fn latest_release_url(repository: &str) -> Option<String> {
    let path = repository.trim().trim_end_matches('/').trim_end_matches(".git").strip_prefix(GITHUB_PREFIX)?;
    let (owner, repo) = path.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/')).then(|| format!("https://api.github.com/repos/{owner}/{repo}/releases/latest"))
}

/// Returns true when a download link is one the updater may fetch: https, on an allowed GitHub host.
pub fn is_allowed_url(url: &str) -> bool {
    !url.chars().any(|c| c.is_whitespace() || c.is_control()) && ALLOWED_URL_PREFIXES.iter().any(|prefix| url.starts_with(prefix))
}

/**
 * Returns the link of a release's signature file, signatures/<tag>.sig on the repository's main
 * branch. A tag with anything but letters, digits, dots and dashes gets None, so it cannot reach
 * another path.
 */
pub fn signature_url(repository: &str, tag: &str) -> Option<String> {
    let path = repository.trim().trim_end_matches('/').trim_end_matches(".git").strip_prefix(GITHUB_PREFIX)?;
    let (owner, repo) = path.split_once('/')?;
    let is_safe_tag = !tag.is_empty() && !tag.starts_with('.') && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    (is_safe_tag && !owner.is_empty() && !repo.is_empty() && !repo.contains('/')).then(|| format!("https://raw.githubusercontent.com/{owner}/{repo}/main/signatures/{tag}.sig"))
}

/// Reads exactly `N` bytes written as hex, or None for any other length or a non-hex character.
fn hex_bytes<const N: usize>(hex: &str) -> Option<[u8; N]> {
    let (pairs, rest) = hex.as_bytes().as_chunks::<2>();
    if pairs.len() != N || !rest.is_empty() {
        return None;
    }
    let nibble = |digit: u8| char::from(digit).to_digit(16).and_then(|value| u8::try_from(value).ok());
    let mut bytes = [0u8; N];
    for (byte, [high, low]) in bytes.iter_mut().zip(pairs) {
        *byte = nibble(*high)? << 4 | nibble(*low)?;
    }
    Some(bytes)
}

/**
 * Returns true when `signature_hex` (one line of 128 hex characters) is the Ed25519 signature of
 * the exact bytes of SHA256SUMS.txt under the key `public_key_hex`. Anything unreadable is false.
 */
pub fn is_signature_valid_with(public_key_hex: &str, sums: &[u8], signature_hex: &str) -> bool {
    let signature_hex = signature_hex.trim();
    if signature_hex.len() != SIGNATURE_HEX_LENGTH {
        return false;
    }
    let (Some(key), Some(signature)) = (hex_bytes::<32>(public_key_hex), hex_bytes::<64>(signature_hex)) else { return false };
    let Ok(key) = VerifyingKey::from_bytes(&key) else { return false };
    key.verify_strict(sums, &Signature::from_bytes(&signature)).is_ok()
}

/// Returns true when the signature file proves SHA256SUMS.txt came from the release key.
pub fn is_signature_valid(sums: &[u8], signature_hex: &str) -> bool {
    is_signature_valid_with(RELEASE_PUBLIC_KEY, sums, signature_hex)
}

/// Returns the User-Agent GitHub asks every client for: the app's name, nothing about the user.
pub fn user_agent(app_name: &str) -> String {
    format!("{}-updater", app_name.replace(' ', ""))
}

/// Returns every string value of a key in a JSON text, in order. Enough for GitHub's flat release fields.
fn json_strings<'a>(json: &'a str, key: &str) -> Vec<&'a str> {
    let needle = format!("\"{key}\"");
    let mut values = Vec::new();
    let mut rest = json;
    while let Some(start) = rest.find(&needle) {
        rest = &rest[start + needle.len()..];
        let Some(after_colon) = rest.trim_start().strip_prefix(':') else { continue };
        let Some(value) = after_colon.trim_start().strip_prefix('"') else { continue };
        if let Some(end) = value.find('"') {
            values.push(&value[..end]);
        }
    }
    values
}

/// Returns the release's tag, such as "v1.2.1".
pub fn release_tag(json: &str) -> Option<String> {
    json_strings(json, "tag_name").first().map(|tag| (*tag).to_string())
}

/// Returns the download link of the release file with this exact name.
pub fn asset_url(json: &str, asset_name: &str) -> Option<String> {
    let ending = format!("/{asset_name}");
    json_strings(json, "browser_download_url").into_iter().find(|url| url.ends_with(&ending)).map(str::to_string)
}

/// Finds a file's sha256 in SHA256SUMS.txt ("hash  name" lines), lower case.
pub fn expected_sha(sums: &str, asset_name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let line = line.trim_start_matches('\u{feff}').trim();
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        let is_hash = hash.len() == SHA256_HEX_LENGTH && hash.chars().all(|c| c.is_ascii_hexdigit());
        (is_hash && name == asset_name).then(|| hash.to_ascii_lowercase())
    })
}

/// Returns the sha256 of some bytes as lower case hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Returns true when the downloaded bytes are exactly the file the release lists.
pub fn is_sha_match(bytes: &[u8], expected: &str) -> bool {
    sha256_hex(bytes).eq_ignore_ascii_case(expected.trim())
}

/// Where the Check now row stands.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Downloading(String),
    /// The new copy is downloaded and its signature checked; the app asks before it goes in.
    Ready(String),
    Failed,
}

/// Returns a Windows release file's name, as release.yml writes it: "NetSpeedGauge-x64.exe" or "NetSpeedGauge-arm64.exe".
pub fn windows_asset_name(program_name: &str, is_arm: bool) -> String {
    format!("{program_name}-{}.exe", if is_arm { "arm64" } else { "x64" })
}

/// Returns the Mac release file's name, as release.yml writes it: "NetSpeedGauge-mac-arm64.zip".
pub fn mac_asset_name(program_name: &str) -> String {
    format!("{program_name}-mac-arm64.zip")
}

/// Returns the question asked before an update goes in, such as "NetSpeedGauge 1.4.0 is ready. Install now?".
pub fn install_question(app_name: &str, tag: &str) -> String {
    format!("{app_name} {} is ready. Install now?", tag.trim_start_matches(['v', 'V']))
}

/**
 * Returns the hint under Check now. `last_checked` is the last check's local time, already
 * formatted by the app (such as "28 Nov 15:20"), or None before the first check.
 */
pub fn status_text(status: &UpdateStatus, current_version: &str, last_checked: Option<&str>) -> String {
    match status {
        UpdateStatus::Idle => last_checked.map_or_else(|| "Not checked yet".to_string(), |time| format!("Last checked {time}")),
        UpdateStatus::Checking => "Checking...".to_string(),
        UpdateStatus::UpToDate => format!("Up to date ({current_version})"),
        UpdateStatus::Downloading(version) => format!("Downloading {}...", version.trim_start_matches(['v', 'V'])),
        UpdateStatus::Ready(version) => format!("Asks to install {} when Settings closes", version.trim_start_matches(['v', 'V'])),
        UpdateStatus::Failed => "Could not reach GitHub".to_string(),
    }
}
