//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! Counts Windows notifications via `UserNotificationListener` (the same API YASB uses), running on a background thread
//! and reporting the count to the UI whenever it changes.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
use windows::UI::Notifications::NotificationKinds;

use crate::engine::app::N_NOTIFICATIONS;
use crate::engine::win::post_notify;

static STARTED: AtomicBool = AtomicBool::new(false);
/// Generation counter: `stop()` increments it so the old background thread exits by itself and does not push notifications into a later run.
static EPOCH: AtomicU64 = AtomicU64::new(0);

/// Stops counting notifications (the background thread exits on its next iteration) and allows it to be started again.
pub fn stop() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
    STARTED.store(false, Ordering::SeqCst);
}

pub fn ensure_started(needed: bool) {
    if !needed || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let epoch = EPOCH.load(Ordering::SeqCst);
    std::thread::spawn(move || unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        let listener = match UserNotificationListener::Current() {
            Ok(l) => l,
            Err(e) => {
                crate::engine::log::warn(&format!("Không dùng được UserNotificationListener: {e}"));
                return;
            }
        };
        let status = listener.RequestAccessAsync().and_then(|op| op.join());
        if !matches!(status, Ok(s) if s == UserNotificationListenerAccessStatus::Allowed) {
            crate::engine::log::warn("Windows chưa cho phép đọc thông báo (Cài đặt → Quyền riêng tư → Thông báo).");
            return;
        }
        let mut last: i64 = -1;
        while EPOCH.load(Ordering::SeqCst) == epoch {
            let count = listener
                .GetNotificationsAsync(NotificationKinds::Toast)
                .and_then(|op| op.join())
                .and_then(|v| v.Size())
                .map(|n| n as i64)
                .unwrap_or(last.max(0));
            if EPOCH.load(Ordering::SeqCst) != epoch {
                break;
            }
            if count != last {
                last = count;
                post_notify(N_NOTIFICATIONS, count as isize);
            }
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }
    });
}
