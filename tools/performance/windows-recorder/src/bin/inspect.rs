use anyhow::Result;
use serde_json::json;
use windows::Win32::{Media::Audio::*,System::Com::*};

fn main() -> Result<()> {
    unsafe { CoInitializeEx(None,COINIT_MULTITHREADED).ok()?; }
    let enumerator: IMMDeviceEnumerator=unsafe { CoCreateInstance(&MMDeviceEnumerator,None,CLSCTX_ALL)? };
    let device=unsafe { enumerator.GetDefaultAudioEndpoint(eRender,eConsole)? };
    let format=WAVEFORMATEX { wFormatTag:3,nChannels:2,nSamplesPerSec:48000,nAvgBytesPerSec:384000,
        nBlockAlign:8,wBitsPerSample:32,cbSize:0 };
    for ms in [40,30,20,10] {
        let client: IAudioClient=unsafe { device.Activate(CLSCTX_ALL,None)? };
        unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED,AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            ms*10000,0,&format,None)?; }
        let frames=unsafe { client.GetBufferSize()? };
        let latency=unsafe { client.GetStreamLatency()? };
        let mut default=0; let mut minimum=0;
        unsafe { client.GetDevicePeriod(Some(&mut default),Some(&mut minimum))?; }
        println!("{}",json!({"requested_ms":ms,"actual_buffer_frames":frames,
            "actual_buffer_ms":frames as f64/48.0,"stream_latency_ms":latency as f64/10000.0,
            "default_period_ms":default as f64/10000.0,"minimum_period_ms":minimum as f64/10000.0,
            "started":false}));
    }
    Ok(())
}
