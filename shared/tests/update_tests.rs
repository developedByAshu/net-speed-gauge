//! Tests for the update check: versions, when a check is due, reading GitHub's answer and SHA256SUMS.txt,
//! the release signature and the hosts a download may come from.

use ed25519_dalek::{Signer, SigningKey};
use netspeedgauge_shared::update::{
    asset_url, expected_sha, install_question, is_allowed_url, is_check_due, is_newer, is_sha_match, is_signature_valid, is_signature_valid_with,
    latest_release_url, mac_asset_name, parse_version, release_tag, sha256_hex, signature_url, status_text, user_agent, windows_asset_name,
    UpdateCheck, UpdateStatus, RELEASE_PUBLIC_KEY, UPDATE_CHECK_LABELS,
};

const DAY: u64 = 86_400;
const NOW: u64 = 1_795_000_000;
// sha256 of the three bytes "abc", from FIPS 180-2.
const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn versions_read_with_or_without_a_leading_v() {
    assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
    assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
    assert_eq!(parse_version(" V0.1 "), Some((0, 1, 0)), "missing parts are 0");
    assert_eq!(parse_version("2.0.0-beta.1"), Some((2, 0, 0)), "a pre-release suffix is dropped");
    assert_eq!(parse_version("1.2.3.4"), None);
    assert_eq!(parse_version("latest"), None);
    assert_eq!(parse_version(""), None);
}

#[test]
fn only_a_higher_release_counts_as_newer() {
    assert!(is_newer("0.1.1", "v0.2.0"));
    assert!(is_newer("0.1.1", "0.1.2"));
    assert!(is_newer("1.9.9", "v2.0.0"));
    assert!(is_newer("1.2.10", "1.10.0"), "numbers compare as numbers, not text");
    assert!(!is_newer("0.1.1", "v0.1.1"), "the same version is not newer");
    assert!(!is_newer("1.3.0", "v1.2.9"), "an older release is not newer");
    assert!(!is_newer("1.3.0", "nightly"), "an unreadable tag is never newer");
}

#[test]
fn each_interval_is_due_only_after_its_gap() {
    assert!(is_check_due(UpdateCheck::Daily, 0, NOW, false), "never checked is due");
    assert!(!is_check_due(UpdateCheck::Daily, NOW - DAY + 60, NOW, false));
    assert!(is_check_due(UpdateCheck::Daily, NOW - DAY, NOW, false));
    assert!(!is_check_due(UpdateCheck::Weekly, NOW - 6 * DAY, NOW, false));
    assert!(is_check_due(UpdateCheck::Weekly, NOW - 7 * DAY, NOW, false));
    assert!(!is_check_due(UpdateCheck::Monthly, NOW - 29 * DAY, NOW, false));
    assert!(is_check_due(UpdateCheck::Monthly, NOW - 30 * DAY, NOW, false));
    assert!(is_check_due(UpdateCheck::Daily, NOW + DAY, NOW, false), "a clock set back still checks");
}

#[test]
fn on_launch_checks_once_per_start_and_never_never_checks() {
    assert!(is_check_due(UpdateCheck::OnLaunch, NOW, NOW, true));
    assert!(!is_check_due(UpdateCheck::OnLaunch, 0, NOW, false), "only the launch check runs");
    assert!(!is_check_due(UpdateCheck::Never, 0, NOW, true));
    assert!(!is_check_due(UpdateCheck::Never, 0, NOW, false));
}

#[test]
fn the_choice_survives_its_number_and_defaults_to_daily() {
    for choice in [UpdateCheck::OnLaunch, UpdateCheck::Daily, UpdateCheck::Weekly, UpdateCheck::Monthly, UpdateCheck::Never] {
        assert_eq!(UpdateCheck::from_number(choice.number()), choice);
    }
    assert_eq!(UpdateCheck::from_number(9), UpdateCheck::Daily);
    assert_eq!(UpdateCheck::default(), UpdateCheck::Daily);
    assert_eq!(UPDATE_CHECK_LABELS, ["Launch", "Daily", "Weekly", "Monthly", "Never"]);
}

#[test]
fn the_api_link_comes_from_the_repository_link() {
    let expected = Some("https://api.github.com/repos/developedByAshu/net-speed-gauge/releases/latest".to_string());
    assert_eq!(latest_release_url("https://github.com/developedByAshu/net-speed-gauge"), expected);
    assert_eq!(latest_release_url("https://github.com/developedByAshu/net-speed-gauge.git/"), expected);
    assert_eq!(latest_release_url("https://gitlab.com/developedByAshu/net-speed-gauge"), None);
    assert_eq!(latest_release_url("https://github.com/developedByAshu"), None);
}

#[test]
fn the_user_agent_names_only_the_app() {
    assert_eq!(user_agent("Net Speed Gauge"), "NetSpeedGauge-updater");
    assert_eq!(user_agent("NetSpeedGauge"), "NetSpeedGauge-updater");
}

#[test]
fn release_file_names_match_release_yml() {
    assert_eq!(windows_asset_name("NetSpeedGauge", false), "NetSpeedGauge-x64.exe");
    assert_eq!(windows_asset_name("NetSpeedGauge", true), "NetSpeedGauge-arm64.exe");
    assert_eq!(mac_asset_name("NetSpeedGauge"), "NetSpeedGauge-mac-arm64.zip");
}

#[test]
fn githubs_answer_gives_the_tag_and_each_download_link() {
    let json = r#"{"url":"x","tag_name" : "v0.2.0","name":"NetSpeedGauge v0.2.0","assets":[
        {"name":"NetSpeedGauge-arm64.exe","browser_download_url":"https://github.com/o/r/releases/download/v0.2.0/NetSpeedGauge-arm64.exe"},
        {"name":"NetSpeedGauge-x64.exe","browser_download_url":"https://github.com/o/r/releases/download/v0.2.0/NetSpeedGauge-x64.exe"},
        {"name":"SHA256SUMS.txt","browser_download_url": "https://github.com/o/r/releases/download/v0.2.0/SHA256SUMS.txt"}]}"#;
    assert_eq!(release_tag(json).as_deref(), Some("v0.2.0"));
    assert_eq!(asset_url(json, "NetSpeedGauge-x64.exe").as_deref(), Some("https://github.com/o/r/releases/download/v0.2.0/NetSpeedGauge-x64.exe"));
    assert_eq!(asset_url(json, "SHA256SUMS.txt").as_deref(), Some("https://github.com/o/r/releases/download/v0.2.0/SHA256SUMS.txt"));
    assert_eq!(asset_url(json, "x64.exe"), None, "the whole file name must match");
    assert_eq!(release_tag(r#"{"message":"Not Found"}"#), None);
}

#[test]
fn the_sums_file_gives_each_files_hash() {
    let sums = format!("\u{feff}{}  NetSpeedGauge-arm64.exe\r\n{ABC_SHA}  NetSpeedGauge-x64.exe\r\n{} *NetSpeedGauge-mac-arm64.zip\n", "1".repeat(64), "A".repeat(64));
    assert_eq!(expected_sha(&sums, "NetSpeedGauge-x64.exe").as_deref(), Some(ABC_SHA));
    assert_eq!(expected_sha(&sums, "NetSpeedGauge-arm64.exe").as_deref(), Some("1".repeat(64).as_str()), "a byte order mark is skipped");
    assert_eq!(expected_sha(&sums, "NetSpeedGauge-mac-arm64.zip").as_deref(), Some("a".repeat(64).as_str()), "binary marker and case");
    assert_eq!(expected_sha(&sums, "NetSpeedGauge.exe"), None);
    assert_eq!(expected_sha("nothexatall  NetSpeedGauge-x64.exe", "NetSpeedGauge-x64.exe"), None);
}

#[test]
fn only_the_exact_bytes_match_their_hash() {
    assert_eq!(sha256_hex(b"abc"), ABC_SHA);
    assert!(is_sha_match(b"abc", ABC_SHA));
    assert!(is_sha_match(b"abc", &ABC_SHA.to_ascii_uppercase()));
    assert!(!is_sha_match(b"abd", ABC_SHA));
    assert!(!is_sha_match(b"abc", ""));
}

#[test]
fn the_check_now_hint_says_where_it_stands() {
    assert_eq!(status_text(&UpdateStatus::Idle, "0.1.1", None), "Not checked yet");
    assert_eq!(status_text(&UpdateStatus::Idle, "0.1.1", Some("28 Nov 15:20")), "Last checked 28 Nov 15:20");
    assert_eq!(status_text(&UpdateStatus::Checking, "0.1.1", None), "Checking...");
    assert_eq!(status_text(&UpdateStatus::UpToDate, "0.1.1", None), "Up to date (0.1.1)");
    assert_eq!(status_text(&UpdateStatus::Downloading("v0.2.0".into()), "0.1.1", None), "Downloading 0.2.0...");
    assert_eq!(status_text(&UpdateStatus::Ready("v0.2.0".into()), "0.1.1", None), "Asks to install 0.2.0 when Settings closes");
    assert_eq!(status_text(&UpdateStatus::Failed, "0.1.1", None), "Could not reach GitHub");
}

const SUMS: &[u8] = b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  NetSpeedGauge-x64.exe\n";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Returns a test key's public half as hex and its signature of `message` as hex.
fn signed_by(seed: u8, message: &[u8]) -> (String, String) {
    let key = SigningKey::from_bytes(&[seed; 32]);
    (hex(key.verifying_key().as_bytes()), hex(&key.sign(message).to_bytes()))
}

#[test]
fn a_good_signature_over_the_exact_sums_passes() {
    let (public_key, signature) = signed_by(7, SUMS);
    assert!(is_signature_valid_with(&public_key, SUMS, &signature));
    assert!(is_signature_valid_with(&public_key, SUMS, &format!("{signature}\n")), "the file's line ending is ignored");
}

#[test]
fn a_bad_signature_fails() {
    let (public_key, signature) = signed_by(7, SUMS);
    let flipped = format!("{}{}", if signature.starts_with('0') { '1' } else { '0' }, &signature[1..]);
    assert!(!is_signature_valid_with(&public_key, SUMS, &flipped));
    assert!(!is_signature_valid_with(&public_key, SUMS, &"0".repeat(128)));
    assert!(!is_signature_valid_with(&public_key, SUMS, ""));
}

#[test]
fn a_signature_from_another_key_fails() {
    let (_, signature) = signed_by(7, SUMS);
    let (other_key, _) = signed_by(9, SUMS);
    assert!(!is_signature_valid_with(&other_key, SUMS, &signature));
    assert!(!is_signature_valid(SUMS, &signature), "the release key did not sign this");
}

#[test]
fn a_truncated_or_non_hex_signature_fails() {
    let (public_key, signature) = signed_by(7, SUMS);
    assert!(!is_signature_valid_with(&public_key, SUMS, &signature[..126]));
    assert!(!is_signature_valid_with(&public_key, SUMS, &format!("{signature}00")));
    assert!(!is_signature_valid_with(&public_key, SUMS, &format!("{}zz", &signature[..126])));
    assert!(!is_signature_valid_with(&public_key[..62], SUMS, &signature), "a short key fails too");
}

#[test]
fn tampered_sums_fail() {
    let (public_key, signature) = signed_by(7, SUMS);
    let mut tampered = SUMS.to_vec();
    tampered[0] = b'c';
    assert!(!is_signature_valid_with(&public_key, &tampered, &signature));
    assert!(!is_signature_valid_with(&public_key, &[SUMS, b"\n"].concat(), &signature), "even one extra byte");
}

#[test]
fn the_release_key_is_32_bytes_of_hex() {
    assert_eq!(RELEASE_PUBLIC_KEY.len(), 64);
    assert!(RELEASE_PUBLIC_KEY.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn the_signature_link_is_on_main_and_only_for_a_plain_tag() {
    assert_eq!(
        signature_url("https://github.com/developedByAshu/net-speed-gauge", "v1.4.0").as_deref(),
        Some("https://raw.githubusercontent.com/developedByAshu/net-speed-gauge/main/signatures/v1.4.0.sig")
    );
    assert_eq!(signature_url("https://github.com/developedByAshu/net-speed-gauge", "../x"), None);
    assert_eq!(signature_url("https://github.com/developedByAshu/net-speed-gauge", "v1/x"), None);
    assert_eq!(signature_url("https://github.com/developedByAshu/net-speed-gauge", ""), None);
    assert_eq!(signature_url("https://gitlab.com/developedByAshu/net-speed-gauge", "v1.4.0"), None);
}

#[test]
fn downloads_come_only_from_github_over_https() {
    assert!(is_allowed_url("https://github.com/developedByAshu/net-speed-gauge/releases/download/v1.4.0/SHA256SUMS.txt"));
    assert!(is_allowed_url("https://objects.githubusercontent.com/github-production-release-asset/1"));
    assert!(is_allowed_url("https://release-assets.githubusercontent.com/github-production-release-asset/1"));
    assert!(is_allowed_url("https://raw.githubusercontent.com/developedByAshu/net-speed-gauge/main/signatures/v1.4.0.sig"));
    assert!(!is_allowed_url("http://github.com/developedByAshu/net-speed-gauge/releases/download/v1.4.0/SHA256SUMS.txt"), "plain http");
    assert!(!is_allowed_url("https://github.com/someoneelse/net-speed-gauge/releases/download/v1.4.0/x.exe"), "another owner");
    assert!(!is_allowed_url("https://raw.githubusercontent.com/someoneelse/x/main/signatures/v1.sig"));
    assert!(!is_allowed_url("https://github.com.evil.example/developedByAshu/x"));
    assert!(!is_allowed_url("https://objects.githubusercontent.com.evil.example/x"));
    assert!(!is_allowed_url("https://github.com/developedByAshu/x y"), "no spaces");
}

#[test]
fn the_install_question_names_the_app_and_version() {
    assert_eq!(install_question("NetSpeedGauge", "v1.4.0"), "NetSpeedGauge 1.4.0 is ready. Install now?");
}
