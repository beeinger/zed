//! File-backed credentials for the headless daemon (no session keyring).

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use credentials_provider::CredentialsProvider;
use futures::FutureExt as _;
use gpui::{App, AsyncApp, Task};
use language_model::LanguageModelRegistry;

pub fn install_daemon_credentials(cx: &mut App) {
    let path = paths::remote_server_state_dir().join("credentials.json");
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        log::error!("create daemon credentials dir {}: {error}", parent.display());
    }
    let provider = Arc::new(FileCredentialsProvider { path });
    cx.set_global(zed_credentials_provider::ZedCredentialsProvider(provider));
}

struct FileCredentialsProvider {
    path: PathBuf,
}

impl FileCredentialsProvider {
    fn load(&self) -> Result<HashMap<String, (String, Vec<u8>)>> {
        let json = std::fs::read(&self.path)?;
        Ok(serde_json::from_slice(&json)?)
    }

    fn save(&self, credentials: &HashMap<String, (String, Vec<u8>)>) -> Result<()> {
        let json = serde_json::to_vec(credentials)?;
        write_private(&self.path, &json)
    }
}

fn write_private(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

impl CredentialsProvider for FileCredentialsProvider {
    fn read_credentials<'a>(
        &'a self,
        url: &'a str,
        _cx: &'a AsyncApp,
    ) -> Pin<Box<dyn Future<Output = Result<Option<(String, Vec<u8>)>>> + 'a>> {
        async move { Ok(self.load().unwrap_or_default().get(url).cloned()) }.boxed_local()
    }

    fn write_credentials<'a>(
        &'a self,
        url: &'a str,
        username: &'a str,
        password: &'a [u8],
        _cx: &'a AsyncApp,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        async move {
            let mut credentials = self.load().unwrap_or_default();
            credentials.insert(url.to_string(), (username.to_string(), password.to_vec()));
            self.save(&credentials)
        }
        .boxed_local()
    }

    fn delete_credentials<'a>(
        &'a self,
        url: &'a str,
        _cx: &'a AsyncApp,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        async move {
            let mut credentials = self.load().unwrap_or_default();
            credentials.remove(url);
            self.save(&credentials)
        }
        .boxed_local()
    }
}

pub fn store_api_key(
    url: String,
    username: String,
    api_key: Option<String>,
    cx: &mut App,
) -> Task<Result<()>> {
    let provider = zed_credentials_provider::global(cx);
    cx.spawn(async move |cx| {
        if let Some(key) = &api_key {
            provider
                .write_credentials(&url, &username, key.as_bytes(), cx)
                .await
                .with_context(|| format!("write daemon credentials for {url}"))?;
        } else {
            provider
                .delete_credentials(&url, cx)
                .await
                .with_context(|| format!("delete daemon credentials for {url}"))?;
        }
        cx.update(|cx| {
            let registry = LanguageModelRegistry::global(cx);
            for provider in registry.read(cx).providers() {
                provider.authenticate(cx).detach();
            }
        });
        anyhow::Ok(())
    })
}
