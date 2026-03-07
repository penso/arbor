pub fn send(title: &str, body: &str) {
    if let Err(error) = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .show()
    {
        tracing::warn!(%error, "failed to send desktop notification");
    }
}
