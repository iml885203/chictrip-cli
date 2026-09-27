use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const CONFIG_DIR_ENV: &str = "CHICTRIP_CONFIG_DIR";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Credentials {
    pub access_token: String,
    pub refresh_token: String,
    pub member_id: String,
}

pub fn load() -> Result<Credentials> {
    if let Some(credentials) = from_environment()? {
        return Ok(credentials);
    }
    let path = credentials_path()?;
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("not logged in; run `chictrip login` ({})", path.display()))?;
    let credentials =
        serde_json::from_str(&raw).context("stored ChicTrip credentials are invalid")?;
    validate(&credentials)?;
    Ok(credentials)
}

pub fn save(credentials: &Credentials) -> Result<()> {
    validate(credentials)?;
    let path = credentials_path()?;
    let parent = path.parent().context("credential path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;

    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("could not write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(serde_json::to_string(credentials)?.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

pub fn delete() -> Result<()> {
    let path = credentials_path()?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("could not remove {}", path.display())),
    }
}

pub fn validate(credentials: &Credentials) -> Result<()> {
    if credentials.access_token.trim().is_empty()
        || credentials.refresh_token.trim().is_empty()
        || credentials.member_id.trim().is_empty()
    {
        bail!("access token, refresh token, and member ID are all required");
    }
    Ok(())
}

fn credentials_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("credentials.json"))
}

pub(crate) fn config_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(CONFIG_DIR_ENV) {
        return Ok(PathBuf::from(path));
    }
    dirs::config_dir()
        .map(|path| path.join("chictrip"))
        .context("could not determine the user configuration directory")
}

fn from_environment() -> Result<Option<Credentials>> {
    let access_token = std::env::var("CHICTRIP_ACCESS_TOKEN").ok();
    let refresh_token = std::env::var("CHICTRIP_REFRESH_TOKEN").ok();
    let member_id = std::env::var("CHICTRIP_MEMBER_ID").ok();
    if access_token.is_none() && refresh_token.is_none() && member_id.is_none() {
        return Ok(None);
    }
    let credentials = Credentials {
        access_token: access_token
            .context("CHICTRIP_ACCESS_TOKEN is required when using environment credentials")?,
        refresh_token: refresh_token
            .context("CHICTRIP_REFRESH_TOKEN is required when using environment credentials")?,
        member_id: member_id
            .context("CHICTRIP_MEMBER_ID is required when using environment credentials")?,
    };
    validate(&credentials)?;
    Ok(Some(credentials))
}
