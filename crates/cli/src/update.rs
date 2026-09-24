//! Explicit self-update. Nothing here runs during ordinary startup.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const RELEASE_API: &str = "https://api.github.com/repos/Axium-Labs/AX/releases/latest";

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
        .timeout(Duration::from_secs(120))
        .build()?;

    println!("AX: checking for updates...");
    let release: Release = client
        .get(RELEASE_API)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("could not read the latest GitHub Release")?;
    if release.tag_name.trim_start_matches('v') == env!("CARGO_PKG_VERSION") {
        println!("AX is up to date ({}).", release.tag_name);
        return Ok(());
    }

    println!("AX: downloading {}...", release.tag_name);
    let archive = download(&client, &release.asset(&asset_name)?.browser_download_url).await?;
    let sums = download(&client, &release.asset("SHA256SUMS")?.browser_download_url).await?;
    let expected = checksum_for(&sums, &asset_name)?;
    let actual = format!("{:x}", Sha256::digest(&archive));
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("checksum mismatch for {asset_name}; the existing AX executable was not changed");
    }

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
}

async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    Ok(client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec())
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
    fn parses_update_flag_without_a_command() {
        use clap::Parser;
        let cli = super::super::Cli::try_parse_from(["ax", "--update"]).unwrap();
        assert!(cli.update);
    }
}
