//! Explicit self-update. Nothing here runs during ordinary startup.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::config;

const SOURCE_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone)]
struct UpdateSource {
    name: &'static str,
    api: String,
    github: bool,
}

/// Release discovery is deliberately provider-based: GitHub defines versions;
/// configured mirrors only supply assets for that exact tag.
struct ReleaseProvider {
    sources: Vec<UpdateSource>,
}

impl ReleaseProvider {
    fn configured() -> Self {
        let mut sources = vec![UpdateSource {
            name: "GitHub",
            api: "https://api.github.com/repos/Axium-Labs/AX/releases/latest".into(),
            github: true,
        }];
        if let Some(repo) = option_env!("GITCODE_AX_REPOSITORY").filter(|value| valid_repo(value)) {
            sources.push(UpdateSource {
                name: "GitCode",
                api: format!("https://api.gitcode.com/api/v5/repos/{repo}/releases/latest"),
                github: false,
            });
        }
        Self { sources }
    }

    async fn latest(
        &self,
        client: &reqwest::Client,
        asset_name: &str,
    ) -> Result<Vec<(UpdateSource, Release)>> {
        let probes = self.sources.iter().map(|source| async move {
            let started = Instant::now();
            let result = tokio::time::timeout(SOURCE_TIMEOUT, async {
                let release: Release = client
                    .get(&source.api)
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                release.asset(asset_name)?;
                release.asset("SHA256SUMS")?;
                if !valid_tag(&release.tag_name) {
                    bail!("invalid release tag from {}", source.name);
                }
                Ok::<_, anyhow::Error>(release)
            })
            .await
            .unwrap_or_else(|_| Err(anyhow!("{} release probe timed out", source.name)));
            (source.clone(), result, started.elapsed())
        });
        let responses = futures_util::future::join_all(probes).await;
        let github_tag = responses.iter().find_map(|(source, result, _)| {
            source
                .github
                .then(|| result.as_ref().ok().map(|release| release.tag_name.clone()))
                .flatten()
        });
        let canonical = github_tag.or_else(|| {
            responses.iter().find_map(|(_, result, _)| {
                result.as_ref().ok().map(|release| release.tag_name.clone())
            })
        });
        let Some(canonical) = canonical else {
            let details = responses
                .into_iter()
                .map(|(source, result, _)| {
                    format!(
                        "{}: {}",
                        source.name,
                        result.err().map_or_else(
                            || "no valid release".to_owned(),
                            |error| format!("{error:#}")
                        )
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            bail!("all release providers failed: {details}");
        };
        let mut candidates: Vec<_> = responses
            .into_iter()
            .filter_map(|(source, result, elapsed)| {
                result
                    .ok()
                    .filter(|r| r.tag_name == canonical)
                    .map(|r| (source, r, elapsed))
            })
            .collect();
        candidates.sort_by_key(|(_, _, elapsed)| *elapsed);
        if candidates.is_empty() {
            bail!("no provider has canonical GitHub version {canonical}");
        }
        Ok(candidates
            .into_iter()
            .map(|(source, release, _)| (source, release))
            .collect())
    }
}

fn valid_tag(tag: &str) -> bool {
    let version = tag
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or_default();
    !version.is_empty()
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    matches!((parts.next(), parts.next(), parts.next()), (Some(owner), Some(name), None)
        if !owner.is_empty() && !name.is_empty() && owner.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')))
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

impl Release {
    fn asset(&self, name: &str) -> Result<&ReleaseAsset> {
        self.assets
            .iter()
            .find(|asset| asset.name == name)
            .ok_or_else(|| anyhow!("release {} has no {name} asset", self.tag_name))
    }
}

pub async fn run() -> Result<()> {
    let target = std::env::current_exe().context("cannot locate the running AX executable")?;
    let asset_name = release_asset_name()?;
    let client = reqwest::Client::builder()
        .user_agent(concat!("AX/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()?;

    println!("AX: checking for updates...");
    let candidates = ReleaseProvider::configured()
        .latest(&client, &asset_name)
        .await?;
    let release = &candidates[0].1;
    let binary_outdated = release.tag_name.trim_start_matches('v') != env!("CARGO_PKG_VERSION");

    // Always pull the latest archive so the bundled payload (skills plus the
    // example MCP config) is refreshed even when the binary itself is already
    // current. Archives from before those were packaged simply have nothing to
    // install.
    println!("AX: downloading {}...", release.tag_name);
    let archive = verified_archive(&client, &candidates, &asset_name).await?;

    let home = config::ax_home();
    match install_bundled_assets(&archive, &asset_name, &home) {
        Ok(report) => {
            if !report.skills.is_empty() {
                println!(
                    "AX: installed {} bundled skill package(s) into {}",
                    report.skills.len(),
                    home.join("skills").display()
                );
            }
            if report.mcp {
                println!(
                    "AX: wrote example MCP config to {} (all servers disabled)",
                    home.join("mcp.toml").display()
                );
            }
        }
        Err(error) => {
            eprintln!("AX: could not refresh bundled skills/MCP config: {error:#}");
        }
    }

    if binary_outdated {
        let staged = stage_path(&target)?;
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)
                .with_context(|| format!("cannot stage update beside {}", target.display()))?;
            extract_executable(&archive, &asset_name, &mut file)?;
            file.flush()?;
            file.sync_all()?;
            drop(file);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
            }
            replace_after_download(&staged, &target)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&staged);
        }
        result
    } else {
        println!(
            "AX is up to date ({}); bundled skills and MCP config were refreshed.",
            release.tag_name
        );
        Ok(())
    }
}

async fn verified_archive(
    client: &reqwest::Client,
    candidates: &[(UpdateSource, Release)],
    asset_name: &str,
) -> Result<Vec<u8>> {
    let mut failures = Vec::new();
    for (source, candidate) in candidates {
        let result = async {
            let archive =
                download(client, &candidate.asset(asset_name)?.browser_download_url).await?;
            let sums =
                download(client, &candidate.asset("SHA256SUMS")?.browser_download_url).await?;
            let expected = checksum_for(&sums, asset_name)?;
            let actual = format!("{:x}", Sha256::digest(&archive));
            if !actual.eq_ignore_ascii_case(expected) {
                bail!("checksum mismatch for {asset_name}");
            }
            Ok::<_, anyhow::Error>(archive)
        }
        .await;
        match result {
            Ok(archive) => {
                println!("AX: verified download from {}", source.name);
                return Ok(archive);
            }
            Err(error) => failures.push(format!("{}: {error:#}", source.name)),
        }
    }
    bail!(
        "all release downloads failed: {}. The existing AX executable was not changed",
        failures.join("; ")
    )
}

fn retryable(error: &reqwest::Error) -> bool {
    error.is_connect()
        || error.is_timeout()
        || error.is_body()
        || error
            .status()
            .is_some_and(|status| status.is_server_error() || status.as_u16() == 429)
}

async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    for attempt in 0..3 {
        let result = async {
            client
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await
        }
        .await;
        match result {
            Ok(bytes) => return Ok(bytes.to_vec()),
            Err(error) if attempt < 2 && retryable(&error) => {
                tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("could not download {url}; check your network or proxy settings")
                });
            }
        }
    }
    unreachable!("every final download attempt returns")
}

fn release_asset_name() -> Result<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => bail!("updates are not published for architecture {other}"),
    };
    let platform = match std::env::consts::OS {
        "windows" => "pc-windows-msvc.zip",
        "linux" => "unknown-linux-musl.tar.gz",
        "macos" => "apple-darwin.tar.gz",
        other => bail!("updates are not published for platform {other}"),
    };
    Ok(format!("ax-{arch}-{platform}"))
}

fn checksum_for<'a>(sums: &'a [u8], asset_name: &str) -> Result<&'a str> {
    let text = std::str::from_utf8(sums).context("SHA256SUMS is not UTF-8")?;
    let matches: Vec<&str> = text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            let name = fields.next()?.trim_start_matches('*');
            (name == asset_name && fields.next().is_none()).then_some(hash)
        })
        .collect();
    match matches.as_slice() {
        [hash] if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) => Ok(hash),
        [] => bail!("{asset_name} is missing from SHA256SUMS"),
        _ => bail!("SHA256SUMS has an invalid or duplicate entry for {asset_name}"),
    }
}

fn stage_path(target: &Path) -> Result<PathBuf> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("AX executable has no parent directory"))?;
    Ok(parent.join(format!(".ax-update-{}", uuid::Uuid::new_v4())))
}

fn extract_executable(archive: &[u8], asset_name: &str, staged: &mut File) -> Result<()> {
    if Path::new(asset_name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
    {
        let mut zip = zip::ZipArchive::new(io::Cursor::new(archive))?;
        let mut entry = zip
            .by_name("ax.exe")
            .context("release archive has no ax.exe")?;
        if !entry.is_file() {
            bail!("release archive's ax.exe is not a regular file");
        }
        io::copy(&mut entry, staged)?;
    } else {
        let decoder = flate2::read::GzDecoder::new(archive);
        let mut tar = tar::Archive::new(decoder);
        let mut found = false;
        for entry in tar.entries()? {
            let mut entry = entry?;
            if entry.path()?.as_ref() != Path::new("ax") {
                continue;
            }
            if found || !entry.header().entry_type().is_file() {
                bail!("release archive has an invalid ax entry");
            }
            io::copy(&mut entry, staged)?;
            found = true;
        }
        if !found {
            bail!("release archive has no ax executable");
        }
    }
    if staged.metadata()?.len() == 0 {
        bail!("release archive contains an empty AX executable");
    }
    Ok(())
}

/// What `install_bundled_assets` actually wrote, so the update can report it.
#[derive(Debug, Default)]
struct BundleReport {
    /// Names of the skill packages that were newly installed.
    skills: HashSet<String>,
    /// Whether the example MCP config was written.
    mcp: bool,
}

/// Where an archive entry lands inside `AX_HOME`, if it is part of the bundled
/// payload. Everything else — the executable above all — is ignored, and so is
/// anything that tries to escape `AX_HOME` with `..`.
#[must_use]
fn bundled_destination(home: &Path, name: &str) -> Option<PathBuf> {
    let normalized = name.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let (first, rest) = parts.split_first()?;
    if rest.iter().any(|part| *part == "." || *part == "..") {
        return None;
    }
    match *first {
        "skills" if !rest.is_empty() => {
            let mut destination = home.join("skills");
            for part in rest {
                destination.push(part);
            }
            Some(destination)
        }
        "mcp.example.toml" if rest.is_empty() => Some(home.join("mcp.toml")),
        _ => None,
    }
}

fn record_installed(report: &mut BundleReport, name: &str) {
    let normalized = name.replace('\\', "/");
    let mut parts = normalized.split('/').filter(|part| !part.is_empty());
    if parts.next() == Some("skills") {
        if let Some(package) = parts.next() {
            report.skills.insert(package.to_string());
        }
    } else {
        report.mcp = true;
    }
}

/// Writes one bundled entry, skipping anything already present so local edits
/// to a skill package survive an update. Returns whether a file was created.
fn write_bundled(destination: &Path, reader: &mut impl io::Read) -> Result<bool> {
    if destination.exists() {
        return Ok(false);
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let mut file = File::create(destination)
        .with_context(|| format!("could not create {}", destination.display()))?;
    io::copy(reader, &mut file)?;
    file.sync_all()?;
    Ok(true)
}

/// Unpacks `skills/` and `mcp.example.toml` from a verified release archive
/// into `AX_HOME`, the same layout the install scripts produce. Archives that
/// predate the bundled payload yield an empty report rather than an error.
fn install_bundled_assets(archive: &[u8], asset_name: &str, home: &Path) -> Result<BundleReport> {
    let mut report = BundleReport::default();
    if Path::new(asset_name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
    {
        let mut zip = zip::ZipArchive::new(io::Cursor::new(archive))?;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index)?;
            if !entry.is_file() {
                continue;
            }
            let name = entry.name().to_string();
            let Some(destination) = bundled_destination(home, &name) else {
                continue;
            };
            if write_bundled(&destination, &mut entry)? {
                record_installed(&mut report, &name);
            }
        }
    } else {
        let decoder = flate2::read::GzDecoder::new(archive);
        let mut tar = tar::Archive::new(decoder);
        for entry in tar.entries()? {
            let mut entry = entry?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let name = entry.path()?.to_string_lossy().into_owned();
            let Some(destination) = bundled_destination(home, &name) else {
                continue;
            };
            if write_bundled(&destination, &mut entry)? {
                record_installed(&mut report, &name);
            }
        }
    }
    Ok(report)
}

#[cfg(unix)]
fn replace_after_download(staged: &Path, target: &Path) -> Result<()> {
    fs::rename(staged, target).with_context(|| {
        format!(
            "could not replace {}; the existing AX executable is unchanged",
            target.display()
        )
    })?;
    println!(
        "AX: updated {}. Restart AX to use the new version.",
        target.display()
    );
    Ok(())
}

#[cfg(windows)]
fn replace_after_download(staged: &Path, target: &Path) -> Result<()> {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };

    // Windows keeps the running executable open. A detached helper waits for
    // this process to exit, then swaps the verified staged file into place.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const SCRIPT: &str = r#"param([string]$Target, [string]$Staged, [int]$ParentPid, [string]$Log)
$ErrorActionPreference = 'Stop'
try {
    Wait-Process -Id $ParentPid -ErrorAction SilentlyContinue
    $backup = "$Target.update-backup"
    if (Test-Path -LiteralPath $backup) { throw "An earlier update backup exists: $backup" }
    Move-Item -LiteralPath $Target -Destination $backup
    try {
        Move-Item -LiteralPath $Staged -Destination $Target
    } catch {
        Move-Item -LiteralPath $backup -Destination $Target
        throw
    }
    Remove-Item -LiteralPath $backup -Force
} catch {
    $_ | Out-String | Set-Content -LiteralPath $Log
} finally {
    Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
}
"#;

    let script = std::env::temp_dir().join(format!("ax-update-{}.ps1", uuid::Uuid::new_v4()));
    let log = script.with_extension("log");
    fs::write(&script, SCRIPT).context("could not prepare Windows update helper")?;
    let spawn = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
        ])
        .arg(&script)
        .arg("-Target")
        .arg(target)
        .arg("-Staged")
        .arg(staged)
        .arg("-ParentPid")
        .arg(std::process::id().to_string())
        .arg("-Log")
        .arg(&log)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    if let Err(error) = spawn {
        let _ = fs::remove_file(&script);
        return Err(error).context("could not start Windows update helper");
    }
    println!(
        "AX: update verified. It will replace {} after AX exits.",
        target.display()
    );
    println!(
        "AX: if replacement fails, details will be written to {}",
        log.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_http(responses: Vec<(u16, &'static str)>) -> (String, std::thread::JoinHandle<()>) {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            use std::io::Read;
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 4096];
                let size = stream.read(&mut request).unwrap();
                assert!(size > 0);
                write!(stream, "HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        (url, server)
    }

    #[tokio::test]
    async fn transient_download_failure_retries_but_missing_asset_does_not() {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let (url, server) = mock_http(vec![(503, ""), (200, "archive")]);
        assert_eq!(download(&client, &url).await.unwrap(), b"archive");
        server.join().unwrap();
        let (url, server) = mock_http(vec![(404, "")]);
        let error = download(&client, &url).await.unwrap_err();
        assert!(format!("{error:#}").contains("404"));
        assert!(error.to_string().contains(&url));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn checksum_failure_falls_back_to_the_next_provider() {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let expected = format!("{:x}  ax.zip\n", Sha256::digest(b"verified archive"));
        let bad_sums: &'static str = Box::leak(expected.clone().into_boxed_str());
        let (bad_url, bad_server) = mock_http(vec![(200, "tampered archive"), (200, bad_sums)]);
        let good_sums: &'static str = Box::leak(expected.into_boxed_str());
        let (good_url, good_server) = mock_http(vec![(200, "verified archive"), (200, good_sums)]);
        let candidate = |tag: &str, base: &str| Release {
            tag_name: tag.into(),
            assets: ["ax.zip", "SHA256SUMS"]
                .into_iter()
                .map(|name| ReleaseAsset {
                    name: name.into(),
                    browser_download_url: format!("{base}/{name}"),
                })
                .collect(),
        };
        let candidates = vec![
            (
                UpdateSource {
                    name: "GitHub",
                    api: String::new(),
                    github: true,
                },
                candidate("v0.3.0", &bad_url),
            ),
            (
                UpdateSource {
                    name: "GitCode",
                    api: String::new(),
                    github: false,
                },
                candidate("v0.3.0", &good_url),
            ),
        ];
        assert_eq!(
            verified_archive(&client, &candidates, "ax.zip")
                .await
                .unwrap(),
            b"verified archive"
        );
        bad_server.join().unwrap();
        good_server.join().unwrap();
    }

    #[test]
    fn checksum_entry_must_be_unique_and_valid() {
        let hash = "a".repeat(64);
        let sums = format!("{hash}  ax-x.zip\n");
        assert_eq!(checksum_for(sums.as_bytes(), "ax-x.zip").unwrap(), hash);
        assert!(checksum_for(sums.as_bytes(), "other.zip").is_err());
        assert!(checksum_for(format!("{sums}{sums}").as_bytes(), "ax-x.zip").is_err());
        assert!(checksum_for(b"bad  ax-x.zip", "ax-x.zip").is_err());
    }

    #[test]
    fn bundled_entries_map_into_ax_home() {
        let home = Path::new("/home/tester/.ax");
        assert_eq!(
            bundled_destination(home, "skills/demo/SKILL.md").as_deref(),
            Some(Path::new("/home/tester/.ax/skills/demo/SKILL.md"))
        );
        assert_eq!(
            bundled_destination(home, "skills\\demo\\SKILL.md").as_deref(),
            Some(Path::new("/home/tester/.ax/skills/demo/SKILL.md"))
        );
        assert_eq!(
            bundled_destination(home, "mcp.example.toml").as_deref(),
            Some(Path::new("/home/tester/.ax/mcp.toml"))
        );
        assert_eq!(bundled_destination(home, "ax").as_deref(), None);
        assert_eq!(bundled_destination(home, "skills").as_deref(), None);
        assert_eq!(
            bundled_destination(home, "skills/../../evil").as_deref(),
            None
        );
    }

    /// Builds an archive with the real release layout: the executable plus the
    /// bundled payload.
    fn sample_tar() -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "ax", b"ax\n".as_slice())
            .unwrap();
        let mut skill = tar::Header::new_gnu();
        skill.set_size(5);
        skill.set_mode(0o644);
        skill.set_cksum();
        builder
            .append_data(&mut skill, "skills/demo/SKILL.md", b"demo\n".as_slice())
            .unwrap();
        let mut config = tar::Header::new_gnu();
        config.set_size(4);
        config.set_mode(0o644);
        config.set_cksum();
        builder
            .append_data(&mut config, "mcp.example.toml", b"#ax\n".as_slice())
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn sample_zip() -> Vec<u8> {
        use io::Cursor;
        use zip::{CompressionMethod, ZipWriter, write::FileOptions};

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = FileOptions::default().compression_method(CompressionMethod::Deflated);
        writer.start_file("ax.exe", options).unwrap();
        io::Write::write_all(&mut writer, b"ax\n").unwrap();
        writer.start_file("skills/demo/SKILL.md", options).unwrap();
        io::Write::write_all(&mut writer, b"demo\n").unwrap();
        writer.start_file("mcp.example.toml", options).unwrap();
        io::Write::write_all(&mut writer, b"#ax\n").unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn installs_bundled_skills_and_mcp_template() {
        for (asset, archive) in [
            ("ax-x86_64-apple-darwin.tar.gz", sample_tar()),
            ("ax-x86_64-pc-windows-msvc.zip", sample_zip()),
        ] {
            let home =
                std::env::temp_dir().join(format!("ax-update-test-{}", uuid::Uuid::new_v4()));
            let report = install_bundled_assets(&archive, asset, &home).unwrap();
            assert_eq!(report.skills.len(), 1, "{asset}: {report:?}");
            assert!(report.skills.contains("demo"), "{asset}: {report:?}");
            assert!(report.mcp, "{asset}: {report:?}");
            assert_eq!(
                fs::read_to_string(home.join("skills/demo/SKILL.md")).unwrap(),
                "demo\n"
            );
            assert_eq!(fs::read_to_string(home.join("mcp.toml")).unwrap(), "#ax\n");

            // A second pass must not clobber what is already installed.
            fs::write(home.join("skills/demo/SKILL.md"), b"edited\n").unwrap();
            let report = install_bundled_assets(&archive, asset, &home).unwrap();
            assert!(
                report.skills.is_empty() && !report.mcp,
                "{asset}: {report:?}"
            );
            assert_eq!(
                fs::read_to_string(home.join("skills/demo/SKILL.md")).unwrap(),
                "edited\n"
            );

            fs::remove_dir_all(&home).unwrap();
        }
    }

    #[test]
    fn parses_update_flag_without_a_command() {
        use clap::Parser;
        let cli = crate::args::Cli::try_parse_from(["ax", "--update"]).unwrap();
        assert!(cli.update);
    }
}
