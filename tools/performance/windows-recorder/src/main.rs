use std::{fs::File, io::{BufWriter, Write}, path::PathBuf, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
use anyhow::{Context, Result};
use serde_json::json;
use windows::{core::w, Win32::{Foundation::HANDLE, Media::Audio::*, System::{Com::*, Performance::*, Threading::*}}};

struct AudioTask(HANDLE);
impl Drop for AudioTask {
    fn drop(&mut self) { let _ = unsafe { AvRevertMmThreadCharacteristics(self.0) }; }
}

fn utc_ns() -> i128 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as i128 }

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let folder = PathBuf::from(args.get(1).context("output folder")?);
    let seconds: u64 = args.get(2).context("duration")?.parse()?;
    std::fs::create_dir_all(&folder)?;
    let mut task_index = 0;
    let _audio_task = AudioTask(unsafe { AvSetMmThreadCharacteristicsW(w!("Audio"), &mut task_index)? });
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()?; }
    let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole)? };
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None)? };
    let format = WAVEFORMATEX { wFormatTag: 3, nChannels: 2, nSamplesPerSec: 48_000,
        nAvgBytesPerSec: 384_000, nBlockAlign: 8, wBitsPerSample: 32, cbSize: 0 };
    unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED,
        // This is the observer's capture queue, not the application's render
        // buffer. GetBuffer's endpoint/QPC timestamps remain the timing source.
        AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, 2_000_000, 0, &format, None)?; }
    let capture_buffer_frames = unsafe { client.GetBufferSize()? };
    let capture: IAudioCaptureClient = unsafe { client.GetService()? };
    let mut frequency = 0;
    unsafe { QueryPerformanceFrequency(&mut frequency)?; }
    let before = utc_ns();
    let mut counter = 0;
    unsafe { QueryPerformanceCounter(&mut counter)?; }
    let after = utc_ns();
    let qpc_base_ns = counter as i128 * 1_000_000_000 / frequency as i128;
    let utc_base_ns = (before + after) / 2;
    let mut raw = BufWriter::with_capacity(1024*1024,File::create(folder.join("windows.f32"))?);
    let mut index = BufWriter::with_capacity(256*1024,File::create(folder.join("windows.jsonl"))?);
    let mut offset: u64 = 0;
    let mut invalid = 0;
    let mut discontinuities = 0;
    let zeroes = vec![0u8; 48_000 * 8];
    unsafe { client.Start()?; }
    std::fs::write(folder.join("windows-ready.json"), json!({"rate":48000,"channels":2,
        "qpc_calibration_bracket_ns":after-before,"started_ns":utc_ns(),
        "capture_buffer_frames":capture_buffer_frames,"mmcss_audio":true}).to_string())?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        while unsafe { capture.GetNextPacketSize()? } > 0 {
            let mut data = std::ptr::null_mut();
            let mut frames = 0;
            let mut flags = 0;
            let mut position = 0;
            let mut qpc = 0;
            unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, Some(&mut position), Some(&mut qpc))?; }
            let bytes = frames as usize * 8;
            anyhow::ensure!(bytes <= zeroes.len(), "oversized loopback packet");
            let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
            let timestamp_valid = flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 == 0;
            if !timestamp_valid { invalid += 1; }
            if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 { discontinuities += 1; }
            if silent { raw.write_all(&zeroes[..bytes])?; }
            else {
                anyhow::ensure!(!data.is_null(), "non-silent packet without data");
                raw.write_all(unsafe { std::slice::from_raw_parts(data, bytes) })?;
            }
            unsafe { capture.ReleaseBuffer(frames)?; }
            let sample_ns = utc_base_ns + qpc as i128 * 100 - qpc_base_ns;
            writeln!(index, "{}", json!({"offset":offset,"frames":frames,"t_ns":sample_ns,
                "device_position":position,"flags":flags,"valid":timestamp_valid}))?;
            offset += frames as u64;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    unsafe { client.Stop()?; }
    raw.flush()?; index.flush()?;
    std::fs::write(folder.join("windows-summary.json"), json!({"rate":48000,"channels":2,
        "frames":offset,"timestamp_error_packets":invalid,"discontinuities":discontinuities,
        "duration_s":started.elapsed().as_secs_f64()}).to_string())?;
    Ok(())
}
