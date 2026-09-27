use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::auth::{self, Credentials};

const LOGIN_URL: &str = "https://www.chictrip.com.tw/?action=login";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DebugTarget {
    url: String,
    web_socket_debugger_url: Option<String>,
}

#[derive(Deserialize)]
struct BrowserCredentials {
    access_token: Option<String>,
    refresh_token: Option<String>,
    member_id: Option<String>,
}

pub async fn login() -> Result<Credentials> {
    let chrome = find_chrome()
        .context("Google Chrome was not found; install Chrome or use `chictrip login --manual`")?;
    let port = available_port()?;
    let profile = profile_dir()?;
    fs::create_dir_all(&profile).context("could not create the ChicTrip browser profile")?;
    let mut browser = BrowserProcess::spawn(&chrome, port, &profile)?;
    eprintln!("Complete the official ChicTrip phone/OTP login in the Chrome window.");
    eprintln!("The CLI never receives your phone number, OTP, or password.");

    let deadline = Instant::now() + LOGIN_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(status) = browser.child.try_wait()? {
            bail!("Chrome closed before login completed ({status})");
        }
        if let Some(credentials) = read_credentials(port).await? {
            close_browser(port).await;
            return Ok(credentials);
        }
        sleep(Duration::from_millis(750)).await;
    }
    bail!("login timed out after 5 minutes; run `chictrip login` to try again")
}

async fn close_browser(port: u16) {
    let Ok(response) = reqwest::get(format!("http://127.0.0.1:{port}/json/version")).await else {
        return;
    };
    let Ok(version) = response.json::<Value>().await else {
        return;
    };
    let Some(socket) = version.get("webSocketDebuggerUrl").and_then(Value::as_str) else {
        return;
    };
    let Ok((mut stream, _)) = connect_async(socket).await else {
        return;
    };
    let _ = stream
        .send(Message::Text(
            json!({"id": 1, "method": "Browser.close"})
                .to_string()
                .into(),
        ))
        .await;
    sleep(Duration::from_millis(500)).await;
}

struct BrowserProcess {
    child: Child,
}

impl BrowserProcess {
    fn spawn(chrome: &Path, port: u16, profile: &Path) -> Result<Self> {
        let child = Command::new(chrome)
            .arg(format!("--remote-debugging-port={port}"))
            .arg("--remote-debugging-address=127.0.0.1")
            .arg(format!("--remote-allow-origins=http://127.0.0.1:{port}"))
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--new-window")
            .arg(LOGIN_URL)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("could not launch {}", chrome.display()))?;
        Ok(Self { child })
    }
}

pub fn delete_profile() -> Result<()> {
    let path = profile_dir()?;
    match fs::remove_dir_all(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("could not remove browser profile {}", path.display())),
    }
}

fn profile_dir() -> Result<PathBuf> {
    Ok(auth::config_dir()?.join("browser-profile"))
}

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn read_credentials(port: u16) -> Result<Option<Credentials>> {
    let targets: Vec<DebugTarget> =
        match reqwest::get(format!("http://127.0.0.1:{port}/json/list")).await {
            Ok(response) if response.status().is_success() => response.json().await?,
            _ => return Ok(None),
        };
    let Some(socket) = targets
        .into_iter()
        .find(|target| target.url.starts_with("https://www.chictrip.com.tw/"))
        .and_then(|target| target.web_socket_debugger_url)
    else {
        return Ok(None);
    };
    let (mut stream, _) = connect_async(socket).await?;
    stream
        .send(Message::Text(
            json!({
                "id": 1,
                "method": "Runtime.evaluate",
                "params": {
                    "expression": "JSON.stringify({access_token:localStorage.getItem('accessToken'),refresh_token:localStorage.getItem('refreshToken'),member_id:localStorage.getItem('memberId')})",
                    "returnByValue": true
                }
            })
            .to_string()
            .into(),
        ))
        .await?;
    while let Some(message) = stream.next().await {
        let message = message?;
        let Message::Text(text) = message else {
            continue;
        };
        let response: Value = serde_json::from_str(&text)?;
        if response.get("id") != Some(&json!(1)) {
            continue;
        }
        let Some(raw) = response
            .pointer("/result/result/value")
            .and_then(Value::as_str)
        else {
            return Ok(None);
        };
        let credentials: BrowserCredentials = serde_json::from_str(raw)?;
        let (Some(access_token), Some(refresh_token), Some(member_id)) = (
            credentials.access_token,
            credentials.refresh_token,
            credentials.member_id,
        ) else {
            return Ok(None);
        };
        if access_token.is_empty() || refresh_token.is_empty() || member_id.is_empty() {
            return Ok(None);
        }
        return Ok(Some(Credentials {
            access_token,
            refresh_token,
            member_id,
        }));
    }
    Ok(None)
}

fn available_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

fn find_chrome() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CHICTRIP_CHROME") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    chrome_candidates().into_iter().find(|path| path.is_file())
}

#[cfg(target_os = "macos")]
fn chrome_candidates() -> Vec<PathBuf> {
    vec![
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into(),
        "/Applications/Google Chrome Beta.app/Contents/MacOS/Google Chrome Beta".into(),
        "/Applications/Chromium.app/Contents/MacOS/Chromium".into(),
    ]
}

#[cfg(target_os = "windows")]
fn chrome_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for root in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
        if let Some(root) = std::env::var_os(root) {
            paths.push(PathBuf::from(root).join("Google/Chrome/Application/chrome.exe"));
        }
    }
    paths
}

#[cfg(target_os = "linux")]
fn chrome_candidates() -> Vec<PathBuf> {
    [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}
