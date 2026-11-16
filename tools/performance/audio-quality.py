"""Check continuous reference/output waveforms without treating silence as quality."""
import importlib.util
import json
from pathlib import Path
import sys
import numpy as np
from scipy import signal

tool=Path(__file__).resolve().parent/'windows-pair.py'
spec=importlib.util.spec_from_file_location('measurement',tool)
m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
RATE=48000

def gaps(samples, threshold, minimum=96):
    mask=np.max(np.abs(samples),axis=1)<threshold
    edges=np.diff(np.concatenate(([False],mask,[False])).astype(np.int8))
    starts=np.flatnonzero(edges==1); ends=np.flatnonzero(edges==-1)
    return [(int(a),int(b)) for a,b in zip(starts,ends) if b-a>=minimum]

def analyze(folder):
    rows=json.loads((folder/'measurements.json').read_text())
    rows=[r for r in rows if r['valid']]
    assert len(rows)==64, 'Missing timing anchors: cannot certify quality'
    ref=m.Recording(folder,'tee'); out=m.Recording(folder,'windows')
    source_idx=np.array([r['source_index'] for r in rows]); output_idx=np.array([r['output_index'] for r in rows])
    source_raw=np.fromfile(folder/'tee.f32',dtype='<f4').reshape(-1,2)
    output_raw=np.fromfile(folder/'windows.f32',dtype='<f4').reshape(-1,2)
    a,b=source_idx[0],source_idx[-1]+1920
    c,d=output_idx[0],output_idx[-1]+1920
    source_rms=float(np.sqrt(np.mean(source_raw[a:b].astype(np.float64)**2)))
    output_rms=float(np.sqrt(np.mean(output_raw[c:d].astype(np.float64)**2)))
    assert source_rms>1e-5 and output_rms>1e-5, 'Test audio absent'
    source_gaps=gaps(source_raw[a:b],source_rms*.01)
    output_gaps=gaps(output_raw[c:d],output_rms*.01)
    assert not source_gaps, 'Reference itself has silence: continuous-test result invalid'
    active_packets=[p for p in out.packets if p['offset']+p['frames']>c and p['offset']<d]
    source_packets=[p for p in ref.packets if p['offset']+p['frames']>a and p['offset']<b]
    source_errors=sum(bool(p['flags']&1) or not p['valid'] for p in source_packets)
    source_errors+=sum(current['device_position']!=previous['device_position']+previous['frames']
                       for previous,current in zip(source_packets,source_packets[1:]))
    capture_discontinuities=sum(bool(p['flags']&1) for p in active_packets)
    timestamp_errors=sum(not p['valid'] for p in active_packets)
    position_gaps=sum(current['device_position']!=previous['device_position']+previous['frames']
                      for previous,current in zip(active_packets,active_packets[1:]))
    # A valid/contiguous capture can still expose a late audio-engine tick.
    # Report this separately; it does not erase gaps in the actual waveform.
    endpoint_stalls=[]
    for previous,current in zip(active_packets,active_packets[1:]):
        if not (previous['valid'] and current['valid']):
            continue
        expected=(current['device_position']-previous['device_position'])/RATE*1000
        extra=(current['t_ns']-previous['t_ns'])/1e6-expected
        if extra>=5:
            endpoint_stalls.append({'at_ns':current['t_ns'],'extra_ms':extra})
    windows=[]
    length=1920; radius=720
    for start in range(int(a),int(b)-length,480):
        template=ref.samples[start:start+length].astype(np.float64)
        rms=float(np.sqrt(np.mean(template**2)))
        assert rms>1e-5, 'Reference window unexpectedly silent'
        expected=int(np.interp(start,source_idx,output_idx))
        lo=max(0,expected-radius); hi=min(len(out.samples),expected+length+radius)
        section=out.samples[lo:hi].astype(np.float64)
        corr=signal.correlate(section,template,mode='valid',method='fft')
        cumulative=np.concatenate(([0.0],np.cumsum(section**2)))
        power=np.maximum(cumulative[length:]-cumulative[:-length],0)
        scores=np.zeros_like(corr); audible=power>length*1e-10
        scores[audible]=corr[audible]/np.sqrt(power[audible]*np.sum(template**2))
        k=int(np.argmax(scores)); score=float(np.clip(scores[k],-1,1))
        gain=float(corr[k]/np.sum(template**2))
        windows.append({'source_index':start,'output_index':lo+k,'correlation':score,'gain':gain})
    correlations=np.array([w['correlation'] for w in windows])
    report={'windows':len(windows),'window_ms':40,'stride_ms':10,
            'source_capture_errors':source_errors,
            'capture_discontinuities_in_active_audio':capture_discontinuities,
            'capture_position_gaps':position_gaps,'capture_timestamp_errors':timestamp_errors,
            'endpoint_ticks_late_ge_5ms':endpoint_stalls,
            'endpoint_late_total_ms':sum(s['extra_ms'] for s in endpoint_stalls),
            'correlation_median':float(np.median(correlations)),
            'correlation_p05':float(np.percentile(correlations,5)),
            'low_correlation_windows':int(np.sum(correlations<.75)),
            'near_silent_gaps_ge_2ms':len(output_gaps),
            'near_silent_total_ms':sum(b-a for a,b in output_gaps)/RATE*1000,
            'output_clipped_samples':int(np.sum(np.abs(output_raw[c:d])>=.999)),
            'source_clipped_samples':int(np.sum(np.abs(source_raw[a:b])>=.999)),
            'source_rms':source_rms,'output_rms':output_rms,
            'output_gaps':[{'start_s':(c+x)/RATE,'length_ms':(y-x)/RATE*1000} for x,y in output_gaps]}
    (folder/'quality-windows.json').write_text(json.dumps(windows))
    (folder/'quality.json').write_text(json.dumps(report,indent=2))
    return report

if __name__=='__main__':
    folder=Path(sys.argv[1]).resolve()
    assert json.loads((folder/'session.json').read_text()).get('continuous'), 'Use the continuous test'
    runs=json.loads((folder/'session.json').read_text()).get('runs',3)
    reports=[analyze(folder/f'run-{n}') for n in range(1,runs+1)]
    summary={'runs':reports,'windows':sum(r['windows'] for r in reports),
             'near_silent_gaps_ge_2ms':sum(r['near_silent_gaps_ge_2ms'] for r in reports),
             'near_silent_total_ms':sum(r['near_silent_total_ms'] for r in reports),
             'low_correlation_windows':sum(r['low_correlation_windows'] for r in reports),
             'worst_p05_correlation':min(r['correlation_p05'] for r in reports),
             'output_clipped_samples':sum(r['output_clipped_samples'] for r in reports)}
    summary['capture_errors']=sum(r['source_capture_errors']+r['capture_discontinuities_in_active_audio']+r['capture_position_gaps']+r['capture_timestamp_errors'] for r in reports)
    summary['endpoint_ticks_late_ge_5ms']=sum(len(r['endpoint_ticks_late_ge_5ms']) for r in reports)
    summary['endpoint_late_total_ms']=sum(r['endpoint_late_total_ms'] for r in reports)
    (folder/'quality.json').write_text(json.dumps(summary,indent=2))
    print(json.dumps(summary,indent=2))
