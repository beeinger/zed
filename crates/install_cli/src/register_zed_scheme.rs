use paths::URL_SCHEME;
use gpui::{AsyncApp, actions};

actions!(
    cli,
    [
        /// Registers the Peekado URL scheme handler.
        RegisterZedScheme
    ]
);

pub async fn register_zed_scheme(cx: &AsyncApp) -> anyhow::Result<()> {
    // FORK:branding — register peekado:// so we do not steal Zed's zed:// handler.
    cx.update(|cx| cx.register_url_scheme(URL_SCHEME)).await
}
