pub fn send(title: &str, body: &str, play_sound: bool) {
    let mut notification = notify_rust::Notification::new();
    notification.summary(title).body(body);
    if play_sound {
        notification.sound_name("default");
    }

    if let Err(error) = notification.show() {
        tracing::warn!(%error, "failed to send desktop notification");
    }
}
