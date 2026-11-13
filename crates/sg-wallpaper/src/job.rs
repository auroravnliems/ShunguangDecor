// Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).
//! Job Object: every child process (mpv, wallpaper apps) is closed along with ShunguangDecor when it exits
//! or crashes, so no orphaned process is ever left running in the background.

use std::sync::OnceLock;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};


struct Job(HANDLE);
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

fn job() -> Option<&'static Job> {
    static JOB: OnceLock<Option<Job>> = OnceLock::new();
    JOB.get_or_init(|| unsafe {
        let h = CreateJobObjectW(None, None).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            h,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .is_err()
        {
            logln!("Không cấu hình được Job Object");
        }
        Some(Job(h))
    })
    .as_ref()
}

pub fn adopt(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    if let Some(j) = job() {
        unsafe {
            if let Err(e) = AssignProcessToJobObject(j.0, HANDLE(child.as_raw_handle())) {
                logln!("Không gắn tiến trình {} vào job: {e}", child.id());
            }
        }
    }
}
