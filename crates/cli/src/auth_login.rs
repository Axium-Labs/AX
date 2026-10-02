//! Browser login composed with AX's existing provider credential/catalog stores.
use crate::args::{AuthCommand, WorkBuddyLoginRegion};
use anyhow::{Result, anyhow};
use model::workbuddy::WorkBuddyRegion;

pub(crate) async fn login_workbuddy(
    report: impl Fn(&str),
    discover: bool,
    region: WorkBuddyRegion,
) -> Result<()> {
    let login = model::workbuddy::BrowserLogin::begin_for_region(region).await?;
    report(&format!(
        "Open or copy this URL to sign in to {} (10 minute timeout):\n{}",
        region.label(),
        login.auth_url
    ));
    let credential = login.wait().await?;
    model::AuthStorage::new(crate::bootstrap::ax_auth_path())
        .store_oauth(region.provider_id(), credential)?;
    let cache = crate::bootstrap::ax_models_dir().join(format!("{}.json", region.provider_id()));
    match std::fs::remove_file(cache) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if !discover {
        return Ok(());
    }
    // This refresh is awaited: exiting the CLI must not lose the model cache.
    let provider = model::workbuddy::WorkBuddyProvider::for_region(
        model::AuthStorage::new(crate::bootstrap::ax_auth_path()),
        "catalog-only".into(),
        128_000,
        region,
    )?;
    let catalog = model::ModelRegistry::new(crate::bootstrap::ax_models_dir())
        .discover(&provider)
        .await;
    if let Some(warning) = catalog.warning {
        report(&format!(
            "{} credential saved; model discovery: {warning}",
            region.label()
        ));
    } else {
        report(&format!(
            "{} login successful; {} available models. Select one with /model.",
            region.label(),
            catalog.models.len()
        ));
    }
    Ok(())
}
fn selected_region(
    provider: &str,
    option: Option<WorkBuddyLoginRegion>,
) -> Result<WorkBuddyRegion> {
    let default = WorkBuddyRegion::from_provider_id(provider)
        .ok_or_else(|| anyhow!("browser CLI login is not enabled for {provider}; use /login"))?;
    let selected = match option {
        Some(WorkBuddyLoginRegion::Intl) => WorkBuddyRegion::International,
        Some(WorkBuddyLoginRegion::Cn) => WorkBuddyRegion::China,
        None => default,
    };
    if provider == "workbuddy-cn" && selected != WorkBuddyRegion::China {
        return Err(anyhow!(
            "workbuddy-cn requires --region cn; use workbuddy for international login"
        ));
    }
    Ok(selected)
}
pub(crate) async fn run(command: &AuthCommand) -> Result<()> {
    match command {
        AuthCommand::Login { provider, region } => {
            if provider == "openai-codex" {
                if region.is_some() {
                    return Err(anyhow!("--region only applies to WorkBuddy"));
                }
                return tokio::select! {
                    result=login_codex()=>result,
                    result=tokio::signal::ctrl_c()=>{result?;Err(anyhow!("Codex login cancelled"))}
                };
            }
            let selected = selected_region(provider, *region)?;
            tokio::select! {
                result=login_workbuddy(|text|eprintln!("{text}"), true, selected)=>result,
                result=tokio::signal::ctrl_c()=> {result?; Err(anyhow!("{} login cancelled", selected.label()))}
            }
        }
    }
}

async fn login_codex() -> Result<()> {
    let auth = model::begin().await?;
    eprintln!("{}", auth.prompt());
    let tokens = auth.poll_and_exchange().await?;
    model::AuthStorage::new(crate::bootstrap::ax_auth_path()).store_oauth(
        "openai-codex",
        model::OAuthCredential {
            access: tokens.access_token,
            refresh: tokens.refresh_token,
            expires: tokens.expires_at,
            account_id: tokens.account_id,
        },
    )?;
    eprintln!("Codex credentials saved; discovering models…");
    if let Some(catalog) = crate::tui::catalog_refresh::refresh_provider(
        &std::path::PathBuf::new(),
        None,
        "openai-codex",
    )
    .await
        && let Some(warning) = catalog.warning
    {
        eprintln!("Model discovery: {warning}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    #[test]
    fn browser_auth_cli_and_model_provider_use_native_workbuddy() {
        let cli = crate::args::Cli::try_parse_from(["ax", "auth", "login", "workbuddy"]).unwrap();
        assert!(
            matches!(cli.command, Some(crate::args::Command::Auth { command: crate::args::AuthCommand::Login {provider, ..} }) if provider == "workbuddy")
        );
        let cli = crate::args::Cli::try_parse_from([
            "ax",
            "--provider",
            "workbuddy",
            "--model",
            "account-model",
            "run",
            "hi",
        ])
        .unwrap();
        assert_eq!(cli.provider.as_deref(), Some("workbuddy"));
        assert!(model::provider_supported("workbuddy"));
        assert_eq!(
            model::provider("workbuddy").unwrap().auth,
            model::ProviderAuthKind::ExternalOAuth
        );
        assert!(model::provider_supports_oauth("workbuddy"));
    }
    #[test]
    fn login_region_is_explicit_and_conflicts_are_rejected() {
        use super::*;
        for (provider, flag, expected) in [
            ("workbuddy", None, WorkBuddyRegion::International),
            ("workbuddy-cn", None, WorkBuddyRegion::China),
            ("workbuddy", Some("cn"), WorkBuddyRegion::China),
            ("workbuddy", Some("intl"), WorkBuddyRegion::International),
        ] {
            let mut args = vec!["ax", "auth", "login", provider];
            if let Some(flag) = flag {
                args.extend(["--region", flag]);
            }
            let cli = crate::args::Cli::try_parse_from(args).unwrap();
            let Some(crate::args::Command::Auth {
                command: AuthCommand::Login { provider, region },
            }) = cli.command
            else {
                panic!("missing login command")
            };
            assert_eq!(selected_region(&provider, region).unwrap(), expected);
        }
        assert!(selected_region("workbuddy-cn", Some(WorkBuddyLoginRegion::Intl)).is_err());
        assert!(
            crate::args::Cli::try_parse_from([
                "ax",
                "auth",
                "login",
                "workbuddy",
                "--region",
                "invalid"
            ])
            .is_err()
        );
        assert!(model::provider_supported("workbuddy-cn"));
        assert_eq!(
            model::provider("workbuddy-cn").unwrap().name,
            "WorkBuddy China"
        );
    }
}
