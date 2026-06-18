//! Optional completion notifications. When `MAGNETBOX_NOTIFY_URL` is set,
//! MagnetBox POSTs a message to it whenever a torrent or direct download
//! finishes — so you get pinged instead of babysitting the dashboard.
//!
//! Works with **Discord** webhooks (JSON `{content}`), **ntfy** (plain text),
//! and most generic webhooks (plain-text body).

/// POST `text` to the webhook. Best-effort — failures are logged, never fatal.
pub async fn send(client: &reqwest::Client, url: &str, text: &str) {
    let res = if url.contains("discord.com") || url.contains("discordapp.com") {
        client
            .post(url)
            .json(&serde_json::json!({ "content": text }))
            .send()
            .await
    } else {
        // ntfy reads the body as the message and `X-Title` as the title; generic
        // webhooks just receive the text.
        client
            .post(url)
            .header("X-Title", "MagnetBox")
            .body(text.to_owned())
            .send()
            .await
    };
    match res {
        Ok(r) if !r.status().is_success() => {
            tracing::warn!(status = %r.status(), "notification webhook returned an error status");
        }
        Err(e) => tracing::warn!(error = %e, "notification webhook request failed"),
        _ => {}
    }
}
