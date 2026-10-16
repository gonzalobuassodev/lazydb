//! Installation ownership and state primitives used by the update command.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use async_trait::async_trait;
use clap::ValueEnum;
use crossterm::style::Stylize;
use reqwest::Client;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use xz2::read::XzDecoder;

mod cli_progress;

const INSTALLATION_SCHEMA: u32 = 1;
const CHANNEL_MANIFEST_SCHEMA: u32 = 2;
const DEFAULT_CHANNEL_BASE_URL: &str = "https://lazydb.yelog.org/channels";
const GITHUB_HOST: &str = "github.com";
const PAGES_HOST: &str = "lazydb.yelog.org";
const SUPPORTED_TARGETS: [&str; 5] = [
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
];

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct UpdateReport {
    pub schema: u32,
    pub manager: InstallationManager,
    pub channel: UpdateChannel,
    pub current_version: Option<String>,
    pub target_version: Option<String>,
    pub status: String,
    pub action: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateStatus {
    UpToDate,
    Available,
    ReadyToRestart,
    ManagerActionRequired,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateStage {
    Preparing,
    Downloading,
    VerifyingChecksum,
    Extracting,
    VerifyingInstallation,
    Activating,
    Finishing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateProgress {
    pub stage: UpdateStage,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateInspection {
    pub manager: InstallationManager,
    pub channel: UpdateChannel,
    pub running_version: String,
    pub installed_version: Option<String>,
    pub target_version: Option<String>,
    pub status: UpdateStatus,
    pub action: Option<String>,
    pub launcher_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChannelManifest {
    pub schema: u32,
    pub product: String,
    pub channel: UpdateChannel,
    pub version: String,
    pub tag: String,
    pub prerelease: bool,
    pub published_at: String,
    pub release_url: String,
    pub assets: BTreeMap<String, ManifestAsset>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestAsset {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub requirements: Option<AssetRequirements>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetRequirements {
    pub libc: LibcRequirement,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LibcRequirement {
    pub family: String,
    pub min_version: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("invalid channel manifest JSON: {0}")]
    Json(String),
    #[error("invalid channel manifest: {0}")]
    Invalid(String),
}

pub fn parse_channel_manifest(input: &str) -> Result<ChannelManifest, ManifestError> {
    let manifest: ChannelManifest =
        serde_json::from_str(input).map_err(|error| ManifestError::Json(error.to_string()))?;
    validate_channel_manifest(manifest)
}

fn validate_channel_manifest(manifest: ChannelManifest) -> Result<ChannelManifest, ManifestError> {
    if !matches!(manifest.schema, 1 | CHANNEL_MANIFEST_SCHEMA) || manifest.product != "lazydb" {
        return Err(ManifestError::Invalid(
            "unsupported schema or product".into(),
        ));
    }
    let version = Version::parse(&manifest.version)
        .map_err(|_| ManifestError::Invalid("version is not valid SemVer".into()))?;
    let is_beta = !version.pre.is_empty();
    let beta_version = version
        .pre
        .to_string()
        .split_once('.')
        .is_some_and(|(name, number)| {
            name == "beta"
                && number.parse::<u64>().is_ok_and(|number| number > 0)
                && !number.starts_with('0')
                && number.chars().all(|character| character.is_ascii_digit())
        });
    if is_beta != (manifest.channel == UpdateChannel::Beta)
        || manifest.prerelease != is_beta
        || manifest.tag != format!("v{}", manifest.version)
        || (manifest.channel == UpdateChannel::Beta && !beta_version)
    {
        return Err(ManifestError::Invalid(
            "channel, version, tag, or prerelease mismatch".into(),
        ));
    }
    if manifest.published_at.trim().is_empty()
        || !approved_url(&manifest.release_url, GITHUB_HOST)
        || manifest.release_url
            != format!(
                "https://github.com/yelog/lazydb/releases/tag/{}",
                manifest.tag
            )
    {
        return Err(ManifestError::Invalid(
            "invalid release URL or publication timestamp".into(),
        ));
    }
    if manifest.assets.len() != SUPPORTED_TARGETS.len()
        || !SUPPORTED_TARGETS
            .iter()
            .all(|target| manifest.assets.contains_key(*target))
    {
        return Err(ManifestError::Invalid(
            "target asset set is incomplete or contains extras".into(),
        ));
    }
    for target in SUPPORTED_TARGETS {
        let asset = &manifest.assets[target];
        let extension = if target == "x86_64-pc-windows-msvc" {
            "zip"
        } else {
            "tar.xz"
        };
        let expected = format!("lazydb_{}_{}.{}", manifest.version, target, extension);
        if asset.url
            != format!(
                "https://github.com/yelog/lazydb/releases/download/{}/{}",
                manifest.tag, expected
            )
            || asset.sha256.len() != 64
            || !asset
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(ManifestError::Invalid(format!(
                "invalid asset for {target}"
            )));
        }
        if target.ends_with("-linux-gnu") {
            match (manifest.schema, &asset.requirements) {
                (1, None) => {}
                (2, Some(requirements))
                    if requirements.libc.family == "glibc"
                        && valid_glibc_version(&requirements.libc.min_version) => {}
                _ => {
                    return Err(ManifestError::Invalid(format!(
                        "invalid Linux requirements for {target}"
                    )));
                }
            }
        } else if asset.requirements.is_some() {
            return Err(ManifestError::Invalid(format!(
                "unexpected requirements for {target}"
            )));
        }
    }
    Ok(manifest)
}

fn valid_glibc_version(value: &str) -> bool {
    let mut parts = value.split('.');
    matches!((parts.next(), parts.next(), parts.next()), (Some(major), Some(minor), None)
        if !major.is_empty() && !minor.is_empty()
            && major.bytes().all(|byte| byte.is_ascii_digit())
            && minor.bytes().all(|byte| byte.is_ascii_digit()))
}

fn approved_url(value: &str, host: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| url.scheme() == "https" && url.host_str() == Some(host))
}

#[async_trait]
pub trait UpdateHttpClient {
    async fn get(&self, url: &str) -> anyhow::Result<String>;

    async fn get_manifest(&self, url: &str) -> anyhow::Result<(u16, String)> {
        self.get(url).await.map(|body| (200, body))
    }

    async fn download(&self, url: &str) -> anyhow::Result<Vec<u8>>;

    async fn download_with_progress(
        &self,
        url: &str,
        progress: &(dyn Fn(UpdateProgress) + Send + Sync),
    ) -> anyhow::Result<Vec<u8>> {
        let archive = self.download(url).await?;
        progress(UpdateProgress {
            stage: UpdateStage::Downloading,
            downloaded_bytes: archive.len() as u64,
            total_bytes: Some(archive.len() as u64),
        });
        Ok(archive)
    }
}

pub struct SystemUpdateHttpClient {
    client: Client,
}

impl Default for SystemUpdateHttpClient {
    fn default() -> Self {
        Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() >= 10 {
                        return attempt.error("too many HTTP redirects");
                    }
                    let requested = attempt
                        .previous()
                        .first()
                        .expect("redirect attempts have a previous URL");
                    if validate_response_url(requested.as_str(), attempt.url()).is_ok() {
                        attempt.follow()
                    } else {
                        attempt.error("HTTP redirect left the approved host or scheme")
                    }
                }))
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .build()
                .expect("valid HTTP client configuration"),
        }
    }
}

fn validate_response_url(requested: &str, response: &url::Url) -> anyhow::Result<()> {
    let requested = url::Url::parse(requested)?;
    let response_host = response
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("redirected URL has no host"))?;
    let requested_host = requested
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("requested URL has no host"))?;
    let same_host = response_host == requested_host;
    let github_asset_redirect = requested_host == GITHUB_HOST
        && matches!(
            response_host,
            "release-assets.githubusercontent.com" | "objects.githubusercontent.com"
        );
    let local_fixture = matches!(requested_host, "127.0.0.1" | "localhost" | "fixture");
    let valid_scheme = response.scheme() == "https"
        || (local_fixture && requested.scheme() == "http" && response.scheme() == "http");
    if !valid_scheme || !(same_host || github_asset_redirect) {
        anyhow::bail!("HTTP redirect left the approved host or scheme")
    }
    Ok(())
}

#[async_trait]
impl UpdateHttpClient for SystemUpdateHttpClient {
    async fn get(&self, url: &str) -> anyhow::Result<String> {
        self.get_manifest(url).await.map(|(_, body)| body)
    }

    async fn get_manifest(&self, url: &str) -> anyhow::Result<(u16, String)> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("failed to request update manifest {url}"))?;
        validate_response_url(url, response.url())?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .context("failed to read update manifest response")?;
        Ok((status, body))
    }

    async fn download(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("failed to request update asset {url}"))?
            .error_for_status()
            .with_context(|| format!("update asset request failed for {url}"))?;
        validate_response_url(url, response.url())?;
        response
            .bytes()
            .await
            .context("failed to read update asset response")
            .map(|bytes| bytes.to_vec())
    }

    async fn download_with_progress(
        &self,
        url: &str,
        progress: &(dyn Fn(UpdateProgress) + Send + Sync),
    ) -> anyhow::Result<Vec<u8>> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("failed to request update asset {url}"))?
            .error_for_status()
            .with_context(|| format!("update asset request failed for {url}"))?;
        validate_response_url(url, response.url())?;
        let total_bytes = response.content_length();
        let mut downloaded_bytes = 0;
        let mut archive = Vec::with_capacity(total_bytes.unwrap_or_default() as usize);
        let mut response = response;
        progress(UpdateProgress {
            stage: UpdateStage::Downloading,
            downloaded_bytes,
            total_bytes,
        });
        while let Some(chunk) = response
            .chunk()
            .await
            .context("failed to read update asset response")?
        {
            downloaded_bytes = downloaded_bytes.saturating_add(chunk.len() as u64);
            archive.extend_from_slice(&chunk);
            progress(UpdateProgress {
                stage: UpdateStage::Downloading,
                downloaded_bytes,
                total_bytes,
            });
        }
        Ok(archive)
    }
}

pub trait InstallationStateSource {
    fn state(&self) -> anyhow::Result<Option<InstallationState>>;
}

pub trait UpdateFileSystem {
    fn read_to_string(&self, path: &Path) -> std::io::Result<String>;
}

pub struct SystemUpdateFileSystem;

impl UpdateFileSystem for SystemUpdateFileSystem {
    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        fs::read_to_string(path)
    }
}

pub struct InstallationStateFileSource<F = SystemUpdateFileSystem> {
    path: PathBuf,
    file_system: F,
}

impl<F: UpdateFileSystem> InstallationStateSource for InstallationStateFileSource<F> {
    fn state(&self) -> anyhow::Result<Option<InstallationState>> {
        let input = match self.file_system.read_to_string(&self.path) {
            Ok(input) => input,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => anyhow::bail!("failed to read {}: {error}", self.path.display()),
        };
        parse_installation_state(&input).map(Some).map_err(|error| {
            anyhow::anyhow!(
                "invalid installation state at {}: {error}",
                self.path.display()
            )
        })
    }
}

pub type SystemInstallationStateSource = InstallationStateFileSource<SystemUpdateFileSystem>;

pub const UPDATE_STATUSES: [&str; 5] = [
    "up_to_date",
    "update_available",
    "updated",
    "manager_action_required",
    "error",
];

pub async fn run(args: crate::cli::UpdateArgs, _config: Option<PathBuf>) -> anyhow::Result<String> {
    let mut progress = if args.json {
        None
    } else {
        Some(cli_progress::CliProgress::start()?)
    };
    let mut installing = false;
    let mut result = run_with_progress(&args, progress.as_ref(), &mut installing).await;
    if let Some(progress) = &mut progress {
        let cleanup = progress.finish();
        if result.is_ok() {
            result = result.and_then(|output| cleanup.map(|()| output).map_err(Into::into));
        }
    }
    if args.json {
        result
    } else {
        result.context(if installing {
            "LazyDB update failed"
        } else {
            "Could not check for LazyDB updates"
        })
    }
}

async fn run_with_progress(
    args: &crate::cli::UpdateArgs,
    progress: Option<&cli_progress::CliProgress>,
    installing: &mut bool,
) -> anyhow::Result<String> {
    let paths = Some(crate::persistence::paths::AppPaths::discover()?);
    let source = SystemInstallationStateSource {
        path: paths
            .as_ref()
            .map(|paths| paths.data_dir.join("install.json"))
            .unwrap_or_else(|| PathBuf::from("install.json")),
        file_system: SystemUpdateFileSystem,
    };
    let mut report = inspect_local_installation(
        args.channel,
        args.allow_downgrade,
        &source,
        &SystemInstallationProbe,
        &SystemUpdateHttpClient::default(),
    )
    .await;
    let mut display_context = UpdateDisplayContext {
        previous_version: report.current_version.clone(),
        ..Default::default()
    };
    if !args.check
        && report.manager == InstallationManager::Native
        && report.status == "update_available"
    {
        *installing = true;
        if let Some(progress) = progress {
            progress.available(&report)?;
        }
        let state = source
            .state()?
            .ok_or_else(|| anyhow::anyhow!("native installation state is unavailable"))?;
        let target = current_target(Some(&state))
            .ok_or_else(|| anyhow::anyhow!("current target is unsupported"))?;
        let manifest = fetch_manifest(report.channel, &SystemUpdateHttpClient::default())
            .await
            .map_err(anyhow::Error::msg)?;
        let lock = UpdateLock::acquire(&native_data_dir(&state)?)?;
        let version = apply_native_update_with_progress(
            &state,
            &target,
            &manifest,
            &SystemUpdateHttpClient::default(),
            &|value| {
                if let Some(progress) = progress {
                    progress.progress(value);
                }
            },
        )
        .await?;
        drop(lock);
        report.target_version = Some(version.clone());
        report.current_version = Some(version);
        report.status = "updated".to_owned();
        report.action = None;
        display_context.applied_in_this_run = true;
    }
    if args.json {
        return Ok(serde_json::to_string(&report)?);
    }
    if report.status == "error" {
        anyhow::bail!(
            "{}",
            report.action.as_deref().unwrap_or("unknown update error")
        );
    }
    Ok(format_update_report(
        &report,
        &display_context,
        output_style(),
    ))
}

async fn inspect_local_installation<P, S, H>(
    requested_channel: Option<UpdateChannel>,
    allow_downgrade: bool,
    source: &S,
    probe: &P,
    http: &H,
) -> UpdateReport
where
    P: InstallationProbe,
    S: InstallationStateSource,
    H: UpdateHttpClient + Sync,
{
    let inspection = inspect_installation(
        requested_channel,
        allow_downgrade,
        source,
        probe,
        &std::env::current_exe().unwrap_or_else(|_| PathBuf::from("lazydb")),
        http,
    )
    .await;
    report_from_inspection(&inspection)
}

pub async fn inspect_current_installation(
    requested_channel: Option<UpdateChannel>,
    allow_downgrade: bool,
) -> anyhow::Result<UpdateInspection> {
    let paths = Some(crate::persistence::paths::AppPaths::discover()?);
    let source = InstallationStateFileSource {
        path: paths
            .as_ref()
            .map(|paths| paths.data_dir.join("install.json"))
            .unwrap_or_else(|| PathBuf::from("install.json")),
        file_system: SystemUpdateFileSystem,
    };
    Ok(inspect_installation(
        requested_channel,
        allow_downgrade,
        &source,
        &SystemInstallationProbe,
        &std::env::current_exe().unwrap_or_else(|_| PathBuf::from("lazydb")),
        &SystemUpdateHttpClient::default(),
    )
    .await)
}

pub async fn install_current_native(
    requested_channel: Option<UpdateChannel>,
    allow_downgrade: bool,
) -> anyhow::Result<UpdateInspection> {
    install_current_native_with_progress(requested_channel, allow_downgrade, |_| {}).await
}

pub async fn install_current_native_with_progress<F>(
    requested_channel: Option<UpdateChannel>,
    allow_downgrade: bool,
    progress: F,
) -> anyhow::Result<UpdateInspection>
where
    F: Fn(UpdateProgress) + Send + Sync,
{
    let paths = Some(crate::persistence::paths::AppPaths::discover()?);
    let source = InstallationStateFileSource {
        path: paths
            .as_ref()
            .map(|paths| paths.data_dir.join("install.json"))
            .unwrap_or_else(|| PathBuf::from("install.json")),
        file_system: SystemUpdateFileSystem,
    };
    let executable = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("lazydb"));
    let initial_state = source
        .state()?
        .ok_or_else(|| anyhow::anyhow!("native installation state is unavailable"))?;
    if detect_installation_manager(&executable, Some(&initial_state), &SystemInstallationProbe)
        != InstallationManager::Native
    {
        anyhow::bail!("the current installation is not managed by LazyDB")
    }
    let lock = UpdateLock::acquire(&native_data_dir(&initial_state)?)?;
    let current_state = source
        .state()?
        .ok_or_else(|| anyhow::anyhow!("native installation state is unavailable"))?;
    if detect_installation_manager(&executable, Some(&current_state), &SystemInstallationProbe)
        != InstallationManager::Native
    {
        anyhow::bail!("the current installation changed during update preparation")
    }
    let channel = resolve_channel(requested_channel, Some(current_state.channel.as_str()));
    let manifest = fetch_manifest(channel, &SystemUpdateHttpClient::default())
        .await
        .map_err(anyhow::Error::msg)?;
    let target = current_target(Some(&current_state))
        .ok_or_else(|| anyhow::anyhow!("current target is unsupported"))?;
    let current_version = Version::parse(&current_state.version)
        .map_err(|_| anyhow::anyhow!("native installation version is invalid"))?;
    let target_version = Version::parse(&manifest.version)
        .map_err(|_| anyhow::anyhow!("channel manifest version is invalid"))?;
    if target_version <= current_version && !allow_downgrade {
        return Ok(inspection_after_native_install(
            &current_state,
            manifest.version.clone(),
            if target_version == current_version {
                UpdateStatus::UpToDate
            } else {
                UpdateStatus::Error
            },
        ));
    }
    apply_native_update_with_progress(
        &current_state,
        &target,
        &manifest,
        &SystemUpdateHttpClient::default(),
        &progress,
    )
    .await?;
    let installed = source
        .state()?
        .ok_or_else(|| anyhow::anyhow!("updated installation state is unavailable"))?;
    if installed.version != manifest.version {
        anyhow::bail!("updated installation state does not match the downloaded release")
    }
    drop(lock);
    Ok(inspection_after_native_install(
        &installed,
        manifest.version,
        UpdateStatus::ReadyToRestart,
    ))
}

async fn inspect_installation<P, S, H>(
    requested_channel: Option<UpdateChannel>,
    allow_downgrade: bool,
    source: &S,
    probe: &P,
    executable: &Path,
    http: &H,
) -> UpdateInspection
where
    P: InstallationProbe,
    S: InstallationStateSource,
    H: UpdateHttpClient + Sync,
{
    let (state, state_error) = match source.state() {
        Ok(state) => (state, None),
        Err(error) => (None, Some(format!("{error:#}"))),
    };
    let manager = detect_installation_manager(executable, state.as_ref(), probe);
    let channel = resolve_channel(
        requested_channel,
        state.as_ref().map(|state| state.channel.as_str()),
    );
    let running_version = env!("CARGO_PKG_VERSION").to_owned();
    let native_state = (manager == InstallationManager::Native)
        .then_some(state.as_ref())
        .flatten();
    let installed_version = native_state
        .map(|state| state.version.clone())
        .or_else(|| Some(running_version.clone()));
    let launcher_path = native_state.map(|state| state.path.clone());
    let manifest = fetch_manifest(channel, http).await;
    let target_version = manifest
        .as_ref()
        .ok()
        .map(|manifest| manifest.version.clone());
    let runtime_error = manifest.as_ref().ok().and_then(|manifest| {
        current_target(None).and_then(|target| {
            manifest.assets.get(&target).and_then(|asset| {
                asset.requirements.as_ref().and_then(|requirements| {
                    check_linux_runtime_requirements(&requirements.libc)
                        .err()
                        .map(|error| format!("{error:#}"))
                })
            })
        })
    });
    let status = match (&manifest, &runtime_error) {
        (Ok(_), Some(_)) => UpdateStatus::Error,
        (Ok(_), None) => match manager {
            InstallationManager::Native => {
                let status = version_status_kind(
                    installed_version.as_deref(),
                    target_version.as_deref().unwrap_or_default(),
                    allow_downgrade,
                );
                if status == UpdateStatus::UpToDate
                    && installed_version.as_deref() != Some(running_version.as_str())
                {
                    UpdateStatus::ReadyToRestart
                } else {
                    status
                }
            }
            _ => version_status_kind(
                installed_version.as_deref(),
                target_version.as_deref().unwrap_or_default(),
                allow_downgrade,
            )
            .map_manager_action_required(),
        },
        (Err(_), _) => UpdateStatus::Error,
    };
    let action = match status {
        UpdateStatus::Available if manager == InstallationManager::Native => {
            Some("native target update will be applied".to_owned())
        }
        UpdateStatus::ManagerActionRequired => {
            let action = manager_action(manager, channel);
            if manager == InstallationManager::Unknown {
                state_error
                    .map(|error| format!("{error}; {}", action.as_deref().unwrap_or_default()))
                    .or(action)
            } else {
                action
            }
        }
        UpdateStatus::Error => Some(match (manifest, runtime_error) {
            (Ok(_), Some(error)) => error,
            (Ok(_), None) => {
                "installed version is invalid; repair the native installation".to_owned()
            }
            (Err(error), _) => error,
        }),
        _ => None,
    };
    UpdateInspection {
        manager,
        channel,
        running_version,
        installed_version,
        target_version,
        status,
        action,
        launcher_path,
    }
}

trait UpdateStatusManagerAction {
    fn map_manager_action_required(self) -> UpdateStatus;
}

impl UpdateStatusManagerAction for UpdateStatus {
    fn map_manager_action_required(self) -> UpdateStatus {
        match self {
            UpdateStatus::Available => UpdateStatus::ManagerActionRequired,
            status => status,
        }
    }
}

fn report_from_inspection(inspection: &UpdateInspection) -> UpdateReport {
    let status = match inspection.status {
        UpdateStatus::UpToDate => "up_to_date",
        UpdateStatus::Available => "update_available",
        UpdateStatus::ReadyToRestart => "updated",
        UpdateStatus::ManagerActionRequired => "manager_action_required",
        UpdateStatus::Error => "error",
    };
    UpdateReport {
        schema: 1,
        manager: inspection.manager,
        channel: inspection.channel,
        current_version: inspection.installed_version.clone(),
        target_version: inspection.target_version.clone(),
        status: status.to_owned(),
        action: inspection.action.clone(),
    }
}

fn inspection_after_native_install(
    state: &InstallationState,
    target_version: String,
    status: UpdateStatus,
) -> UpdateInspection {
    UpdateInspection {
        manager: InstallationManager::Native,
        channel: resolve_channel(None, Some(state.channel.as_str())),
        running_version: env!("CARGO_PKG_VERSION").to_owned(),
        installed_version: Some(state.version.clone()),
        target_version: Some(target_version),
        status,
        action: None,
        launcher_path: Some(state.path.clone()),
    }
}

fn current_target(state: Option<&InstallationState>) -> Option<String> {
    state
        .map(|state| state.target.clone())
        .or_else(|| {
            Some(
                match (std::env::consts::ARCH, std::env::consts::OS) {
                    ("x86_64", "macos") => "x86_64-apple-darwin",
                    ("aarch64", "macos") => "aarch64-apple-darwin",
                    ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
                    ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
                    ("x86_64", "windows") => "x86_64-pc-windows-msvc",
                    _ => "unsupported",
                }
                .to_owned(),
            )
        })
        .and_then(|target| {
            SUPPORTED_TARGETS
                .contains(&target.as_str())
                .then_some(target)
        })
}

fn version_status_kind(current: Option<&str>, target: &str, allow_downgrade: bool) -> UpdateStatus {
    match version_status(current, target, allow_downgrade) {
        "update_available" => UpdateStatus::Available,
        "up_to_date" => UpdateStatus::UpToDate,
        _ => UpdateStatus::Error,
    }
}

fn version_status(current: Option<&str>, target: &str, allow_downgrade: bool) -> &'static str {
    let Some(current) = current.and_then(|version| Version::parse(version).ok()) else {
        return "error";
    };
    let Ok(target) = Version::parse(target) else {
        return "error";
    };
    match target.cmp(&current) {
        std::cmp::Ordering::Greater => "update_available",
        std::cmp::Ordering::Equal => "up_to_date",
        std::cmp::Ordering::Less if allow_downgrade => "update_available",
        std::cmp::Ordering::Less => "up_to_date",
    }
}

async fn fetch_manifest<H: UpdateHttpClient + Sync>(
    channel: UpdateChannel,
    http: &H,
) -> Result<ChannelManifest, String> {
    let base = std::env::var("LAZYDB_CHANNEL_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_CHANNEL_BASE_URL.to_owned());
    let base_url = url::Url::parse(&base).map_err(|error| error.to_string())?;
    let local_fixture = matches!(
        base_url.host_str(),
        Some("127.0.0.1") | Some("localhost") | Some("fixture")
    );
    if !(base_url.scheme() == "https" && base_url.host_str() == Some(PAGES_HOST)
        || local_fixture && base_url.scheme() == "http")
    {
        return Err("channel base URL must use HTTPS".to_owned());
    }
    let v2_url = format!(
        "{}/v2/{}.json",
        base.trim_end_matches('/'),
        channel_name(channel)
    );
    let (url, input, expected_schema) = match http.get_manifest(&v2_url).await {
        Ok((200, body)) => (v2_url, body, 2),
        Ok((404, _)) => {
            let legacy_url = format!(
                "{}/{}.json",
                base.trim_end_matches('/'),
                channel_name(channel)
            );
            let (status, body) = http
                .get_manifest(&legacy_url)
                .await
                .map_err(|error| format!("failed to fetch {legacy_url}: {error:#}"))?;
            if status != 200 {
                return Err(format!(
                    "channel manifest request returned HTTP {status} at {legacy_url}"
                ));
            }
            (legacy_url, body, 1)
        }
        Ok((status, _)) => {
            return Err(format!(
                "channel manifest request returned HTTP {status} at {v2_url}"
            ));
        }
        Err(error) => return Err(format!("failed to fetch {v2_url}: {error:#}")),
    };
    let manifest = parse_channel_manifest(&input)
        .map_err(|error| format!("invalid channel manifest at {url}: {error}"))?;
    if manifest.schema != expected_schema {
        return Err(format!("channel manifest schema mismatch at {url}"));
    }
    (manifest.channel == channel)
        .then_some(manifest)
        .ok_or_else(|| "manifest channel mismatch".to_owned())
}

fn channel_name(channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Stable => "stable",
        UpdateChannel::Beta => "beta",
    }
}

fn persisted_channel(channel: &str) -> Option<UpdateChannel> {
    match channel {
        "stable" => Some(UpdateChannel::Stable),
        "beta" => Some(UpdateChannel::Beta),
        _ => None,
    }
}

fn resolve_channel(
    requested_channel: Option<UpdateChannel>,
    persisted: Option<&str>,
) -> UpdateChannel {
    requested_channel
        .or_else(|| persisted.and_then(persisted_channel))
        .unwrap_or_default()
}

/// The exact command a user can run to update through a known installation
/// manager. Returns `None` when the manager cannot be updated with a single
/// unambiguous command (for example system package managers that need sudo).
pub fn manager_update_command(manager: InstallationManager) -> Option<String> {
    match manager {
        InstallationManager::Homebrew => Some("brew upgrade yelog/tap/lazydb".to_owned()),
        InstallationManager::Cargo => Some("cargo install lazydb".to_owned()),
        _ => None,
    }
}

/// Human-readable guidance for updating through an installation manager. This
/// text never masquerades as a copyable command.
pub fn manager_update_message(
    manager: InstallationManager,
    _channel: UpdateChannel,
) -> Option<String> {
    match manager {
        InstallationManager::Homebrew => Some("Update LazyDB with Homebrew".to_owned()),
        InstallationManager::Cargo => Some("Update LazyDB with Cargo".to_owned()),
        InstallationManager::Npm => Some(
            "official npm distribution is unavailable; use the Pages installer or Homebrew"
                .to_owned(),
        ),
        InstallationManager::Deb => Some(
            "use your Debian package manager to upgrade lazydb; LazyDB will not invoke sudo"
                .to_owned(),
        ),
        InstallationManager::Rpm => Some(
            "use your RPM package manager to upgrade lazydb; LazyDB will not invoke sudo"
                .to_owned(),
        ),
        InstallationManager::Arch => Some(
            "use your Arch package manager to upgrade lazydb; LazyDB will not invoke sudo"
                .to_owned(),
        ),
        InstallationManager::Unknown => {
            Some("installation manager could not be determined; update using the original installation method or the official installer at https://lazydb.yelog.org/install.sh".to_owned())
        }
        InstallationManager::Native => None,
    }
}

fn manager_action(manager: InstallationManager, channel: UpdateChannel) -> Option<String> {
    manager_update_command(manager).or_else(|| manager_update_message(manager, channel))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum UpdateOutputStyle {
    #[default]
    Plain,
    Styled,
}

#[derive(Default)]
struct UpdateDisplayContext {
    previous_version: Option<String>,
    applied_in_this_run: bool,
}

fn output_style() -> UpdateOutputStyle {
    let is_terminal = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    let dumb_terminal = std::env::var("TERM").is_ok_and(|term| term == "dumb");
    output_style_for(is_terminal, no_color, dumb_terminal)
}

fn output_style_for(is_terminal: bool, no_color: bool, dumb_terminal: bool) -> UpdateOutputStyle {
    if is_terminal && !no_color && !dumb_terminal {
        UpdateOutputStyle::Styled
    } else {
        UpdateOutputStyle::Plain
    }
}

fn format_update_report(
    report: &UpdateReport,
    context: &UpdateDisplayContext,
    style: UpdateOutputStyle,
) -> String {
    let (marker, title, color) = match report.status.as_str() {
        "updated" if context.applied_in_this_run => ("✓", "LazyDB updated successfully", "green"),
        "updated" => ("✓", "LazyDB update is already installed", "green"),
        "up_to_date" if versions_match(report) => ("✓", "LazyDB is already up to date", "green"),
        "up_to_date" => ("[OK]", "No LazyDB update needed", "green"),
        "update_available" => ("↑", "A LazyDB update is available", "cyan"),
        "manager_action_required" if report.manager != InstallationManager::Unknown => {
            ("!", manager_title(report.manager), "yellow")
        }
        "manager_action_required" => ("!", "Manual update required for LazyDB", "yellow"),
        "error" => ("✗", "Could not check for LazyDB updates", "red"),
        _ => ("?", "LazyDB update status", "yellow"),
    };
    let marker = match style {
        UpdateOutputStyle::Plain => plain_marker(marker),
        UpdateOutputStyle::Styled => marker,
    };
    let title = match style {
        UpdateOutputStyle::Plain => format!("{marker} {title}"),
        UpdateOutputStyle::Styled => match color {
            "green" => format!("{}", format!("{marker} {title}").green().bold()),
            "cyan" => format!("{}", format!("{marker} {title}").cyan().bold()),
            "yellow" => format!("{}", format!("{marker} {title}").yellow().bold()),
            "red" => format!("{}", format!("{marker} {title}").red().bold()),
            _ => format!("{marker} {title}"),
        },
    };
    let mut lines = vec![title];

    match report.status.as_str() {
        "updated" if context.applied_in_this_run => {
            push_version_line(
                &mut lines,
                "Version",
                context.previous_version.as_deref(),
                report.current_version.as_deref(),
            );
        }
        "updated" => push_version_line(
            &mut lines,
            "Installed",
            report.current_version.as_deref(),
            None,
        ),
        "up_to_date" if versions_match(report) => {
            push_value_line(&mut lines, "Version", report.current_version.as_deref());
        }
        "up_to_date" => {
            push_value_line(&mut lines, "Installed", report.current_version.as_deref());
            push_value_line(
                &mut lines,
                "Channel latest",
                report.target_version.as_deref(),
            );
        }
        "update_available" => {
            push_version_line(
                &mut lines,
                "Version",
                report.current_version.as_deref(),
                report.target_version.as_deref(),
            );
            push_value_line(&mut lines, "Run", Some(update_command(report.channel)));
        }
        "manager_action_required" => {
            push_value_line(&mut lines, "Current", report.current_version.as_deref());
        }
        _ => {}
    }
    push_value_line(&mut lines, "Channel", Some(channel_name(report.channel)));
    match report.status.as_str() {
        "manager_action_required" => {
            let label = matches!(
                report.manager,
                InstallationManager::Homebrew | InstallationManager::Cargo
            )
            .then_some("Run")
            .unwrap_or("Next step");
            push_value_line(&mut lines, label, report.action.as_deref());
        }
        "error" => push_value_line(&mut lines, "Reason", report.action.as_deref()),
        status if status != "update_available" && status != "updated" => {
            push_value_line(&mut lines, "Next step", report.action.as_deref());
            if status != "up_to_date" {
                push_value_line(&mut lines, "Status", Some(status));
            }
        }
        _ => {}
    }
    lines.join("\n")
}

fn versions_match(report: &UpdateReport) -> bool {
    let (Some(current), Some(target)) = (
        report.current_version.as_deref(),
        report.target_version.as_deref(),
    ) else {
        return false;
    };
    Version::parse(current).ok() == Version::parse(target).ok() && Version::parse(current).is_ok()
}

fn plain_marker(marker: &str) -> &str {
    match marker {
        "✓" => "[OK]",
        "↑" => "[INFO]",
        "!" => "[WARN]",
        "✗" => "[ERROR]",
        _ => marker,
    }
}

fn push_value_line(lines: &mut Vec<String>, label: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        lines.push(format!("  {label:<13}{value}"));
    }
}

fn push_version_line(
    lines: &mut Vec<String>,
    label: &str,
    current: Option<&str>,
    target: Option<&str>,
) {
    match (
        current.filter(|value| !value.is_empty()),
        target.filter(|value| !value.is_empty()),
    ) {
        (Some(current), Some(target)) => lines.push(format!("  {label:<13}{current} -> {target}")),
        (Some(current), None) => push_value_line(lines, label, Some(current)),
        (None, Some(target)) => push_value_line(lines, "Target", Some(target)),
        (None, None) => {}
    }
}

fn update_command(channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Stable => "lazydb update",
        UpdateChannel::Beta => "lazydb update --channel beta",
    }
}

fn manager_title(manager: InstallationManager) -> &'static str {
    match manager {
        InstallationManager::Homebrew => "Update LazyDB using Homebrew",
        InstallationManager::Npm => "Update LazyDB using npm",
        InstallationManager::Deb => "Update LazyDB using the Debian package manager",
        InstallationManager::Rpm => "Update LazyDB using the RPM package manager",
        InstallationManager::Arch => "Update LazyDB using the Arch package manager",
        InstallationManager::Cargo => "Update LazyDB using Cargo",
        _ => "Manual update required for LazyDB",
    }
}

pub(crate) struct UpdateLock {
    path: PathBuf,
}

impl UpdateLock {
    pub(crate) fn acquire(data_dir: &Path) -> anyhow::Result<Self> {
        Self::acquire_path(data_dir.join(".install.lock"))
    }

    pub(crate) fn acquire_path(path: PathBuf) -> anyhow::Result<Self> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("update lock has no parent directory"))?;
        fs::create_dir_all(parent)?;
        match fs::create_dir(&path) {
            Ok(()) => {
                if let Err(error) = fs::write(path.join("pid"), std::process::id().to_string()) {
                    let _ = fs::remove_dir_all(&path);
                    return Err(error.into());
                }
                Ok(Self { path })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(anyhow::anyhow!("another lazydb update is in progress"))
            }
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn native_data_dir(state: &InstallationState) -> anyhow::Result<PathBuf> {
    let link = fs::read_link(&state.path)
        .map_err(|error| anyhow::anyhow!("native executable is not a symlink: {error}"))?;
    let resolved = if link.is_absolute() {
        link
    } else {
        state
            .path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(link)
    };
    let current = resolved
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "current"))
        .ok_or_else(|| anyhow::anyhow!("native executable does not point through current"))?;
    Ok(current
        .parent()
        .ok_or_else(|| anyhow::anyhow!("native installation root is unavailable"))?
        .to_owned())
}

#[cfg(test)]
async fn apply_native_update<H: UpdateHttpClient + Sync>(
    state: &InstallationState,
    target: &str,
    manifest: &ChannelManifest,
    http: &H,
) -> anyhow::Result<String> {
    apply_native_update_with_progress(state, target, manifest, http, &|_| {}).await
}

async fn apply_native_update_with_progress<H: UpdateHttpClient + Sync>(
    state: &InstallationState,
    target: &str,
    manifest: &ChannelManifest,
    http: &H,
    progress: &(dyn Fn(UpdateProgress) + Send + Sync),
) -> anyhow::Result<String> {
    let asset = manifest
        .assets
        .get(target)
        .ok_or_else(|| anyhow::anyhow!("current target is unsupported"))?;
    progress(UpdateProgress {
        stage: UpdateStage::Preparing,
        downloaded_bytes: 0,
        total_bytes: None,
    });
    if let Some(requirements) = &asset.requirements {
        check_linux_runtime_requirements(&requirements.libc)?;
    }
    let archive = http.download_with_progress(&asset.url, progress).await?;
    progress(UpdateProgress {
        stage: UpdateStage::VerifyingChecksum,
        downloaded_bytes: archive.len() as u64,
        total_bytes: Some(archive.len() as u64),
    });
    let actual = format!("{:x}", Sha256::digest(&archive));
    if actual != asset.sha256 {
        anyhow::bail!("checksum mismatch")
    }

    let data_dir = native_data_dir(state)?;
    let temp_name = format!(".update-{}", unique_suffix());
    let staging = data_dir.join(&temp_name);
    let release = staging.join("release");
    fs::create_dir_all(&release)?;
    progress(UpdateProgress {
        stage: UpdateStage::Extracting,
        downloaded_bytes: archive.len() as u64,
        total_bytes: Some(archive.len() as u64),
    });
    if let Err(error) = extract_archive(&archive, &release, manifest, target) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let binary_name = if target == "x86_64-pc-windows-msvc" {
        "lazydb.exe"
    } else {
        "lazydb"
    };
    let binary = release.join(binary_name);
    progress(UpdateProgress {
        stage: UpdateStage::VerifyingInstallation,
        downloaded_bytes: archive.len() as u64,
        total_bytes: Some(archive.len() as u64),
    });
    let reported = match run_staged_version(&binary) {
        Ok(version) => version,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if reported != manifest.version {
        let _ = fs::remove_dir_all(&staging);
        anyhow::bail!(
            "staged binary reported version {reported}, expected {}",
            manifest.version
        )
    }

    let releases = data_dir.join("releases");
    fs::create_dir_all(&releases)?;
    let destination = releases.join(&manifest.version);
    let destination_created = !destination.exists();
    if !destination_created {
        let existing_binary = destination.join(binary_name);
        let existing_is_valid = existing_binary.is_file()
            && run_staged_version(&existing_binary)
                .is_ok_and(|version| version == manifest.version);
        if existing_is_valid {
            fs::remove_dir_all(&staging)?;
        } else {
            fs::remove_dir_all(&destination)?;
            fs::rename(&release, &destination)?;
            fs::remove_dir_all(&staging)?;
        }
    } else {
        if let Err(error) = fs::rename(&release, &destination) {
            let _ = fs::remove_dir_all(&staging);
            return Err(error.into());
        }
        fs::remove_dir_all(&staging)?;
    }
    progress(UpdateProgress {
        stage: UpdateStage::Activating,
        downloaded_bytes: archive.len() as u64,
        total_bytes: Some(archive.len() as u64),
    });
    if let Err(error) = publish_native_state(state, &data_dir, &destination, manifest, target) {
        if destination_created {
            let _ = fs::remove_dir_all(&destination);
        }
        return Err(error);
    }
    progress(UpdateProgress {
        stage: UpdateStage::Finishing,
        downloaded_bytes: archive.len() as u64,
        total_bytes: Some(archive.len() as u64),
    });
    Ok(manifest.version.clone())
}

fn check_linux_runtime_requirements(requirement: &LibcRequirement) -> anyhow::Result<()> {
    if requirement.family != "glibc" {
        anyhow::bail!("unsupported Linux libc requirement: {}", requirement.family)
    }
    if !cfg!(target_os = "linux") {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        let output = Command::new("getconf")
            .arg("GNU_LIBC_VERSION")
            .output()
            .context("could not determine the installed glibc version")?;
        if !output.status.success() {
            anyhow::bail!("could not determine the installed glibc version")
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let current = text
            .split_whitespace()
            .last()
            .ok_or_else(|| anyhow::anyhow!("invalid getconf output"))?;
        let current =
            Version::parse(&format!("{}.0", current)).or_else(|_| Version::parse(current))?;
        let required = Version::parse(&format!("{}.0", requirement.min_version))
            .or_else(|_| Version::parse(&requirement.min_version))?;
        if current < required {
            anyhow::bail!(
                "Linux runtime requires glibc {}, but this system has {}",
                requirement.min_version,
                current
            )
        }
    }
    Ok(())
}

fn extract_archive(
    archive: &[u8],
    destination: &Path,
    manifest: &ChannelManifest,
    target: &str,
) -> anyhow::Result<()> {
    let expected_root = format!("lazydb_{}_{}", manifest.version, target);
    let mut names = std::collections::BTreeSet::new();
    let binary_file = if target == "x86_64-pc-windows-msvc" {
        "lazydb.exe"
    } else {
        "lazydb"
    };
    let binary_name = format!("{expected_root}/{binary_file}");
    let mut root_seen = false;
    let mut binary_seen = false;
    let mut validate_entry = |raw_name: &str, is_dir: bool, is_file: bool| -> anyhow::Result<()> {
        // A single trailing slash is a directory marker, not an empty path component.
        let name = if is_dir {
            raw_name.strip_suffix('/').unwrap_or(raw_name)
        } else {
            raw_name
        };
        if !archive_entry_path_is_normal(name)
            || !(name == expected_root || name.starts_with(&format!("{expected_root}/")))
            || !names.insert(name.to_owned())
            || !(is_dir || is_file)
            || (name == expected_root && !is_dir)
            || (name == binary_name && !is_file)
        {
            anyhow::bail!("unsafe archive entry: {raw_name}")
        }
        root_seen |= name == expected_root && is_dir;
        binary_seen |= name == binary_name && is_file;
        Ok(())
    };
    if target == "x86_64-pc-windows-msvc" {
        let mut zip = zip::ZipArchive::new(Cursor::new(archive))?;
        for index in 0..zip.len() {
            let entry = zip.by_index(index)?;
            let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
            if entry.is_symlink() || !matches!(mode, 0 | 0o040000 | 0o100000) {
                anyhow::bail!("unsafe archive entry: {}", entry.name())
            }
            validate_entry(entry.name(), entry.is_dir(), entry.is_file())?;
        }
        if !root_seen || !binary_seen {
            anyhow::bail!("archive does not contain the expected executable")
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(archive))?;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index)?;
            let output = destination.join(entry.name());
            if entry.is_dir() {
                fs::create_dir_all(output)?;
            } else {
                if let Some(parent) = output.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut file = fs::File::create(output)?;
                std::io::copy(&mut entry, &mut file)?;
            }
        }
    } else {
        let mut decoder = XzDecoder::new(Cursor::new(archive));
        let mut tar_bytes = Vec::new();
        decoder.read_to_end(&mut tar_bytes)?;
        let mut tar_archive = tar::Archive::new(Cursor::new(&tar_bytes));
        for entry in tar_archive.entries()? {
            let entry = entry?;
            let path = entry.path_bytes();
            let name = std::str::from_utf8(&path)?;
            let entry_type = entry.header().entry_type();
            validate_entry(name, entry_type.is_dir(), entry_type.is_file())?;
        }
        if !root_seen || !binary_seen {
            anyhow::bail!("archive does not contain the expected executable")
        }
        let mut tar_archive = tar::Archive::new(Cursor::new(&tar_bytes));
        tar_archive.unpack(destination)?;
    }
    fs::rename(
        destination.join(&expected_root),
        destination.join("lazydb-root"),
    )?;
    let root = destination.join("lazydb-root");
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        let target_path = destination.join(entry.file_name());
        fs::rename(path, target_path)?;
    }
    fs::remove_dir(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(destination.join(binary_file))?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(destination.join(binary_file), permissions)?;
    }
    Ok(())
}

fn archive_entry_path_is_normal(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\0')
        && !name.contains('\\')
        && !name.contains(':')
        && Path::new(name)
            .components()
            .all(|component| matches!(component, Component::Normal(part) if !part.is_empty()))
        && !name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

fn run_staged_version(binary: &Path) -> anyhow::Result<String> {
    let output = Command::new(binary).args(["version", "--json"]).output()?;
    if !output.status.success() {
        anyhow::bail!("staged binary failed version check")
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    value["version"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("staged binary returned invalid version JSON"))
}

fn publish_native_state(
    state: &InstallationState,
    data_dir: &Path,
    destination: &Path,
    manifest: &ChannelManifest,
    target: &str,
) -> anyhow::Result<()> {
    let current = data_dir.join("current");
    let current_new = data_dir.join(".current.new");
    let current_old = data_dir.join(".current.old");
    let state_new = data_dir.join(".install.json.new");
    let state_path = data_dir.join("install.json");
    let state_old = data_dir.join(".install.json.old");
    #[cfg(unix)]
    {
        let relative =
            Path::new("releases").join(destination.file_name().ok_or_else(|| {
                anyhow::anyhow!("native release destination has no version name")
            })?);
        std::os::unix::fs::symlink(relative, &current_new)?;
    }
    let new_state = InstallationState {
        schema: 1,
        product: "lazydb".into(),
        manager: InstallationManager::Native,
        channel: channel_name(manifest.channel).into(),
        version: manifest.version.clone(),
        target: target.into(),
        path: state.path.clone(),
        bin_dir: state.bin_dir.clone(),
        installed_at: state.installed_at.clone(),
        shell_profiles: state.shell_profiles.clone(),
    };
    let mut file = fs::File::create(&state_new)?;
    serde_json::to_writer_pretty(&mut file, &new_state)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    if current.exists() || fs::symlink_metadata(&current).is_ok() {
        fs::rename(&current, &current_old)?;
    }
    if let Err(error) = fs::rename(&current_new, &current) {
        let _ = fs::rename(&current_old, &current);
        return Err(error.into());
    }
    if (state_path.exists() || fs::symlink_metadata(&state_path).is_ok())
        && let Err(error) = fs::rename(&state_path, &state_old)
    {
        let _ = fs::remove_file(&current);
        let _ = fs::rename(&current_old, &current);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&state_new, &state_path) {
        let _ = fs::remove_file(&current);
        let _ = fs::rename(&current_old, &current);
        let _ = fs::rename(&state_old, &state_path);
        return Err(error.into());
    }
    let _ = fs::remove_file(&current_old);
    let _ = fs::remove_file(&state_old);
    Ok(())
}

fn unique_suffix() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InstallationState {
    pub schema: u32,
    pub product: String,
    pub manager: InstallationManager,
    pub channel: String,
    pub version: String,
    pub target: String,
    pub path: PathBuf,
    #[serde(default)]
    pub bin_dir: Option<PathBuf>,
    #[serde(default)]
    pub installed_at: Option<String>,
    #[serde(default)]
    pub shell_profiles: Vec<ShellProfileState>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ShellProfileState {
    pub path: PathBuf,
    pub block_sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstallationManager {
    Native,
    Homebrew,
    Npm,
    Deb,
    Rpm,
    Arch,
    Cargo,
    Unknown,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StateError {
    #[error("invalid installation state JSON: {0}")]
    Json(String),
    #[error("unsupported installation state schema: {0}")]
    UnsupportedSchema(u32),
    #[error("installation state describes {product}, not lazydb")]
    WrongProduct { product: String },
    #[error("installation state manager must be native")]
    NotNative,
}

pub fn parse_installation_state(input: &str) -> Result<InstallationState, StateError> {
    let state: InstallationState =
        serde_json::from_str(input).map_err(|error| StateError::Json(error.to_string()))?;
    validate_installation_state(state)
}

pub fn read_installation_state(path: &Path) -> Result<InstallationState, StateError> {
    let input = fs::read_to_string(path).map_err(|error| StateError::Json(error.to_string()))?;
    parse_installation_state(&input)
}

fn validate_installation_state(state: InstallationState) -> Result<InstallationState, StateError> {
    if state.schema != INSTALLATION_SCHEMA {
        return Err(StateError::UnsupportedSchema(state.schema));
    }
    if state.product != "lazydb" {
        return Err(StateError::WrongProduct {
            product: state.product,
        });
    }
    if state.manager != InstallationManager::Native {
        return Err(StateError::NotNative);
    }
    Ok(state)
}

pub trait InstallationProbe {
    fn canonicalize(&self, path: &Path) -> Option<PathBuf>;
    fn read_link(&self, path: &Path) -> Option<PathBuf>;
    fn command_output(&self, command: &str, args: &[&str]) -> Option<String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemInstallationProbe;

impl InstallationProbe for SystemInstallationProbe {
    fn canonicalize(&self, path: &Path) -> Option<PathBuf> {
        fs::canonicalize(path).ok()
    }

    fn read_link(&self, path: &Path) -> Option<PathBuf> {
        fs::read_link(path).ok()
    }

    fn command_output(&self, command: &str, args: &[&str]) -> Option<String> {
        let output = Command::new(command).args(args).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
}

pub fn native_installation_is_active<P: InstallationProbe>(
    state: &InstallationState,
    executable: &Path,
    probe: &P,
) -> bool {
    if state.manager != InstallationManager::Native || state.product != "lazydb" {
        return false;
    }
    let Some(state_path) = probe.canonicalize(&state.path) else {
        return false;
    };
    let Some(executable_path) = probe.canonicalize(executable) else {
        return false;
    };
    if state_path != executable_path {
        return false;
    }

    // current_exe() may resolve symlinks (notably on Linux). Validate the
    // recorded launcher, after confirming it resolves to the running binary.
    probe.read_link(&state.path).is_some_and(|target| {
        let target_has_current = target
            .components()
            .any(|component| component.as_os_str() == "current");
        let resolved_target = if target.is_absolute() {
            target
        } else {
            state
                .path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(target)
        };
        target_has_current
            && probe
                .canonicalize(&resolved_target)
                .is_some_and(|current| current == executable_path)
    })
}

pub fn detect_installation_manager<P: InstallationProbe>(
    executable: &Path,
    state: Option<&InstallationState>,
    probe: &P,
) -> InstallationManager {
    if let Some(state) = state
        && native_installation_is_active(state, executable, probe)
    {
        return InstallationManager::Native;
    }

    let path = executable.to_string_lossy();
    if path.contains("/Cellar/") || brew_prefix_contains(executable, probe) {
        return InstallationManager::Homebrew;
    }
    if path.contains("node_modules") || npm_prefix_contains(executable, probe) {
        return InstallationManager::Npm;
    }
    if package_owned(executable, probe, "dpkg", "-S") {
        return InstallationManager::Deb;
    }
    if package_owned(executable, probe, "rpm", "-qf") {
        return InstallationManager::Rpm;
    }
    if package_owned(executable, probe, "pacman", "-Qo") {
        return InstallationManager::Arch;
    }
    if cargo_path(executable) {
        return InstallationManager::Cargo;
    }
    InstallationManager::Unknown
}

fn brew_prefix_contains<P: InstallationProbe>(path: &Path, probe: &P) -> bool {
    probe
        .command_output("brew", &["--prefix"])
        .is_some_and(|prefix| path.starts_with(prefix))
}

fn npm_prefix_contains<P: InstallationProbe>(path: &Path, probe: &P) -> bool {
    probe
        .command_output("npm", &["config", "get", "prefix"])
        .is_some_and(|prefix| path.starts_with(Path::new(&prefix)))
}

fn package_owned<P: InstallationProbe>(path: &Path, probe: &P, command: &str, query: &str) -> bool {
    probe
        .command_output(command, &[query, &path.to_string_lossy()])
        .is_some_and(|output| !output.is_empty())
}

fn cargo_path(path: &Path) -> bool {
    let path = path.to_string_lossy();
    path.contains("/.cargo/bin/")
        || path.contains("/target/debug/")
        || path.contains("/target/release/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;
    use xz2::write::XzEncoder;

    #[derive(Default)]
    struct FakeProbe {
        paths: HashMap<PathBuf, PathBuf>,
        commands: HashMap<(String, Vec<String>), String>,
    }

    impl InstallationProbe for FakeProbe {
        fn canonicalize(&self, path: &Path) -> Option<PathBuf> {
            self.paths
                .get(path)
                .cloned()
                .or_else(|| Some(path.to_owned()))
        }

        fn read_link(&self, path: &Path) -> Option<PathBuf> {
            (path.to_string_lossy().contains("/bin/lazydb"))
                .then(|| PathBuf::from("/tmp/data/current/lazydb"))
        }

        fn command_output(&self, command: &str, args: &[&str]) -> Option<String> {
            self.commands
                .get(&(
                    command.to_owned(),
                    args.iter().map(|arg| (*arg).to_owned()).collect(),
                ))
                .cloned()
        }
    }

    fn valid_state(path: &str) -> String {
        format!(
            r#"{{"schema":1,"product":"lazydb","manager":"native","channel":"stable","version":"1.2.3","target":"x86_64-apple-darwin","path":"{path}"}}"#
        )
    }

    #[test]
    fn parses_task_two_native_state() {
        let state = parse_installation_state(&valid_state("/tmp/bin/lazydb")).unwrap();
        assert_eq!(state.manager, InstallationManager::Native);
        assert_eq!(state.version, "1.2.3");
    }

    #[test]
    fn rejects_malformed_missing_and_unsupported_state() {
        assert!(matches!(
            parse_installation_state("{"),
            Err(StateError::Json(_))
        ));
        assert!(matches!(
            parse_installation_state(
                r#"{"schema":2,"product":"lazydb","manager":"native","channel":"stable","version":"1","target":"x","path":"/tmp/lazydb"}"#
            ),
            Err(StateError::UnsupportedSchema(2))
        ));
        assert!(matches!(
            parse_installation_state(r#"{"schema":1,"product":"lazydb","manager":"native"}"#),
            Err(StateError::Json(_))
        ));
        assert!(matches!(
            parse_installation_state(
                r#"{"schema":1,"product":"lazydb","manager":"homebrew","channel":"stable","version":"1","target":"x","path":"/tmp/lazydb"}"#
            ),
            Err(StateError::NotNative)
        ));
    }

    #[test]
    fn stale_native_metadata_does_not_authorize_replacement() {
        let state = parse_installation_state(&valid_state("/tmp/bin/lazydb")).unwrap();
        let probe = FakeProbe::default();
        assert!(!native_installation_is_active(
            &state,
            Path::new("/tmp/other/lazydb"),
            &probe
        ));
    }

    #[test]
    fn confirms_native_current_symlink() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let bin = dir.path().join("bin/lazydb");
        let release = data.join("releases/1.2.3/lazydb");
        let state = InstallationState {
            schema: 1,
            product: "lazydb".into(),
            manager: InstallationManager::Native,
            channel: "stable".into(),
            version: "1.2.3".into(),
            target: "x86_64-apple-darwin".into(),
            path: bin.clone(),
            bin_dir: None,
            installed_at: None,
            shell_profiles: Vec::new(),
        };
        let mut probe = FakeProbe::default();
        probe.paths.insert(bin.clone(), release.clone());
        probe
            .paths
            .insert(PathBuf::from("/tmp/data/current/lazydb"), release.clone());
        assert!(native_installation_is_active(&state, &bin, &probe));
    }

    #[test]
    fn detects_manager_paths_and_package_probes() {
        let mut probe = FakeProbe::default();
        assert_eq!(
            detect_installation_manager(
                Path::new("/opt/homebrew/Cellar/lazydb/1/bin/lazydb"),
                None,
                &probe
            ),
            InstallationManager::Homebrew
        );
        assert_eq!(
            detect_installation_manager(
                Path::new("/usr/local/lib/node_modules/@yelog/lazydb/bin/lazydb"),
                None,
                &probe
            ),
            InstallationManager::Npm
        );
        assert_eq!(
            detect_installation_manager(Path::new("/work/target/release/lazydb"), None, &probe),
            InstallationManager::Cargo
        );
        assert_eq!(
            detect_installation_manager(Path::new("/opt/lazydb"), None, &probe),
            InstallationManager::Unknown
        );
        probe.commands.insert(
            ("dpkg".into(), vec!["-S".into(), "/opt/lazydb".into()]),
            "lazydb: /opt/lazydb".into(),
        );
        assert_eq!(
            detect_installation_manager(Path::new("/opt/lazydb"), None, &probe),
            InstallationManager::Deb
        );

        probe
            .commands
            .remove(&("dpkg".into(), vec!["-S".into(), "/opt/lazydb".into()]));
        probe.commands.insert(
            ("rpm".into(), vec!["-qf".into(), "/opt/lazydb".into()]),
            "lazydb-1.2.3".into(),
        );
        assert_eq!(
            detect_installation_manager(Path::new("/opt/lazydb"), None, &probe),
            InstallationManager::Rpm
        );

        probe
            .commands
            .remove(&("rpm".into(), vec!["-qf".into(), "/opt/lazydb".into()]));
        probe.commands.insert(
            ("pacman".into(), vec!["-Qo".into(), "/opt/lazydb".into()]),
            "lazydb owns /opt/lazydb".into(),
        );
        assert_eq!(
            detect_installation_manager(Path::new("/opt/lazydb"), None, &probe),
            InstallationManager::Arch
        );
    }

    #[test]
    #[cfg(unix)]
    fn native_launcher_validation_accepts_resolved_executable_and_rejects_invalid_links() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let release = data.join("releases/1.2.3");
        fs::create_dir_all(&release).unwrap();
        let binary = release.join("lazydb");
        fs::write(&binary, b"binary").unwrap();
        let launcher = dir.path().join("bin/lazydb");
        fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        symlink(&release, data.join("current")).unwrap();
        let mut state = parse_installation_state(&valid_state("/unused")).unwrap();
        state.path = launcher.clone();
        for target in [
            data.join("current/lazydb"),
            PathBuf::from("../data/current/lazydb"),
        ] {
            symlink(target, &launcher).unwrap();
            assert_eq!(
                detect_installation_manager(&binary, Some(&state), &SystemInstallationProbe),
                InstallationManager::Native
            );
            assert!(native_installation_is_active(
                &state,
                &launcher,
                &SystemInstallationProbe
            ));
            assert!(!native_installation_is_active(
                &state,
                Path::new("/missing"),
                &SystemInstallationProbe
            ));
            fs::remove_file(&launcher).unwrap();
        }
        for target in [&binary, &data.join("current/missing")] {
            symlink(target, &launcher).unwrap();
            assert!(!native_installation_is_active(
                &state,
                &binary,
                &SystemInstallationProbe
            ));
            fs::remove_file(&launcher).unwrap();
        }
        state.path = binary.clone();
        assert!(!native_installation_is_active(
            &state,
            &binary,
            &SystemInstallationProbe
        ));
    }

    struct ManifestHttp(Result<String, &'static str>);

    #[async_trait]
    impl UpdateHttpClient for ManifestHttp {
        async fn get(&self, _url: &str) -> anyhow::Result<String> {
            self.0.clone().map_err(anyhow::Error::msg)
        }

        async fn get_manifest(&self, url: &str) -> anyhow::Result<(u16, String)> {
            if url.contains("/v2/") {
                return match &self.0 {
                    Err(error) => Err(anyhow::anyhow!(*error)),
                    Ok(body) if body == "not JSON" => Ok((200, body.clone())),
                    Ok(body) => Ok((200, body.clone())),
                };
            }
            self.get(url).await.map(|body| (200, body))
        }

        async fn download(&self, _url: &str) -> anyhow::Result<Vec<u8>> {
            panic!("inspection must not download an archive")
        }
    }

    #[tokio::test]
    async fn inspection_distinguishes_unknown_installation_network_and_invalid_metadata() {
        let dir = tempdir().unwrap();
        let source = InstallationStateFileSource {
            path: dir.path().join("install.json"),
            file_system: SystemUpdateFileSystem,
        };
        let probe = FakeProbe::default();
        let executable = Path::new("/opt/lazydb");
        let inspect = |http| inspect_installation(None, false, &source, &probe, executable, http);
        let http = ManifestHttp(Ok(manifest_fixture()));
        let inspection = inspect(&http).await;
        assert_eq!(inspection.manager, InstallationManager::Unknown);
        assert_eq!(inspection.target_version.as_deref(), Some("1.3.0"));
        assert_eq!(inspection.status, UpdateStatus::ManagerActionRequired);
        assert!(
            inspection
                .action
                .unwrap()
                .contains("original installation method")
        );

        let http = ManifestHttp(Err("connection timed out"));
        let inspection = inspect(&http).await;
        assert_eq!(inspection.status, UpdateStatus::Error);
        let message = inspection.action.unwrap();
        assert!(message.contains("failed to fetch"));
        assert!(message.contains("connection timed out"));

        let http = ManifestHttp(Ok("not JSON".into()));
        let inspection = inspect(&http).await;
        assert_eq!(inspection.status, UpdateStatus::Error);
        assert!(
            inspection
                .action
                .unwrap()
                .contains("invalid channel manifest")
        );

        fs::write(&source.path, "not JSON").unwrap();
        let http = ManifestHttp(Ok(manifest_fixture()));
        let inspection = inspect(&http).await;
        assert_eq!(inspection.status, UpdateStatus::ManagerActionRequired);
        let message = inspection.action.unwrap();
        assert!(message.contains("invalid installation state"));
        assert!(message.contains(&source.path.display().to_string()));
    }

    #[tokio::test]
    async fn non_native_installation_is_up_to_date_when_manifest_matches_running_version() {
        let dir = tempdir().unwrap();
        let source = InstallationStateFileSource {
            path: dir.path().join("install.json"),
            file_system: SystemUpdateFileSystem,
        };
        let probe = FakeProbe::default();
        let manifest = manifest_fixture().replace("1.3.0", env!("CARGO_PKG_VERSION"));
        let inspection = inspect_installation(
            None,
            false,
            &source,
            &probe,
            Path::new("/work/target/release/lazydb"),
            &ManifestHttp(Ok(manifest)),
        )
        .await;

        assert_eq!(inspection.manager, InstallationManager::Cargo);
        assert_eq!(inspection.status, UpdateStatus::UpToDate);
        assert!(inspection.action.is_none());
    }

    #[test]
    fn update_report_serializes_the_stable_contract_on_one_line() {
        let report = UpdateReport {
            schema: 1,
            manager: InstallationManager::Homebrew,
            channel: UpdateChannel::Stable,
            current_version: Some("1.2.3".into()),
            target_version: None,
            status: "manager_action_required".into(),
            action: Some("brew upgrade yelog/tap/lazydb".into()),
        };
        let output = serde_json::to_string(&report).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert!(!output.contains('\n'));
        assert_eq!(value["schema"], 1);
        assert_eq!(value["manager"], "homebrew");
        assert_eq!(value["channel"], "stable");
        assert_eq!(value["status"], "manager_action_required");
        assert_eq!(value["action"], "brew upgrade yelog/tap/lazydb");
    }

    fn display_report(status: &str) -> UpdateReport {
        UpdateReport {
            schema: 1,
            manager: InstallationManager::Native,
            channel: UpdateChannel::Stable,
            current_version: Some("1.2.3".into()),
            target_version: Some("1.3.0".into()),
            status: status.into(),
            action: None,
        }
    }

    #[test]
    fn update_report_distinguishes_applied_and_preinstalled_updates() {
        let report = UpdateReport {
            current_version: Some("1.3.0".into()),
            ..display_report("updated")
        };
        let applied = format_update_report(
            &report,
            &UpdateDisplayContext {
                previous_version: Some("1.2.3".into()),
                applied_in_this_run: true,
            },
            UpdateOutputStyle::Plain,
        );
        assert!(applied.contains("LazyDB updated successfully"));
        assert!(applied.contains("1.2.3 -> 1.3.0"));

        let installed = format_update_report(
            &report,
            &UpdateDisplayContext::default(),
            UpdateOutputStyle::Plain,
        );
        assert!(installed.contains("LazyDB update is already installed"));
        assert!(!installed.contains("successfully"));
        assert!(!installed.contains("->"));
    }

    #[test]
    fn update_report_renders_readable_status_summaries() {
        let latest = UpdateReport {
            target_version: Some("1.2.3".into()),
            ..display_report("up_to_date")
        };
        let output = format_update_report(
            &latest,
            &UpdateDisplayContext::default(),
            UpdateOutputStyle::Plain,
        );
        assert!(output.contains("already up to date"));
        assert!(output.contains("Version"));
        assert!(!output.contains("action: none"));

        let available = UpdateReport {
            channel: UpdateChannel::Beta,
            ..display_report("update_available")
        };
        let output = format_update_report(
            &available,
            &UpdateDisplayContext::default(),
            UpdateOutputStyle::Plain,
        );
        assert!(output.contains("A LazyDB update is available"));
        assert!(output.contains("lazydb update --channel beta"));
        assert!(!output.contains("successfully"));

        let error = UpdateReport {
            action: Some("connection timed out".into()),
            ..display_report("error")
        };
        let output = format_update_report(
            &error,
            &UpdateDisplayContext::default(),
            UpdateOutputStyle::Plain,
        );
        assert!(output.contains("Could not check for LazyDB updates"));
        assert!(output.contains("connection timed out"));
        assert!(output.contains("[ERROR]"));
    }

    #[test]
    fn output_style_disables_ansi_for_non_tty_or_opt_out() {
        assert_eq!(
            output_style_for(false, false, false),
            UpdateOutputStyle::Plain
        );
        assert_eq!(
            output_style_for(true, true, false),
            UpdateOutputStyle::Plain
        );
        assert_eq!(
            output_style_for(true, false, true),
            UpdateOutputStyle::Plain
        );
        assert_eq!(
            output_style_for(true, false, false),
            UpdateOutputStyle::Styled
        );
    }

    #[test]
    fn update_status_contract_includes_all_planned_states() {
        assert_eq!(
            UPDATE_STATUSES,
            [
                "up_to_date",
                "update_available",
                "updated",
                "manager_action_required",
                "error"
            ]
        );
    }

    #[test]
    fn manager_actions_are_explicit_and_npm_distribution_is_unavailable() {
        assert_eq!(
            manager_action(InstallationManager::Homebrew, UpdateChannel::Stable),
            Some("brew upgrade yelog/tap/lazydb".into())
        );
        assert_eq!(
            manager_action(InstallationManager::Npm, UpdateChannel::Stable),
            Some(
                "official npm distribution is unavailable; use the Pages installer or Homebrew"
                    .into()
            )
        );
        assert_eq!(
            manager_action(InstallationManager::Npm, UpdateChannel::Beta),
            Some(
                "official npm distribution is unavailable; use the Pages installer or Homebrew"
                    .into()
            )
        );
    }

    #[test]
    fn persisted_channel_yields_to_explicit_override() {
        assert_eq!(resolve_channel(None, Some("beta")), UpdateChannel::Beta);
        assert_eq!(
            resolve_channel(Some(UpdateChannel::Stable), Some("beta")),
            UpdateChannel::Stable
        );
        assert_eq!(resolve_channel(None, None), UpdateChannel::Stable);
    }

    fn manifest_fixture() -> String {
        let assets = SUPPORTED_TARGETS
            .iter()
            .map(|target| {
                (
                    (*target).to_owned(),
                    ManifestAsset {
                        url: format!(
                            "https://github.com/yelog/lazydb/releases/download/v1.3.0/lazydb_1.3.0_{target}.{}",
                            if *target == "x86_64-pc-windows-msvc" { "zip" } else { "tar.xz" }
                        ),
                        sha256: "a".repeat(64),
                        requirements: if target.ends_with("-linux-gnu") {
                            Some(AssetRequirements {
                                libc: LibcRequirement { family: "glibc".into(), min_version: "2.34".into() },
                            })
                        } else { None },
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        serde_json::to_string(&ChannelManifest {
            schema: 2,
            product: "lazydb".into(),
            channel: UpdateChannel::Stable,
            version: "1.3.0".into(),
            tag: "v1.3.0".into(),
            prerelease: false,
            published_at: "2026-08-31T00:00:00Z".into(),
            release_url: "https://github.com/yelog/lazydb/releases/tag/v1.3.0".into(),
            assets,
        })
        .unwrap()
    }

    #[test]
    fn validates_manifest_fixture_and_rejects_unapproved_asset_url() {
        assert!(parse_channel_manifest(&manifest_fixture()).is_ok());
        let mut value: serde_json::Value = serde_json::from_str(&manifest_fixture()).unwrap();
        value["assets"][SUPPORTED_TARGETS[0]]["url"] =
            serde_json::Value::String("http://evil.invalid/archive.tar.xz".into());
        assert!(matches!(
            parse_channel_manifest(&value.to_string()),
            Err(ManifestError::Invalid(_))
        ));
    }

    #[test]
    fn glibc_requirements_accept_release_format_and_reject_invalid_versions() {
        assert!(valid_glibc_version("2.17"));
        assert!(valid_glibc_version("2.38"));
        for invalid in ["2", "2.17.1", "v2.17", "2.x", "", "2.17-beta"] {
            assert!(!valid_glibc_version(invalid), "accepted {invalid}");
        }
    }

    #[test]
    fn validates_semver_and_default_downgrade_policy() {
        assert_eq!(
            version_status(Some("1.2.3"), "1.3.0", false),
            "update_available"
        );
        assert_eq!(version_status(Some("1.3.0"), "1.2.3", false), "up_to_date");
        assert_eq!(
            version_status(Some("1.3.0"), "1.2.3", true),
            "update_available"
        );
        assert_eq!(version_status(Some("not-semver"), "1.2.3", false), "error");
    }

    #[test]
    fn archive_paths_require_normal_components_and_expected_root() {
        assert!(archive_entry_path_is_normal("root/lazydb"));
        for path in [
            "root/./lazydb",
            "root//lazydb",
            "root/../lazydb",
            "root\\lazydb",
            "C:\\lazydb",
            "/root/lazydb",
        ] {
            assert!(
                !archive_entry_path_is_normal(path),
                "accepted unsafe path {path}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn release_packager_output_is_accepted_by_updater() {
        let dir = tempdir().unwrap();
        let target = SUPPORTED_TARGETS[0];
        let root = dir.path().join(format!("lazydb_1.3.0_{target}"));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("lazydb"), b"binary").unwrap();
        fs::write(root.join("README.md"), b"documentation").unwrap();
        let archive = dir.path().join("release.tar.xz");
        let status = Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/scripts/release/package-archive.py"
            ))
            .arg(&root)
            .arg(&archive)
            .status()
            .unwrap();
        assert!(status.success());
        let destination = dir.path().join("unpack");
        let manifest = update_manifest("1.3.0", "a".repeat(64));
        extract_archive(&fs::read(archive).unwrap(), &destination, &manifest, target).unwrap();
        assert_eq!(fs::read(destination.join("lazydb")).unwrap(), b"binary");
        assert_eq!(
            fs::read(destination.join("README.md")).unwrap(),
            b"documentation"
        );
    }

    #[test]
    fn archive_directory_markers_and_unsafe_entries() {
        let target = SUPPORTED_TARGETS[0];
        let root = format!("lazydb_1.3.0_{target}");
        let binary = format!("{root}/lazydb");
        let manifest = update_manifest("1.3.0", "a".repeat(64));
        let cases = [
            (root.clone(), binary.clone(), tar::EntryType::Regular, true),
            (
                format!("{root}/"),
                binary.clone(),
                tar::EntryType::Regular,
                true,
            ),
            (
                format!("{root}//"),
                binary.clone(),
                tar::EntryType::Regular,
                false,
            ),
            (
                root.clone(),
                binary.clone(),
                tar::EntryType::Directory,
                false,
            ),
            (root.clone(), binary.clone(), tar::EntryType::Symlink, false),
            (root.clone(), binary.clone(), tar::EntryType::Link, false),
            (
                root.clone(),
                format!("{root}/"),
                tar::EntryType::Directory,
                false,
            ),
            (
                root.clone(),
                format!("{root}/../lazydb"),
                tar::EntryType::Regular,
                false,
            ),
            (
                root.clone(),
                format!("{root}/./lazydb"),
                tar::EntryType::Regular,
                false,
            ),
            (
                root.clone(),
                format!("{root}//lazydb"),
                tar::EntryType::Regular,
                false,
            ),
            (
                root.clone(),
                format!("/{binary}"),
                tar::EntryType::Regular,
                false,
            ),
        ];
        for (directory, name, kind, valid) in cases {
            let mut bytes = Vec::new();
            {
                let mut builder = tar::Builder::new(&mut bytes);
                for (path, entry_type) in [(&directory, tar::EntryType::Directory), (&name, kind)] {
                    let mut header = tar::Header::new_ustar();
                    // Write raw names so the fixture builder cannot normalize unsafe paths.
                    header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
                    header.set_entry_type(entry_type);
                    header.set_size(0);
                    header.set_mode(0o755);
                    header.set_cksum();
                    builder.append(&header, &[][..]).unwrap();
                }
                builder.finish().unwrap();
            }
            let mut encoder = XzEncoder::new(Vec::new(), 6);
            encoder.write_all(&bytes).unwrap();
            let archive = encoder.finish().unwrap();
            let dir = tempdir().unwrap();
            let result = extract_archive(&archive, dir.path(), &manifest, target);
            assert_eq!(result.is_ok(), valid, "{directory}, {name}: {result:?}");
            if valid {
                assert!(dir.path().join("lazydb").is_file());
            } else {
                assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
            }
        }
    }

    #[test]
    fn zip_archive_is_validated_before_extraction() {
        let target = "x86_64-pc-windows-msvc";
        let root = format!("lazydb_1.3.0_{target}");
        let manifest = update_manifest("1.3.0", "a".repeat(64));
        for extra in [None, Some("../escape"), Some("lazydb.exe")] {
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let options = zip::write::SimpleFileOptions::default();
            writer.add_directory(format!("{root}/"), options).unwrap();
            writer
                .start_file(format!("{root}/lazydb.exe"), options)
                .unwrap();
            writer.write_all(b"binary").unwrap();
            if let Some(extra) = extra {
                // A directory with the binary's name also checks type/alias rejection.
                writer
                    .add_directory(format!("{root}/{extra}/"), options)
                    .unwrap();
            }
            let archive = writer.finish().unwrap().into_inner();
            let dir = tempdir().unwrap();
            let result = extract_archive(&archive, dir.path(), &manifest, target);
            assert_eq!(result.is_ok(), extra.is_none(), "{extra:?}: {result:?}");
            if extra.is_none() {
                assert_eq!(fs::read(dir.path().join("lazydb.exe")).unwrap(), b"binary");
            } else {
                assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
            }
        }
    }

    #[test]
    fn archive_without_expected_root_directory_is_rejected() {
        let dir = tempdir().unwrap();
        let target = SUPPORTED_TARGETS[0];
        let manifest = update_manifest("1.3.0", "a".repeat(64));
        let root = format!("lazydb_1.3.0_{target}");
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let contents = b"binary";
            let mut header = tar::Header::new_gnu();
            header.set_path(format!("{root}/lazydb")).unwrap();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, &contents[..]).unwrap();
            builder.finish().unwrap();
        }
        let mut compressed = Vec::new();
        let mut encoder = XzEncoder::new(&mut compressed, 6);
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap();

        assert!(
            extract_archive(&compressed, &dir.path().join("release"), &manifest, target,).is_err()
        );
    }

    #[test]
    fn redirected_urls_must_remain_on_approved_scheme_and_host() {
        assert!(
            validate_response_url(
                "https://github.com/yelog/lazydb/archive.tar.xz",
                &url::Url::parse("https://release-assets.githubusercontent.com/archive").unwrap(),
            )
            .is_ok()
        );
        assert!(
            validate_response_url(
                "https://github.com/yelog/lazydb/archive.tar.xz",
                &url::Url::parse("https://evil.example/archive").unwrap(),
            )
            .is_err()
        );
        assert!(
            validate_response_url(
                "https://github.com/yelog/lazydb/archive.tar.xz",
                &url::Url::parse("http://github.com/archive").unwrap(),
            )
            .is_err()
        );
    }

    struct FakeHttp {
        archive: Vec<u8>,
    }

    #[async_trait]
    impl UpdateHttpClient for FakeHttp {
        async fn get(&self, _url: &str) -> anyhow::Result<String> {
            unreachable!("manifest fetch is not part of apply_native_update tests")
        }

        async fn download(&self, _url: &str) -> anyhow::Result<Vec<u8>> {
            Ok(self.archive.clone())
        }
    }

    fn update_manifest(version: &str, digest: String) -> ChannelManifest {
        let target = SUPPORTED_TARGETS[0];
        let mut assets = BTreeMap::new();
        for supported in SUPPORTED_TARGETS {
            assets.insert(
                supported.to_owned(),
                ManifestAsset {
                    url: format!("https://github.com/yelog/lazydb/releases/download/v{version}/lazydb_{version}_{supported}.tar.xz"),
                    sha256: if supported == target { digest.clone() } else { "a".repeat(64) },
                    requirements: None,
                },
            );
        }
        ChannelManifest {
            schema: 1,
            product: "lazydb".into(),
            channel: UpdateChannel::Stable,
            version: version.into(),
            tag: format!("v{version}"),
            prerelease: false,
            published_at: "2026-08-31T00:00:00Z".into(),
            release_url: format!("https://github.com/yelog/lazydb/releases/tag/v{version}"),
            assets,
        }
    }

    fn archive_for(version: &str, target: &str, output: &Path) -> Vec<u8> {
        archive_for_reported(version, version, target, output)
    }

    fn archive_for_reported(
        version: &str,
        reported_version: &str,
        target: &str,
        output: &Path,
    ) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let root = format!("lazydb_{version}_{target}");
            let mut directory = tar::Header::new_gnu();
            directory.set_path(format!("{root}/")).unwrap();
            directory.set_entry_type(tar::EntryType::Directory);
            directory.set_size(0);
            directory.set_mode(0o755);
            directory.set_cksum();
            builder.append(&directory, &[][..]).unwrap();
            let script = format!(
                "#!/bin/sh\nprintf '{{\"version\":\"{reported_version}\",\"cli_api\":1}}\\n'\n"
            );
            let mut header = tar::Header::new_gnu();
            header.set_path(format!("{root}/lazydb")).unwrap();
            header.set_size(script.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, script.as_bytes()).unwrap();
            builder.finish().unwrap();
        }
        let mut compressed = Vec::new();
        let mut encoder = XzEncoder::new(&mut compressed, 6);
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap();
        File::create(output)
            .unwrap()
            .write_all(&compressed)
            .unwrap();
        compressed
    }

    #[tokio::test]
    async fn applies_verified_release_and_preserves_previous_release() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let old = data.join("releases/1.2.3");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("lazydb"), b"old").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&old, data.join("current")).unwrap();
        let bin = dir.path().join("bin/lazydb");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(data.join("current/lazydb"), &bin).unwrap();
        let state = InstallationState {
            schema: 1,
            product: "lazydb".into(),
            manager: InstallationManager::Native,
            channel: "stable".into(),
            version: "1.2.3".into(),
            target: SUPPORTED_TARGETS[0].into(),
            path: bin,
            bin_dir: None,
            installed_at: None,
            shell_profiles: Vec::new(),
        };
        let archive_path = dir.path().join("archive.tar.xz");
        let archive = archive_for("1.3.0", SUPPORTED_TARGETS[0], &archive_path);
        let digest = format!("{:x}", Sha256::digest(&archive));
        let manifest = update_manifest("1.3.0", digest);
        let progress = std::sync::Mutex::new(Vec::new());
        let version = apply_native_update_with_progress(
            &state,
            SUPPORTED_TARGETS[0],
            &manifest,
            &FakeHttp { archive },
            &|value| progress.lock().unwrap().push(value),
        )
        .await
        .unwrap();
        let progress = progress.into_inner().unwrap();
        assert_eq!(
            progress.iter().map(|value| value.stage).collect::<Vec<_>>(),
            vec![
                UpdateStage::Preparing,
                UpdateStage::Downloading,
                UpdateStage::VerifyingChecksum,
                UpdateStage::Extracting,
                UpdateStage::VerifyingInstallation,
                UpdateStage::Activating,
                UpdateStage::Finishing,
            ]
        );
        assert!(progress[1].downloaded_bytes > 0);
        assert_eq!(progress[1].total_bytes, Some(progress[1].downloaded_bytes));
        assert_eq!(version, "1.3.0");
        assert!(old.exists());
        assert_eq!(
            fs::read_link(data.join("current")).unwrap(),
            PathBuf::from("releases/1.3.0")
        );
        assert!(data.join("releases/1.3.0/lazydb").exists());
        let state_path = data.join("install.json");
        publish_native_state(
            &state,
            &data,
            &data.join("releases/1.3.0"),
            &manifest,
            SUPPORTED_TARGETS[0],
        )
        .unwrap();
        assert_eq!(
            read_installation_state(&state_path).unwrap().version,
            "1.3.0"
        );
        let persisted = read_installation_state(&state_path).unwrap();
        assert_eq!(persisted.channel, "stable");
        assert_eq!(persisted.target, SUPPORTED_TARGETS[0]);
        assert_eq!(persisted.path, state.path);
    }

    #[tokio::test]
    async fn checksum_failure_leaves_current_release_unchanged() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let old = data.join("releases/1.2.3");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("marker"), b"old").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&old, data.join("current")).unwrap();
        let bin = dir.path().join("bin/lazydb");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(data.join("current/lazydb"), &bin).unwrap();
        let state = InstallationState {
            schema: 1,
            product: "lazydb".into(),
            manager: InstallationManager::Native,
            channel: "stable".into(),
            version: "1.2.3".into(),
            target: SUPPORTED_TARGETS[0].into(),
            path: bin,
            bin_dir: None,
            installed_at: None,
            shell_profiles: Vec::new(),
        };
        let archive = b"not an archive".to_vec();
        let manifest = update_manifest("1.3.0", "0".repeat(64));
        assert!(
            apply_native_update(
                &state,
                SUPPORTED_TARGETS[0],
                &manifest,
                &FakeHttp { archive }
            )
            .await
            .is_err()
        );
        assert_eq!(fs::read_link(data.join("current")).unwrap(), old);
        assert_eq!(fs::read(old.join("marker")).unwrap(), b"old");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn download_failure_leaves_current_release_and_install_state_unchanged() {
        use std::os::unix::fs::symlink;

        struct FailedDownload;

        #[async_trait]
        impl UpdateHttpClient for FailedDownload {
            async fn get(&self, _url: &str) -> anyhow::Result<String> {
                unreachable!("manifest is supplied directly")
            }

            async fn download(&self, _url: &str) -> anyhow::Result<Vec<u8>> {
                Err(anyhow::anyhow!("fixture download failed"))
            }
        }

        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let old = data.join("releases/1.2.3");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("marker"), b"old release").unwrap();
        symlink(Path::new("releases/1.2.3"), data.join("current")).unwrap();
        let bin = dir.path().join("bin/lazydb");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        symlink(data.join("current/lazydb"), &bin).unwrap();
        let state = InstallationState {
            schema: 1,
            product: "lazydb".into(),
            manager: InstallationManager::Native,
            channel: "stable".into(),
            version: "1.2.3".into(),
            target: SUPPORTED_TARGETS[0].into(),
            path: bin,
            bin_dir: None,
            installed_at: None,
            shell_profiles: Vec::new(),
        };
        let manifest = update_manifest("1.3.0", "a".repeat(64));
        let state_path = data.join("install.json");
        let original_state = serde_json::to_vec(&state).unwrap();
        fs::write(&state_path, &original_state).unwrap();

        let result =
            apply_native_update(&state, SUPPORTED_TARGETS[0], &manifest, &FailedDownload).await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("fixture download failed")
        );
        assert_eq!(
            fs::read_link(data.join("current")).unwrap(),
            Path::new("releases/1.2.3")
        );
        assert_eq!(fs::read(old.join("marker")).unwrap(), b"old release");
        assert_eq!(fs::read(state_path).unwrap(), original_state);
        assert!(!data.join("releases/1.3.0").exists());
    }

    #[tokio::test]
    async fn staged_version_failure_leaves_current_release_unchanged() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let old = data.join("releases/1.2.3");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("marker"), b"old").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&old, data.join("current")).unwrap();
        let bin = dir.path().join("bin/lazydb");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(data.join("current/lazydb"), &bin).unwrap();
        let state = InstallationState {
            schema: 1,
            product: "lazydb".into(),
            manager: InstallationManager::Native,
            channel: "stable".into(),
            version: "1.2.3".into(),
            target: SUPPORTED_TARGETS[0].into(),
            path: bin,
            bin_dir: None,
            installed_at: None,
            shell_profiles: Vec::new(),
        };
        let archive_path = dir.path().join("archive.tar.xz");
        let archive = archive_for_reported("1.3.0", "1.3.1", SUPPORTED_TARGETS[0], &archive_path);
        let manifest = update_manifest("1.3.0", format!("{:x}", Sha256::digest(&archive)));
        let result = apply_native_update(
            &state,
            SUPPORTED_TARGETS[0],
            &manifest,
            &FakeHttp { archive },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(fs::read_link(data.join("current")).unwrap(), old);
        assert_eq!(fs::read(old.join("marker")).unwrap(), b"old");
        assert!(!data.join("releases/1.3.0").exists());
    }
}
