//! Update check and self-update from the public GitHub releases.
//!
//! Nothing is downloaded or replaced unless the user clicks Update. The check
//! asks GitHub for the latest release and compares its version with this one
//! (SemVer precedence). The download URL comes from that reply, so it is not
//! trusted: it must sit under [`DOWNLOAD_PREFIX`], be the program's own exe
//! name, have a plausible size and a published SHA-256. A release that fails
//! any of that is only offered as a link to the releases page.
//!
//! Install: download to a temporary file next to the exe while hashing it,
//! check size, SHA-256 and the MZ header, then rename the running exe to
//! `<exe>.old` (Windows allows renaming a running exe) and the new file into
//! its place, rolling back if the second rename fails. The `.old` copy is
//! deleted on the next start.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

const REPO: &str = "ghostrdr-ctrl/TLS-450-Plus-Snapshot-Audit-Log-Tool";
/// The releases/latest API of the public repo.
pub const LATEST_API: &str = "https://api.github.com/repos/ghostrdr-ctrl/TLS-450-Plus-Snapshot-Audit-Log-Tool/releases/latest";
/// An asset URL must sit under this prefix (this repo's release downloads).
pub const DOWNLOAD_PREFIX: &str = "https://github.com/ghostrdr-ctrl/TLS-450-Plus-Snapshot-Audit-Log-Tool/releases/download/";
/// Where the user is sent to read the notes or download by hand. A constant,
/// never a URL from the reply.
pub const RELEASES_PAGE: &str = "https://github.com/ghostrdr-ctrl/TLS-450-Plus-Snapshot-Audit-Log-Tool/releases/latest";
/// The asset to install: the same name the build produces.
pub const ASSET_NAME: &str = "TLS-450-Plus-Snapshot-Audit-Log-Tool.exe";

const MIN_ASSET_BYTES: u64 = 1024 * 1024;
const MAX_ASSET_BYTES: u64 = 100 * 1024 * 1024;
/// Cap on the API reply, so a hostile server cannot fill memory.
const MAX_REPLY_BYTES: u64 = 4 * 1024 * 1024;

/// A release newer than the running version.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// e.g. "0.3.0", without the leading v.
    pub version: String,
    /// The release notes (Markdown), for the "What's new" hover.
    pub notes: String,
    /// The vetted exe to install, `None` when it can only be offered as a page.
    pub asset: Option<Asset>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub url: String,
    pub size: u64,
    pub sha256: [u8; 32],
}

#[derive(Debug, PartialEq)]
pub enum Check {
    /// Nothing newer. Holds the latest released version.
    UpToDate(String),
    Available(Release),
}

fn agent(current_version: &str) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(20))
        .https_only(true)
        .user_agent(&format!("TLS450-Snapshot-Audit-Log-Tool/{current_version}"))
        .build()
}

fn net_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(404, _) => "no release has been published yet".to_string(),
        ureq::Error::Status(code, _) => format!("GitHub answered {code}"),
        ureq::Error::Transport(t) => format!("could not reach GitHub ({})", t.kind()),
    }
}

/// Ask GitHub for the latest release. Blocking; run it on a thread.
pub fn check(current_version: &str) -> Result<Check, String> {
    let resp = agent(current_version)
        .get(LATEST_API)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(net_error)?;
    let mut body = String::new();
    resp.into_reader()
        .take(MAX_REPLY_BYTES)
        .read_to_string(&mut body)
        .map_err(|e| format!("could not read GitHub's reply ({e})"))?;
    parse_release(&body, current_version)
}

/// Turn the releases/latest reply into a result. Pure.
pub fn parse_release(body: &str, current_version: &str) -> Result<Check, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|_| "GitHub's reply was not readable".to_string())?;
    let tag = v["tag_name"].as_str().ok_or("GitHub did not report a version")?;
    let latest = tag.trim().trim_start_matches(['v', 'V']).to_string();
    let (Some(cur), Some(new)) = (SemVer::parse(current_version), SemVer::parse(&latest)) else {
        return Err(format!("could not compare versions ({current_version} and {latest})"));
    };
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) || new <= cur {
        return Ok(Check::UpToDate(latest));
    }
    let notes = v["body"].as_str().unwrap_or("").to_string();
    let asset = v["assets"].as_array().and_then(|list| {
        list.iter().find_map(|a| {
            if !a["name"].as_str()?.eq_ignore_ascii_case(ASSET_NAME) {
                return None;
            }
            let url = a["browser_download_url"].as_str()?;
            if !is_vetted_url(url) {
                return None;
            }
            let size = a["size"].as_u64()?;
            if !(MIN_ASSET_BYTES..=MAX_ASSET_BYTES).contains(&size) {
                return None;
            }
            // GitHub's own digest of the uploaded file, else the one in the notes.
            let sha256 = a["digest"]
                .as_str()
                .and_then(|d| d.strip_prefix("sha256:"))
                .and_then(parse_hex32)
                .or_else(|| sha_from_notes(&notes))?;
            Some(Asset {
                url: url.to_string(),
                size,
                sha256,
            })
        })
    });
    Ok(Check::Available(Release {
        version: latest,
        notes,
        asset,
    }))
}

/// The first 64-hex-digit run after "SHA-256" in the notes (the release
/// workflow writes "SHA-256 of the exe: `<hex>`").
fn sha_from_notes(notes: &str) -> Option<[u8; 32]> {
    let at = notes.find("SHA-256")?;
    let rest = &notes[at..];
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_hexdigit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i - start == 64 {
                return parse_hex32(&rest[start..i]);
            }
        } else {
            i += 1;
        }
    }
    None
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// The URL as it will be fetched must sit under [`DOWNLOAD_PREFIX`]: both the
/// raw text and the parsed form (a parser drops "/../" segments), with no
/// percent signs, backslashes, control, space or non-ASCII characters.
pub fn is_vetted_url(url: &str) -> bool {
    if url.contains('%') || url.contains('\\') || url.chars().any(|c| c <= ' ' || c == '\u{7f}' || !c.is_ascii()) {
        return false;
    }
    if !url.starts_with(DOWNLOAD_PREFIX) {
        return false;
    }
    match url::Url::parse(url) {
        Ok(u) => u.scheme() == "https" && u.as_str().starts_with(DOWNLOAD_PREFIX) && u.path().starts_with(&format!("/{REPO}/")),
        Err(_) => false,
    }
}

/// A SemVer 2.0.0 version, compared by its precedence rules (build metadata ignored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVer {
    core: [u64; 3],
    pre: Vec<PreId>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PreId {
    // Declared first: numeric identifiers sort before alphanumeric ones.
    Num(u64),
    Alpha(String),
}

impl SemVer {
    pub fn parse(s: &str) -> Option<SemVer> {
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        let mut nums = [0u64; 3];
        for (n, p) in nums.iter_mut().zip(&parts) {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) || (p.len() > 1 && p.starts_with('0')) {
                return None;
            }
            *n = p.parse().ok()?;
        }
        let mut ids = vec![];
        if let Some(pre) = pre {
            for id in pre.split('.') {
                if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                    return None;
                }
                if id.bytes().all(|b| b.is_ascii_digit()) {
                    if id.len() > 1 && id.starts_with('0') {
                        return None;
                    }
                    ids.push(PreId::Num(id.parse().ok()?));
                } else {
                    ids.push(PreId::Alpha(id.to_string()));
                }
            }
        }
        Some(SemVer { core: nums, pre: ids })
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Equal,
                // A pre-release is lower than the release itself.
                (true, false) => Greater,
                (false, true) => Less,
                // Identifier by identifier; a shorter list that is a prefix is lower.
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

/// `<exe>.old`: where the previous version goes during the swap.
pub fn old_path(exe: &Path) -> PathBuf {
    let mut s: OsString = exe.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

/// Delete the copy an earlier update left behind. Best effort.
pub fn clean_up_old(exe: &Path) {
    let _ = std::fs::remove_file(old_path(exe));
}

/// Can files be created and removed in `dir`? Checked before downloading.
pub fn can_write_to(dir: &Path) -> bool {
    let probe = dir.join(format!(".write-test-{}", token()));
    match std::fs::File::create(&probe) {
        Ok(f) => {
            drop(f);
            std::fs::remove_file(&probe).is_ok()
        }
        Err(_) => false,
    }
}

fn token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{:x}-{nanos:x}", std::process::id())
}

/// Download `asset` into `dir` and verify it. Returns the temporary file,
/// which the caller swaps in. On any failure the temporary file is removed.
/// `progress(done, total)` is called as bytes arrive; `cancel` stops it.
pub fn download(
    asset: &Asset,
    current_version: &str,
    dir: &Path,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    if !is_vetted_url(&asset.url) {
        return Err("the download address is not this program's own release".into());
    }
    let resp = agent(current_version).get(&asset.url).call().map_err(net_error)?;
    let tmp = dir.join(format!(".{ASSET_NAME}.download-{}", token()));
    let result = save_verified(resp.into_reader(), asset, &tmp, progress, cancel);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|_| tmp)
}

/// Copy `reader` to `out` while hashing, then check size, SHA-256 and that it
/// is a Windows program. Separate from the network so it can be tested.
fn save_verified(reader: impl Read, asset: &Asset, out: &Path, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<(), String> {
    let mut file = std::fs::File::create(out).map_err(|e| format!("could not create the download file ({e})"))?;
    let mut reader = reader.take(asset.size + 1);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let mut head = [0u8; 2];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("canceled".into());
        }
        let n = reader.read(&mut buf).map_err(|e| format!("the download stopped ({e})"))?;
        if n == 0 {
            break;
        }
        if done < 2 {
            for (i, b) in buf[..n].iter().enumerate().take((2 - done) as usize) {
                head[done as usize + i] = *b;
            }
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| format!("could not write the download ({e})"))?;
        done += n as u64;
        progress(done, asset.size);
    }
    file.sync_all().map_err(|e| format!("could not write the download ({e})"))?;
    if done != asset.size {
        return Err(format!("the download is {done} bytes, expected {}", asset.size));
    }
    if hasher.finalize().as_slice() != asset.sha256 {
        return Err("the download does not match its published SHA-256".into());
    }
    if &head != b"MZ" {
        return Err("the download is not a Windows program".into());
    }
    Ok(())
}

/// Put `new_file` in place of `exe`, keeping the old one as `<exe>.old`. If
/// the second rename fails the first is undone, so the program is never lost.
pub fn swap_in(exe: &Path, new_file: &Path) -> Result<(), String> {
    let old = old_path(exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).map_err(|e| format!("could not move the running program aside ({e})"))?;
    if let Err(e) = std::fs::rename(new_file, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(format!("could not put the new version in place ({e})"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without eframe's "links" feature, opening a web page (release notes,
    /// download page) silently does nothing. Nothing else would catch that.
    #[test]
    fn eframe_can_open_web_pages() {
        let toml = include_str!("../Cargo.toml");
        let eframe = toml
            .lines()
            .find(|l| l.trim_start().starts_with("eframe ="))
            .expect("eframe in Cargo.toml");
        let default_off = eframe.contains("default-features = false");
        assert!(
            !default_off || eframe.contains("\"links\""),
            "eframe needs the \"links\" feature: {eframe}"
        );
    }

    fn v(s: &str) -> SemVer {
        SemVer::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    #[test]
    fn semver_precedence_follows_the_spec() {
        // The example chain from semver.org, section 11.
        let chain = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "2.0.0",
        ];
        for w in chain.windows(2) {
            assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        }
        assert_eq!(v("1.2.3+build.5"), v("1.2.3"));
        assert!(v("0.10.0") > v("0.9.9"));
    }

    #[test]
    fn semver_refuses_what_is_not_semver() {
        for s in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.x",
            "1.2.3-",
            "1.2.3-01",
            "1.2.3-a..b",
            "v1.2.3",
        ] {
            assert!(SemVer::parse(s).is_none(), "{s}");
        }
    }

    const HASH: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn reply(tag: &str, asset: &str) -> String {
        format!(
            r#"{{"tag_name":"{tag}","draft":false,"prerelease":false,"body":"- New thing\n\nSHA-256 of the exe: `{HASH}`","assets":[{asset}]}}"#
        )
    }

    fn good_asset() -> String {
        format!(r#"{{"name":"{ASSET_NAME}","size":9000000,"browser_download_url":"{DOWNLOAD_PREFIX}v0.3.0/{ASSET_NAME}"}}"#)
    }

    #[test]
    fn newer_release_with_a_good_asset_can_be_installed() {
        let Ok(Check::Available(r)) = parse_release(&reply("v0.3.0", &good_asset()), "0.2.1") else {
            panic!()
        };
        assert_eq!(r.version, "0.3.0");
        let a = r.asset.expect("asset");
        assert_eq!(a.size, 9_000_000);
        assert_eq!(a.sha256, parse_hex32(HASH).unwrap());
    }

    #[test]
    fn same_older_or_prerelease_is_up_to_date() {
        assert_eq!(
            parse_release(&reply("v0.2.1", &good_asset()), "0.2.1"),
            Ok(Check::UpToDate("0.2.1".into()))
        );
        assert_eq!(
            parse_release(&reply("v0.2.0", &good_asset()), "0.2.1"),
            Ok(Check::UpToDate("0.2.0".into()))
        );
        let pre = reply("v0.3.0", &good_asset()).replace(r#""prerelease":false"#, r#""prerelease":true"#);
        assert!(matches!(parse_release(&pre, "0.2.1"), Ok(Check::UpToDate(_))));
        // A beta of the running version is older than the release.
        assert!(matches!(
            parse_release(&reply("v0.3.0", &good_asset()), "0.3.0-beta.1"),
            Ok(Check::Available(_))
        ));
    }

    #[test]
    fn github_digest_is_preferred_over_the_notes() {
        let other = "ff".repeat(32);
        let asset = good_asset().replace("\"size\"", &format!("\"digest\":\"sha256:{other}\",\"size\""));
        let Ok(Check::Available(r)) = parse_release(&reply("v0.3.0", &asset), "0.2.1") else {
            panic!()
        };
        assert_eq!(r.asset.unwrap().sha256, [0xff; 32]);
    }

    #[test]
    fn unvetted_assets_are_only_offered_as_the_page() {
        let bad = [
            good_asset().replace(DOWNLOAD_PREFIX, "https://example.com/releases/download/"),
            good_asset().replace("v0.3.0/", "v0.3.0/../../../../other/repo/releases/download/x/"),
            good_asset().replace("https://", "http://"),
            good_asset().replace(&format!("\"name\":\"{ASSET_NAME}\""), "\"name\":\"other.exe\""),
            good_asset().replace("9000000", "1000"),
            good_asset().replace("9000000", "900000000"),
            "\"not an object\"".to_string(),
        ];
        for a in bad {
            let Ok(Check::Available(r)) = parse_release(&reply("v0.3.0", &a), "0.2.1") else {
                panic!("{a}")
            };
            assert!(r.asset.is_none(), "{a}");
        }
        // No hash anywhere: page only.
        let no_hash = reply("v0.3.0", &good_asset()).replace(HASH, "none");
        let Ok(Check::Available(r)) = parse_release(&no_hash, "0.2.1") else {
            panic!()
        };
        assert!(r.asset.is_none());
    }

    #[test]
    fn unreadable_replies_are_errors() {
        assert!(parse_release("not json", "0.2.1").is_err());
        assert!(parse_release("{}", "0.2.1").is_err());
        assert!(parse_release(r#"{"tag_name":"latest"}"#, "0.2.1").is_err());
    }

    #[test]
    fn url_vetting() {
        let ok = format!("{DOWNLOAD_PREFIX}v1.0.0/{ASSET_NAME}");
        assert!(is_vetted_url(&ok));
        for bad in [
            ok.replace("v1.0.0/", "v1.0.0/%2e%2e/"),
            ok.replace("v1.0.0/", "v1.0.0\\..\\"),
            ok.replace("v1.0.0/", "v1.0.0/ /"),
            format!("{DOWNLOAD_PREFIX}../../../../evil/repo/releases/download/x.exe"),
            ok.replace("github.com", "github.com.evil.net"),
            String::new(),
        ] {
            assert!(!is_vetted_url(&bad), "{bad}");
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tls450-update-test-{name}-{}", token()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn asset_for(bytes: &[u8]) -> Asset {
        Asset {
            url: format!("{DOWNLOAD_PREFIX}v1.0.0/{ASSET_NAME}"),
            size: bytes.len() as u64,
            sha256: Sha256::digest(bytes).into(),
        }
    }

    #[test]
    fn download_is_verified() {
        let d = scratch("dl");
        let out = d.join("new.exe");
        let body = [b"MZ".as_slice(), &[7u8; 200_000]].concat();
        let a = asset_for(&body);
        let no = AtomicBool::new(false);
        let seen = std::cell::Cell::new(0);
        save_verified(&body[..], &a, &out, &|done, _| seen.set(done), &no).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), body);
        assert_eq!(seen.get(), body.len() as u64);

        // Wrong hash, short, long, not a program, canceled.
        let mut wrong = a.clone();
        wrong.sha256[0] ^= 1;
        assert!(
            save_verified(&body[..], &wrong, &out, &|_, _| {}, &no)
                .unwrap_err()
                .contains("SHA-256")
        );
        assert!(
            save_verified(&body[..100], &a, &out, &|_, _| {}, &no)
                .unwrap_err()
                .contains("expected")
        );
        let longer = [body.as_slice(), b"x"].concat();
        assert!(
            save_verified(&longer[..], &a, &out, &|_, _| {}, &no)
                .unwrap_err()
                .contains("expected")
        );
        let text = vec![b'a'; 1000];
        assert!(
            save_verified(&text[..], &asset_for(&text), &out, &|_, _| {}, &no)
                .unwrap_err()
                .contains("Windows")
        );
        let yes = AtomicBool::new(true);
        assert_eq!(save_verified(&body[..], &a, &out, &|_, _| {}, &yes).unwrap_err(), "canceled");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn swap_replaces_the_exe_and_keeps_the_old_one() {
        let d = scratch("swap");
        let exe = d.join("app.exe");
        let new = d.join("new.tmp");
        std::fs::write(&exe, "old").unwrap();
        std::fs::write(old_path(&exe), "stale").unwrap();
        std::fs::write(&new, "new").unwrap();
        swap_in(&exe, &new).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(old_path(&exe)).unwrap(), "old");
        assert!(!new.exists());
        clean_up_old(&exe);
        assert!(!old_path(&exe).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn swap_rolls_back_when_the_new_file_is_missing() {
        let d = scratch("rollback");
        let exe = d.join("app.exe");
        std::fs::write(&exe, "old").unwrap();
        assert!(swap_in(&exe, &d.join("missing.tmp")).is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        assert!(!old_path(&exe).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Talks to GitHub: `cargo test -- --ignored live_check --nocapture`.
    #[test]
    #[ignore]
    fn live_check() {
        let r = check("0.0.1");
        println!("{r:?}");
        if let Err(e) = &r {
            assert_eq!(e, "no release has been published yet");
        }
    }

    #[test]
    fn write_probe() {
        let d = scratch("probe");
        assert!(can_write_to(&d));
        assert!(!can_write_to(&d.join("does-not-exist")));
        let _ = std::fs::remove_dir_all(&d);
    }
}
