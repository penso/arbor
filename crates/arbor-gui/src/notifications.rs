pub trait NotificationService {
    fn send(&self, title: &str, body: &str, play_sound: bool);
}

pub struct DesktopNotificationService;

impl NotificationService for DesktopNotificationService {
    fn send(&self, title: &str, body: &str, play_sound: bool) {
        let mut notification = notify_rust::Notification::new();
        notification.summary(title).body(body);
        if play_sound {
            notification.sound_name("default");
        }

        if let Err(error) = notification.show() {
            tracing::warn!(%error, "failed to send desktop notification");
        }
    }
}

pub fn default_notification_service() -> Box<dyn NotificationService> {
    Box::new(DesktopNotificationService)
}
