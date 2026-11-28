//! Tests for the update check: versions, when a check is due, reading GitHub's answer and SHA256SUMS.txt.

use netspeedgauge_shared::update::{
    asset_url, expected_sha, is_check_due, is_newer, is_sha_match, latest_release_url, mac_asset_name, parse_version, release_tag, sha256_hex,
    status_text, user_agent, windows_asset_name, UpdateCheck, UpdateStatus, UPDATE_CHECK_LABELS,
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
    let expected = Some("https://api.github.com/repos/developedByAshu/juice-bar/releases/latest".to_string());
    assert_eq!(latest_release_url("https://github.com/developedByAshu/juice-bar"), expected);
    assert_eq!(latest_release_url("https://github.com/developedByAshu/juice-bar.git/"), expected);
    assert_eq!(latest_release_url("https://gitlab.com/developedByAshu/juice-bar"), None);
    assert_eq!(latest_release_url("https://github.com/developedByAshu"), None);
}

#[test]
fn the_user_agent_names_only_the_app() {
    assert_eq!(user_agent("Juice Bar"), "JuiceBar-updater");
    assert_eq!(user_agent("NetSpeedGauge"), "NetSpeedGauge-updater");
}

#[test]
fn release_file_names_match_release_yml() {
    assert_eq!(windows_asset_name("JuiceBar", false), "JuiceBar-x64.exe");
    assert_eq!(windows_asset_name("JuiceBar", true), "JuiceBar-arm64.exe");
    assert_eq!(mac_asset_name("NetSpeedGauge"), "NetSpeedGauge-mac-arm64.zip");
}

#[test]
fn githubs_answer_gives_the_tag_and_each_download_link() {
    let json = r#"{"url":"x","tag_name" : "v0.2.0","name":"JuiceBar v0.2.0","assets":[
        {"name":"JuiceBar-arm64.exe","browser_download_url":"https://github.com/o/r/releases/download/v0.2.0/JuiceBar-arm64.exe"},
        {"name":"JuiceBar-x64.exe","browser_download_url":"https://github.com/o/r/releases/download/v0.2.0/JuiceBar-x64.exe"},
        {"name":"SHA256SUMS.txt","browser_download_url": "https://github.com/o/r/releases/download/v0.2.0/SHA256SUMS.txt"}]}"#;
    assert_eq!(release_tag(json).as_deref(), Some("v0.2.0"));
    assert_eq!(asset_url(json, "JuiceBar-x64.exe").as_deref(), Some("https://github.com/o/r/releases/download/v0.2.0/JuiceBar-x64.exe"));
    assert_eq!(asset_url(json, "SHA256SUMS.txt").as_deref(), Some("https://github.com/o/r/releases/download/v0.2.0/SHA256SUMS.txt"));
    assert_eq!(asset_url(json, "x64.exe"), None, "the whole file name must match");
    assert_eq!(release_tag(r#"{"message":"Not Found"}"#), None);
}

#[test]
fn the_sums_file_gives_each_files_hash() {
    let sums = format!("\u{feff}{}  JuiceBar-arm64.exe\r\n{ABC_SHA}  JuiceBar-x64.exe\r\n{} *JuiceBar-mac-arm64.zip\n", "1".repeat(64), "A".repeat(64));
    assert_eq!(expected_sha(&sums, "JuiceBar-x64.exe").as_deref(), Some(ABC_SHA));
    assert_eq!(expected_sha(&sums, "JuiceBar-arm64.exe").as_deref(), Some("1".repeat(64).as_str()), "a byte order mark is skipped");
    assert_eq!(expected_sha(&sums, "JuiceBar-mac-arm64.zip").as_deref(), Some("a".repeat(64).as_str()), "binary marker and case");
    assert_eq!(expected_sha(&sums, "JuiceBar.exe"), None);
    assert_eq!(expected_sha("nothexatall  JuiceBar-x64.exe", "JuiceBar-x64.exe"), None);
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
    assert_eq!(status_text(&UpdateStatus::Ready("v0.2.0".into()), "0.1.1", None), "Updates to 0.2.0 when Settings closes");
    assert_eq!(status_text(&UpdateStatus::Failed, "0.1.1", None), "Could not reach GitHub");
}
