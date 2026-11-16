//! Thread-affine MMCSS registration for the playback decoder and WASAPI callback.
//! CPAL 0.15's priority helper passes a thread ID as a HANDLE; that fails with
//! ERROR_INVALID_HANDLE. Register the actual calling thread through MMCSS instead.
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{
    AVRT_PRIORITY_HIGH, AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
    AvSetMmThreadPriority,
};
use windows::core::w;

static REGISTERED: AtomicUsize = AtomicUsize::new(0);
static PRO_AUDIO: AtomicUsize = AtomicUsize::new(0);
static HIGH_PRIORITY: AtomicUsize = AtomicUsize::new(0);

struct AudioTask {
    handle: Option<HANDLE>,
    pro_audio: bool,
    high_priority: bool,
}

impl AudioTask {
    fn register() -> Self {
        let mut index = 0;
        let preferred = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut index) }.ok();
        let pro_audio = preferred.is_some();
        let handle = preferred.or_else(|| {
            index = 0;
            unsafe { AvSetMmThreadCharacteristicsW(w!("Audio"), &mut index) }.ok()
        });
        let high_priority = handle.is_some_and(|handle| unsafe {
            AvSetMmThreadPriority(handle, AVRT_PRIORITY_HIGH).is_ok()
        });
        if handle.is_some() {
            REGISTERED.fetch_add(1, Ordering::Relaxed);
        }
        if pro_audio {
            PRO_AUDIO.fetch_add(1, Ordering::Relaxed);
        }
        if high_priority {
            HIGH_PRIORITY.fetch_add(1, Ordering::Relaxed);
        }
        Self {
            handle,
            pro_audio,
            high_priority,
        }
    }
}

impl Drop for AudioTask {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // TLS is destroyed on its owning thread, as required by AVRT.
            let _ = unsafe { AvRevertMmThreadCharacteristics(handle) };
            REGISTERED.fetch_sub(1, Ordering::Relaxed);
            if self.pro_audio {
                PRO_AUDIO.fetch_sub(1, Ordering::Relaxed);
            }
            if self.high_priority {
                HIGH_PRIORITY.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

thread_local! {
    static AUDIO_TASK: AudioTask = AudioTask::register();
}

pub(crate) fn enter() -> bool {
    // One registration per thread. No allocation, lock or log on subsequent calls.
    // Failure leaves ordinary scheduling available instead of disabling audio.
    AUDIO_TASK.with(|task| task.handle.is_some())
}

pub(crate) fn registered_threads() -> (usize, usize, usize) {
    (
        REGISTERED.load(Ordering::Relaxed),
        PRO_AUDIO.load(Ordering::Relaxed),
        HIGH_PRIORITY.load(Ordering::Relaxed),
    )
}
