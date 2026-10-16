use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tracing::{info, warn};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    WAVEFORMATEX,
};
use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{Error as WinError, HRESULT, IUnknown, Interface, PROPVARIANT, w};

use crate::codec::OpusEncoder;
use crate::{
    AudioCapture, AudioFrame, BackendState, BackendStatus, CHANNELS, CaptureSink,
    FRAME_SAMPLES_INTERLEAVED, SAMPLE_RATE, StreamKind,
};

const OPUS_BITRATE_BPS: i32 = 128_000;
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_BACKOFF: Duration = Duration::from_secs(1);
const IDLE_POLL: Duration = Duration::from_millis(20);
const CAPTURE_POLL: Duration = Duration::from_millis(5);
const CAPTURE_ACCUMULATOR_CAP: usize = FRAME_SAMPLES_INTERLEAVED * 4;
const MAX_PACKETS_PER_POLL: usize = 8;
const MAX_PACKET_FRAMES: u32 = SAMPLE_RATE;

fn packet_drain_has_budget(processed: usize) -> bool {
    processed < MAX_PACKETS_PER_POLL
}

fn append_bounded(pending: &mut VecDeque<f32>, packet: Vec<f32>, capacity: usize) {
    if capacity == 0 {
        pending.clear();
        return;
    }
    if packet.len() >= capacity {
        let skip = packet.len() - capacity;
        pending.clear();
        pending.extend(packet.into_iter().skip(skip));
        return;
    }
    let overflow = pending
        .len()
        .saturating_add(packet.len())
        .saturating_sub(capacity);
    pending.drain(..overflow);
    pending.extend(packet);
}

#[repr(C)]
struct RawBlob {
    size: u32,
    data: *mut u8,
}

#[repr(C, align(8))]
struct RawBlobPropVariant {
    vt: u16,
    reserved1: u16,
    reserved2: u16,
    reserved3: u16,
    blob: RawBlob,
}

#[cfg(test)]
const fn raw_propvariant_alignment_for_pointer(pointer_alignment: usize) -> usize {
    if pointer_alignment > 8 {
        pointer_alignment
    } else {
        8
    }
}

enum ActivationOutcome {
    AudioClient(usize),
    Error(i32),
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivationTimeoutPolicy {
    RetryNewOperation,
    KeepWaitingForCurrentOperation,
}

#[cfg(test)]
fn activation_timeout_policy() -> ActivationTimeoutPolicy {
    ActivationTimeoutPolicy::KeepWaitingForCurrentOperation
}

#[derive(Default)]
struct ActivationSlot {
    outcome: Option<ActivationOutcome>,
}

fn publish_activation_outcome(slot: &mut ActivationSlot, outcome: ActivationOutcome) {
    slot.outcome = Some(outcome);
}

fn release_activation_outcome(outcome: ActivationOutcome) {
    if let ActivationOutcome::AudioClient(raw) = outcome {
        drop(unsafe { IAudioClient::from_raw(raw as *mut c_void) });
    }
}

fn completed_activation_outcome(slot: &mut ActivationSlot) -> Result<ActivationOutcome> {
    slot.outcome
        .take()
        .ok_or_else(|| anyhow!("activation completed without an outcome"))
}

type SharedActivation = Arc<(Mutex<ActivationSlot>, Condvar)>;

#[windows::core::implement(IActivateAudioInterfaceCompletionHandler)]
struct CompletionHandler {
    shared: SharedActivation,
}

impl CompletionHandler {
    fn publish(&self, outcome: ActivationOutcome) {
        let (lock, ready) = &*self.shared;
        let Ok(mut slot) = lock.lock() else {
            release_activation_outcome(outcome);
            return;
        };
        publish_activation_outcome(&mut slot, outcome);
        ready.notify_one();
    }
}

#[allow(non_snake_case)]
impl IActivateAudioInterfaceCompletionHandler_Impl for CompletionHandler {
    fn ActivateCompleted(
        &self,
        operation: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let Some(operation) = operation else {
            self.publish(ActivationOutcome::Error(E_POINTER.0));
            return Ok(());
        };

        let mut activation_result = HRESULT(0);
        let mut activated: Option<IUnknown> = None;
        let outcome = unsafe {
            match operation.GetActivateResult(&mut activation_result, &mut activated) {
                Err(error) => ActivationOutcome::Error(error.code().0),
                Ok(()) if activation_result.is_err() => {
                    ActivationOutcome::Error(activation_result.0)
                }
                Ok(()) => match activated {
                    None => ActivationOutcome::Error(E_POINTER.0),
                    Some(unknown) => match unknown.cast::<IAudioClient>() {
                        Ok(client) => ActivationOutcome::AudioClient(client.into_raw() as usize),
                        Err(error) => ActivationOutcome::Error(error.code().0),
                    },
                },
            }
        };
        self.publish(outcome);
        Ok(())
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize_mta() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .context("initialize process-loopback COM apartment")?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn process_loopback_activation(target_pid: u32) -> AUDIOCLIENT_ACTIVATION_PARAMS {
    AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: target_pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
            },
        },
    }
}

fn activate_process_loopback(target_pid: u32, status: &BackendStatus) -> Result<IAudioClient> {
    let shared: SharedActivation =
        Arc::new((Mutex::new(ActivationSlot::default()), Condvar::new()));
    let handler: IActivateAudioInterfaceCompletionHandler = CompletionHandler {
        shared: Arc::clone(&shared),
    }
    .into();
    let activation = process_loopback_activation(target_pid);
    let params = RawBlobPropVariant {
        vt: VT_BLOB.0,
        reserved1: 0,
        reserved2: 0,
        reserved3: 0,
        blob: RawBlob {
            size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            data: (&activation as *const AUDIOCLIENT_ACTIVATION_PARAMS)
                .cast_mut()
                .cast(),
        },
    };
    if std::mem::size_of::<RawBlobPropVariant>() != std::mem::size_of::<PROPVARIANT>()
        || std::mem::align_of::<RawBlobPropVariant>() != std::mem::align_of::<PROPVARIANT>()
    {
        bail!("internal process-loopback PROPVARIANT ABI mismatch");
    }

    let params_ptr = (&params as *const RawBlobPropVariant).cast::<PROPVARIANT>();
    let _operation = unsafe {
        ActivateAudioInterfaceAsync(
            w!("VAD\\Process_Loopback"),
            &IAudioClient::IID,
            Some(params_ptr),
            &handler,
        )
    }
    .context("activate VAD\\Process_Loopback")?;

    let (lock, ready) = &*shared;
    let mut slot = lock
        .lock()
        .map_err(|_| anyhow!("process-loopback activation mutex poisoned"))?;
    let mut timeout_reported = false;
    while slot.outcome.is_none() {
        let (next, timeout) = ready
            .wait_timeout_while(slot, ACTIVATION_TIMEOUT, |slot| slot.outcome.is_none())
            .map_err(|_| anyhow!("process-loopback activation wait mutex poisoned"))?;
        slot = next;
        if timeout.timed_out() && slot.outcome.is_none() && !timeout_reported {
            status.set(BackendState::Degraded);
            warn!(
                "process-loopback activation exceeded 10 seconds; keeping the single operation in flight"
            );
            timeout_reported = true;
        }
    }

    match completed_activation_outcome(&mut slot)? {
        ActivationOutcome::AudioClient(raw) => {
            Ok(unsafe { IAudioClient::from_raw(raw as *mut c_void) })
        }
        ActivationOutcome::Error(code) => {
            Err(WinError::from_hresult(HRESULT(code))).context(format!(
                "process-loopback activation returned HRESULT 0x{:08X}",
                code as u32
            ))
        }
    }
}

fn preferred_format() -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT as u16,
        nChannels: CHANNELS,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * u32::from(CHANNELS) * 4,
        nBlockAlign: CHANNELS * 4,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

unsafe fn copy_packet_or_silence(
    data: *const f32,
    frames: u32,
    channels: u16,
    silent: bool,
) -> Result<Vec<f32>> {
    if channels != CHANNELS {
        bail!("process-loopback packet channel count changed to {channels}");
    }
    if frames > MAX_PACKET_FRAMES {
        bail!("process-loopback packet exceeded one second: {frames} frames");
    }
    let sample_count = (frames as usize)
        .checked_mul(channels as usize)
        .ok_or_else(|| anyhow!("process-loopback packet sample count overflow"))?;
    if silent {
        return Ok(vec![0.0; sample_count]);
    }
    if data.is_null() {
        bail!("process-loopback returned a null packet without SILENT");
    }
    // SAFETY: GetBuffer owns a packet with `frames * channels` f32 samples until ReleaseBuffer.
    let pcm = unsafe { std::slice::from_raw_parts(data, sample_count) }.to_vec();
    if pcm.iter().any(|sample| !sample.is_finite()) {
        bail!("process-loopback packet contains non-finite PCM");
    }
    Ok(pcm)
}

fn encode_process_frame(encoder: &mut OpusEncoder, pcm: &[f32]) -> Result<Option<Vec<u8>>> {
    if pcm.iter().all(|sample| *sample == 0.0) {
        return Ok(None);
    }
    encoder.encode(pcm).map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureExit {
    DemandEnded,
    SinkClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerAction {
    Exit,
    Wait,
    Capture,
}

fn worker_action(active: bool, closed: bool) -> WorkerAction {
    if closed {
        WorkerAction::Exit
    } else if active {
        WorkerAction::Capture
    } else {
        WorkerAction::Wait
    }
}

fn run_demand_session(
    sink: &CaptureSink,
    seq: &mut u32,
    status: &BackendStatus,
) -> Result<CaptureExit> {
    let client = activate_process_loopback(std::process::id(), status)?;
    if !sink.is_active() {
        return Ok(CaptureExit::DemandEnded);
    }

    let format = preferred_format();
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            0,
            0,
            &format,
            None,
        )
    }
    .context("initialize 48 kHz stereo f32 process-loopback")?;
    if !sink.is_active() {
        return Ok(CaptureExit::DemandEnded);
    }
    let capture: IAudioCaptureClient =
        unsafe { client.GetService() }.context("get process-loopback IAudioCaptureClient")?;
    let mut encoder = OpusEncoder::new(OPUS_BITRATE_BPS, false)?;
    let mut pending = VecDeque::<f32>::with_capacity(FRAME_SAMPLES_INTERLEAVED * 2);

    unsafe { client.Start() }.context("start process-loopback capture")?;
    status.set(BackendState::Active);
    info!("Windows process-loopback capture started");
    let capture_result = (|| -> Result<CaptureExit> {
        loop {
            if sink.is_closed() {
                return Ok(CaptureExit::SinkClosed);
            }
            if !sink.is_active() {
                return Ok(CaptureExit::DemandEnded);
            }

            let mut processed = 0;
            while packet_drain_has_budget(processed) {
                let packet_frames = unsafe { capture.GetNextPacketSize() }
                    .context("query process-loopback packet size")?;
                if packet_frames == 0 {
                    break;
                }

                let mut data = null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }
                    .context("get process-loopback packet")?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let packet = unsafe {
                    copy_packet_or_silence(data.cast_const().cast(), frames, CHANNELS, silent)
                };
                let release = unsafe { capture.ReleaseBuffer(frames) }
                    .context("release process-loopback packet");
                append_bounded(&mut pending, packet?, CAPTURE_ACCUMULATOR_CAP);
                release?;
                processed += 1;
            }

            while pending.len() >= FRAME_SAMPLES_INTERLEAVED {
                let pcm = pending
                    .drain(..FRAME_SAMPLES_INTERLEAVED)
                    .collect::<Vec<_>>();
                let Some(opus) = encode_process_frame(&mut encoder, &pcm)? else {
                    continue;
                };
                let frame = AudioFrame {
                    stream: StreamKind::SysOut,
                    seq: *seq,
                    opus,
                };
                *seq = seq.wrapping_add(1);
                if !sink.send_lossy(frame) {
                    return Ok(CaptureExit::SinkClosed);
                }
            }
            thread::sleep(CAPTURE_POLL);
        }
    })();
    let stop_result = unsafe { client.Stop() }.context("stop process-loopback capture");
    match (capture_result, stop_result) {
        (Ok(exit), Ok(())) => Ok(exit),
        (Err(capture_error), Ok(())) => Err(capture_error),
        (Ok(_), Err(stop_error)) => Err(stop_error),
        (Err(capture_error), Err(stop_error)) => {
            warn!(error = %stop_error, "process-loopback stop also failed");
            Err(capture_error)
        }
    }
}

fn run_capture_worker(sink: CaptureSink, status: BackendStatus) -> Result<()> {
    let _com = ComApartment::initialize_mta()?;
    let mut seq = 0u32;
    loop {
        match worker_action(sink.is_active(), sink.is_closed()) {
            WorkerAction::Exit => {
                status.set(BackendState::Stopped);
                return Ok(());
            }
            WorkerAction::Wait => {
                status.set(BackendState::Idle);
                thread::sleep(IDLE_POLL);
                continue;
            }
            WorkerAction::Capture => status.set(BackendState::Starting),
        }

        match run_demand_session(&sink, &mut seq, &status) {
            Ok(CaptureExit::SinkClosed) => {
                status.set(BackendState::Stopped);
                return Ok(());
            }
            Ok(CaptureExit::DemandEnded) => {
                status.set(BackendState::Idle);
                info!("Windows process-loopback released after demand ended");
            }
            Err(error) => {
                status.set(BackendState::Degraded);
                warn!(%error, "Windows process-loopback failed closed; retrying while demanded");
                let deadline = std::time::Instant::now() + RETRY_BACKOFF;
                while sink.is_active() && !sink.is_closed() && std::time::Instant::now() < deadline
                {
                    thread::sleep(IDLE_POLL);
                }
            }
        }
    }
}

pub struct ProcessLoopbackCapture {
    started: bool,
    status: BackendStatus,
}

impl ProcessLoopbackCapture {
    pub fn new() -> Self {
        Self {
            started: false,
            status: BackendStatus::new(BackendState::Idle),
        }
    }
}

fn start_once<T>(started: &mut bool, spawn: impl FnOnce() -> Result<T>) -> Result<T> {
    if *started {
        bail!("Windows process-loopback capture already started");
    }
    let value = spawn()?;
    *started = true;
    Ok(value)
}

impl AudioCapture for ProcessLoopbackCapture {
    fn backend_name(&self) -> &'static str {
        "windows-process-loopback"
    }

    fn backend_status(&self) -> BackendStatus {
        self.status.clone()
    }

    fn start(&mut self, sink: CaptureSink) -> Result<()> {
        let status = self.status.clone();
        let _worker = start_once(&mut self.started, move || {
            thread::Builder::new()
                .name("audio-process-loopback".into())
                .spawn(move || {
                    if let Err(error) = run_capture_worker(sink, status.clone()) {
                        status.set(BackendState::Degraded);
                        warn!(%error, "Windows process-loopback worker stopped");
                    }
                })
                .context("spawn Windows process-loopback worker")
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Media::Audio::{
        AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    };

    #[test]
    fn activation_excludes_the_mineshare_process_tree() {
        let activation = process_loopback_activation(42);
        assert_eq!(
            activation.ActivationType,
            AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK
        );
        let loopback = unsafe { activation.Anonymous.ProcessLoopbackParams };
        assert_eq!(loopback.TargetProcessId, 42);
        assert_eq!(
            loopback.ProcessLoopbackMode,
            PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE
        );
    }

    #[test]
    fn activation_completion_handler_is_agile() {
        let shared: SharedActivation =
            Arc::new((Mutex::new(ActivationSlot::default()), Condvar::new()));
        let handler: IActivateAudioInterfaceCompletionHandler = CompletionHandler { shared }.into();

        assert!(
            handler
                .cast::<windows::Win32::System::Com::IAgileObject>()
                .is_ok()
        );
    }

    #[test]
    fn raw_propvariant_preserves_windows_alignment_on_32_bit_targets() {
        assert_eq!(raw_propvariant_alignment_for_pointer(4), 8);
    }

    #[test]
    fn failed_worker_spawn_does_not_poison_start_state() {
        let mut started = false;
        let result: Result<()> = start_once(&mut started, || Err(anyhow!("spawn failed")));

        assert!(result.is_err());
        assert!(!started);
    }

    #[test]
    fn missing_activation_outcome_fails_closed_without_panicking() {
        assert!(completed_activation_outcome(&mut ActivationSlot::default()).is_err());
    }

    #[test]
    fn activation_timeout_keeps_exactly_one_operation_in_flight() {
        assert_eq!(
            activation_timeout_policy(),
            ActivationTimeoutPolicy::KeepWaitingForCurrentOperation
        );
        assert_ne!(
            activation_timeout_policy(),
            ActivationTimeoutPolicy::RetryNewOperation
        );
    }

    #[test]
    fn silent_packet_becomes_zero_without_reading_undefined_payload() {
        let pcm = unsafe { copy_packet_or_silence(std::ptr::null(), 480, 2, true) }.unwrap();
        assert_eq!(pcm, vec![0.0; 960]);
    }

    #[test]
    fn non_finite_process_audio_fails_closed() {
        let packet = [0.0, f32::NAN];
        let result = unsafe { copy_packet_or_silence(packet.as_ptr(), 1, 2, false) };
        assert!(result.is_err());
    }

    #[test]
    fn oversized_process_packet_fails_before_allocation_or_pointer_read() {
        let result = unsafe {
            copy_packet_or_silence(std::ptr::null(), MAX_PACKET_FRAMES + 1, CHANNELS, true)
        };
        assert!(result.is_err());
    }

    #[test]
    fn exact_process_silence_never_reaches_opus() {
        let mut encoder = OpusEncoder::new(OPUS_BITRATE_BPS, false).unwrap();
        let result =
            encode_process_frame(&mut encoder, &vec![0.0; crate::FRAME_SAMPLES_INTERLEAVED])
                .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn capture_accumulator_keeps_only_the_latest_bounded_pcm() {
        let mut pending = VecDeque::from(vec![1.0, 2.0]);
        append_bounded(&mut pending, vec![3.0, 4.0, 5.0], 3);
        assert_eq!(pending, VecDeque::from(vec![3.0, 4.0, 5.0]));
    }

    #[test]
    fn packet_drain_yields_after_a_bounded_batch() {
        assert!(packet_drain_has_budget(MAX_PACKETS_PER_POLL - 1));
        assert!(!packet_drain_has_budget(MAX_PACKETS_PER_POLL));
    }

    #[test]
    fn closed_sink_wins_over_active_demand() {
        assert_eq!(worker_action(true, true), WorkerAction::Exit);
    }
}
