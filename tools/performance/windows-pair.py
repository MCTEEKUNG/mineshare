"""Measure real Windows -> MineShare -> Windows digital audio with bounded clocks.

Requires the existing paired applications and source SysOut -> receiver playback.
Runs only synthetic audio; never changes audio routing, volumes or power settings.
"""
import argparse
import base64
import ctypes
import json
from pathlib import Path
import subprocess
import sys
import time
import wave
import uuid

import numpy as np
from scipy import signal

RATE = 48000
MARKERS = np.arange(2, 34, .5)

def now_ns():
    value = ctypes.c_ulonglong()
    ctypes.windll.kernel32.GetSystemTimePreciseAsFileTime(ctypes.byref(value))
    return (value.value - 116444736000000000) * 100

def templates():
    result = []
    for i in range(len(MARKERS)):
        noise = np.random.default_rng(104729+i).normal(size=1920)
        spec = np.fft.rfft(noise)
        frequency = np.fft.rfftfreq(len(noise), 1/RATE)
        spec[(frequency < 1000) | (frequency > 4500)] = 0
        code = np.fft.irfft(spec, n=len(noise)) * signal.windows.tukey(len(noise), .3)
        result.append(code / np.max(np.abs(code)) * .06)
    return result

def generate(path, continuous):
    audio = np.zeros(36 * RATE, dtype=np.float32)
    if continuous:
        t = np.arange(len(audio)) / RATE
        spectrum = np.fft.rfft(np.random.default_rng(912401).normal(size=len(audio)))
        frequency = np.fft.rfftfreq(len(audio), 1/RATE)
        spectrum[(frequency < 1100) | (frequency > 4300)] = 0
        bed = np.fft.irfft(spectrum, n=len(audio)); bed *= .003 / np.std(bed)
        audio = (bed + .006*np.sin(2*np.pi*523*t) + .006*np.sin(2*np.pi*787*t)).astype(np.float32)
    for when, code in zip(MARKERS, templates()):
        index = round(when * RATE); audio[index:index+len(code)] = code
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(2); wav.setsampwidth(2); wav.setframerate(RATE)
        wav.writeframes((np.repeat(audio[:, None], 2, axis=1) * 32767).astype('<i2').tobytes())

class Recording:
    def __init__(self, folder, name):
        self.samples = np.fromfile(folder / f'{name}.f32', dtype='<f4').reshape(-1, 2).mean(axis=1)
        self.packets = [json.loads(line) for line in (folder / f'{name}.jsonl').read_text().splitlines()]
        self.offset = np.array([p['offset'] for p in self.packets])
        self.times = np.array([p['t_ns'] for p in self.packets], dtype=np.float64)
        self.base = float(np.median(self.times - self.offset*1e9/RATE))

    def at(self, index):
        packet = self.packets[max(0, int(np.searchsorted(self.offset, index, side='right'))-1)]
        return packet['t_ns'] + (index-packet['offset'])*1e9/RATE, packet

    def find(self, code, expected_ns, radius_s):
        expected = int(np.interp(expected_ns, self.times, self.offset))
        lo = max(0, expected-int(radius_s*RATE))
        hi = min(len(self.samples), expected+int(radius_s*RATE)+len(code))
        segment = self.samples[lo:hi].astype(np.float64)
        if len(segment) < len(code): return None
        corr = signal.correlate(segment, code, mode='valid', method='fft')
        energy = np.concatenate(([0.0], np.cumsum(segment**2, dtype=np.float64)))
        power = np.maximum(energy[len(code):]-energy[:-len(code)], 0)
        audible = power > len(code)*1e-10
        score = np.zeros_like(corr)
        score[audible] = corr[audible] / np.sqrt(power[audible]*np.sum(code**2))
        assert np.max(np.abs(score)) <= 1.01, 'Invalid normalized correlation'
        index = lo + int(np.argmax(score))
        stamp, packet = self.at(index)
        return {'index': index, 't_ns': stamp, 'correlation': float(np.max(score)), 'packet': packet}

def calibrate(process):
    samples = []
    for i in range(70):
        t0 = now_ns()
        process.stdin.write(json.dumps({'id': i})+'\n'); process.stdin.flush()
        reply = json.loads(process.stdout.readline()); t3 = now_ns()
        assert reply['id'] == i
        samples.append({'t0': t0, 't1': reply['rx'], 't2': reply['tx'], 't3': t3})
    lower = max(p['t2']-p['t3'] for p in samples)
    upper = min(p['t1']-p['t0'] for p in samples)
    assert lower <= upper, 'Clock changed during calibration'
    return {'at_ns': int(np.median([p['t0'] for p in samples])), 'offset_ns': (lower+upper)//2,
            'uncertainty_ns': (upper-lower)//2, 'samples': samples}

def analyze(folder, number):
    ref, out = Recording(folder, 'tee'), Recording(folder, 'windows')
    clock = json.loads((folder/'clocks.json').read_text())
    before, after = clock['before'], clock['after']
    source_is_remote = clock.get('source_is_remote', True)
    started = json.loads((folder/'stimulus-start.json').read_text())['utc_ns']
    rows = []
    for i, (when, code) in enumerate(zip(MARKERS, templates())):
        source = ref.find(code, started+when*1e9, 1.2)
        row = {'run': number, 'marker': i, 'valid': False}
        row['source_correlation'] = source['correlation'] if source else None
        row['source_valid_timestamp'] = source['packet']['valid'] if source else False
        if source is not None and source['correlation'] >= .45 and source['packet']['valid']:
            approximate_local_source = source['t_ns']-(before['offset_ns'] if source_is_remote else 0)
            fraction = np.clip((approximate_local_source-before['at_ns']) / (after['at_ns']-before['at_ns']), 0, 1)
            offset = before['offset_ns']+(after['offset_ns']-before['offset_ns'])*fraction
            source_local = source['t_ns']-(offset if source_is_remote else 0)
            expected = source_local+(0 if source_is_remote else offset)
            received = out.find(code, expected+250e6, .75)
            row['output_correlation'] = received['correlation'] if received else None
            if received is not None and received['correlation'] >= .32 and received['packet']['valid']:
                null = out.find(code, received['t_ns']+250e6, .08)
                assert null is None or null['correlation'] < .32, 'False-match control failed'
                row.update(valid=True, source_index=source['index'], output_index=received['index'],
                           latency_ms=(received['t_ns']-(0 if source_is_remote else offset)-source_local)/1e6,
                           source_correlation=source['correlation'], output_correlation=received['correlation'])
        rows.append(row)
    (folder/'measurements.json').write_text(json.dumps(rows, indent=2))
    values = [r['latency_ms'] for r in rows if r['valid']]
    report = {'run': number, 'attempted': 64, 'matched': len(values),
              'clock_bound_ms': max(before['uncertainty_ns'], after['uncertainty_ns'])/1e6}
    if len(values) == 64:
        report.update(median_ms=float(np.median(values)), p95_ms=float(np.percentile(values,95)), max_ms=max(values))
    (folder/'analysis.json').write_text(json.dumps(report, indent=2))
    print(json.dumps(report), flush=True)
    return rows, report

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--host', required=True); parser.add_argument('--user', required=True)
    parser.add_argument('--identity', required=True); parser.add_argument('--remote-root', required=True)
    parser.add_argument('--remote-python', default='C:/Python314/python.exe')
    parser.add_argument('--recorder', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--runs', type=int, default=3); parser.add_argument('--continuous', action='store_true')
    parser.add_argument('--analyze-only', action='store_true')
    parser.add_argument('--reverse', action='store_true', help='Local source -> remote Windows output; same verified SSH connection')
    args = parser.parse_args(); args.output.mkdir(parents=True, exist_ok=True)
    ssh = ['-o','BatchMode=yes','-o','StrictHostKeyChecking=yes','-o','ConnectTimeout=8','-i',args.identity]
    peer = args.user+'@'+args.host
    def ps(script):
        encoded = base64.b64encode(("$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; "+script).encode('utf-16-le')).decode()
        return subprocess.run(['ssh',*ssh,peer,'powershell','-NoProfile','-EncodedCommand',encoded],check=True,capture_output=True,text=True,timeout=30)
    session_id = uuid.uuid4().hex
    task = 'MineShareRoundAAudioProbe'
    if not args.analyze_only:
        (args.output/'session.json').write_text(json.dumps(vars(args), default=str, indent=2))
        generate(args.output/'markers.wav', args.continuous)
        subprocess.run(['scp',*ssh,str(args.output/'markers.wav'),f'{peer}:{args.remote_root}/markers.wav'],check=True)
        clock = subprocess.Popen(['ssh',*ssh,peer,args.remote_python,args.remote_root+'/windows-pair-agent.py','clock'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            for number in range(1,args.runs+1):
                folder = args.output/f'run-{number}'; folder.mkdir(exist_ok=False)
                remote = args.remote_root+'/'+args.output.name+f'-{session_id}-run-{number}'
                with (folder/'recorder.log').open('w') as log:
                    local_command = ([sys.executable,str(Path(__file__).with_name('windows-pair-agent.py').resolve()),'capture',str(folder.resolve()),str(args.recorder.resolve()),str((args.output/'markers.wav').resolve())]
                                     if args.reverse else [str(args.recorder.resolve()),str(folder.resolve()),'46'])
                    receiver = subprocess.Popen(local_command,stdout=log,stderr=subprocess.STDOUT,creationflags=subprocess.CREATE_NO_WINDOW)
                    try:
                        action = 'record' if args.reverse else 'capture'
                        command = f'"{args.remote_root}/windows-pair-agent.py" {action} "{remote}" "{args.remote_root}/recorder.exe" "{args.remote_root}/markers.wav"'
                        ps(f"$action=New-ScheduledTaskAction -Execute '{args.remote_python}' -Argument '{command}'; $principal=New-ScheduledTaskPrincipal -UserId '{args.user}' -LogonType Interactive -RunLevel Limited; $settings=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 2); Register-ScheduledTask -TaskName '{task}' -Action $action -Principal $principal -Settings $settings -Force | Out-Null; Start-ScheduledTask -TaskName '{task}'")
                        before = calibrate(clock)
                        print(json.dumps({'run':number,'stage':'capturing','clock_bound_ms':before['uncertainty_ns']/1e6}),flush=True)
                        assert receiver.wait(timeout=55) == 0, 'Receiver recorder failed'
                        after = calibrate(clock)
                        (folder/'clocks.json').write_text(json.dumps({'before':before,'after':after,'source_is_remote':not args.reverse},indent=2))
                        deadline = time.monotonic()+20
                        while ps(f"Test-Path '{remote}/completed.json'").stdout.strip() != 'True':
                            assert time.monotonic() < deadline, 'Source stimulus/recorder failed'
                            time.sleep(1)
                    finally:
                        if receiver.poll() is None: receiver.terminate(); receiver.wait(timeout=5)
                if args.reverse:
                    for original,renamed in [('windows.f32','tee.f32'),('windows.jsonl','tee.jsonl'),('windows-summary.json','tee-summary.json')]:
                        (folder/original).rename(folder/renamed)
                    transfers=[('windows.f32','windows.f32'),('windows.jsonl','windows.jsonl'),('windows-summary.json','windows-summary.json')]
                else:
                    transfers=[('windows.f32','tee.f32'),('windows.jsonl','tee.jsonl'),('windows-summary.json','tee-summary.json'),('stimulus-start.json','stimulus-start.json')]
                for source,dest in transfers:
                    subprocess.run(['scp',*ssh,f'{peer}:{remote}/{source}',str(folder/dest)],check=True)
                analyze(folder,number)
        finally:
            clock.terminate(); clock.wait(timeout=5)
            ps(f"Stop-ScheduledTask -TaskName '{task}' -ErrorAction SilentlyContinue; Unregister-ScheduledTask -TaskName '{task}' -Confirm:$false -ErrorAction SilentlyContinue")
    rows, reports = [], []
    for number in range(1,args.runs+1):
        r,p = analyze(args.output/f'run-{number}',number); rows.extend(r); reports.append(p)
    summary = {'attempted':args.runs*64,'matched':sum(r['valid'] for r in rows),'runs':reports}
    if summary['matched'] == summary['attempted']:
        values=[r['latency_ms'] for r in rows]
        summary.update(median_ms=float(np.median(values)),p95_ms=float(np.percentile(values,95)),max_ms=max(values))
    (args.output/'measurements.json').write_text(json.dumps(rows,indent=2))
    (args.output/'statistics.json').write_text(json.dumps(summary,indent=2))

if __name__ == '__main__': main()
