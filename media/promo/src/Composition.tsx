import {AbsoluteFill, CanvasImage, Easing, Interactive, interpolate, staticFile, useCurrentFrame, useVideoConfig} from 'remotion';
import {CuteDeveloper} from './CuteDeveloper';

const gold = '#bd9653';
const clamp = {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'} as const;
const ease = {...clamp, easing: Easing.bezier(0.22, 1, 0.36, 1)};

const DeskStoryInner = () => {
  const frame = useCurrentFrame();
  const {fps, width} = useVideoConfig();
  const s = frame / fps;
  const arrival = interpolate(s, [0.3, 2.7], [0, 1], ease);
  const placed = interpolate(s, [2.5, 4.1], [0, 1], ease);
  const lit = interpolate(s, [4.1, 5.1], [0, 1], clamp);
  const app = interpolate(s, [4.8, 5.6], [0, 1], ease);
  const headphones = interpolate(s, [9.7, 10.6], [0, 1], ease);
  const hx = interpolate(headphones, [0, 1], [995, 640]);
  const hy = interpolate(headphones, [0, 1], [452, 565]);
  const cx = interpolate(s, [0, 6.8, 7.7, 8.2, 9.6, 12.2, 13.6, 14.1, 15.5, 18], [850, 850, 728, 680, 540, 540, 680, 728, 850, 850], ease);
  const cy = interpolate(s, [0, 6.8, 9.6, 12.2, 15.5, 18], [335, 335, 380, 380, 335, 335], ease);
  const laptopFocused = cx < 700 && s > 7.7;
  const title = s < 2.5 ? 'Long day. Less friction.' : s < 5.8 ? 'Set it down. Stay connected.' : s < 12 ? 'One cursor. Two computers.' : 'Keep your setup. Lose the switching.';
  const subtitle = s < 5.8 ? 'Your laptop joins the desk. Your peripherals stay put.' : 'Keyboard, mouse and headset stay wired to the PC.';
  const typed = 'Hello from the PC keyboard.'.slice(0, Math.max(0, Math.floor((s - 9.3) * 13)));

  return <AbsoluteFill style={{background: '#f6f3eb', color: '#292a2d', fontFamily: 'Segoe UI, Arial, sans-serif', opacity:interpolate(s,[0,.4,17.4,18],[0,1,1,0],clamp)}}>
    {/* Scale the authored vector scene, not a previously rendered bitmap. */}
    <div style={{position:'absolute', width:1280, height:720, left:0, top:0, scale:width/1280, transformOrigin:'top left'}}>
    <div style={{position: 'absolute', inset: 0, background: 'radial-gradient(ellipse at 65% 65%, #e8ddc4 0%, transparent 60%)'}} />
    <CanvasImage src={staticFile('mineshare-logo.png')} style={{position:'absolute', left:62, top:38, width:48, height:48, borderRadius:12}} />
    <div style={{position:'absolute', left:124, top:43, fontWeight:650, fontSize:29, letterSpacing:-1}}>MineShare</div>
    <div style={{position:'absolute', right:65, top:50, color:'#786a51', fontSize:16, letterSpacing:3}}>ONE SETUP. SEAMLESS CONTROL.</div>
    <div style={{position:'absolute', left:64, top:110, fontSize:56, fontWeight:620, letterSpacing:-2.5}}>{title}</div>
    <div style={{position:'absolute', left:67, top:184, fontSize:24, color:'#78746d'}}>{subtitle}</div>
    {/* Clip camera movement to the desk: titles and footer remain stationary. */}
    <div style={{position:'absolute', left:0, top:225, width:1280, height:425, overflow:'hidden'}}>
    <svg viewBox="0 0 1280 720" style={{position:'absolute', left:0, top:-225, width:1280, height:720,
      transformOrigin:'710px 300px',
      scale:interpolate(s,[6.35,7.4,10.5,11.6],[1,1.32,1.32,1],ease)}}>
      <defs>
        <linearGradient id="desktop" x2="1" y2="1"><stop stopColor="#203f65"/><stop offset="1" stopColor="#7290b1"/></linearGradient>
        <linearGradient id="laptop" x2="1" y2="1"><stop stopColor="#194d87"/><stop offset="1" stopColor="#67a9d8"/></linearGradient>
      </defs>
      <ellipse cx="724" cy="615" rx="480" ry="20" fill="#e7dfd0"/>
      <path d="M343 489h795l49 57H303z" fill="#dccbb2"/>
      <rect x="303" y="546" width="884" height="28" rx="5" fill="#bba383"/>
      <path d="M339 573v73m807-73v73" stroke="#89765c" strokeWidth="14"/>
      <path d="M862 435v50m-48 4h96" stroke="#4b5055" strokeWidth="12" strokeLinecap="round"/>
      <rect x="718" y="263" width="289" height="182" rx="10" fill="#30353a"/>
      <rect x="726" y="271" width="273" height="162" rx="4" fill="url(#desktop)"/>
      <rect x="746" y="289" width="230" height="118" rx="6" fill="#f6f6f5"/>
      <path d="M746 313h230" stroke="#e2e5e6"/><circle cx="758" cy="301" r="3" fill="#c5a36a"/>
      <rect x="760" y="324" width="79" height="67" rx="5" fill="#dfe7e9"/>
      <path d="M853 328h103m-103 15h78m-78 15h90" stroke="#c7cdce" strokeWidth="5" strokeLinecap="round"/>
      <rect x="1053" y="332" width="94" height="205" rx="11" fill="#373d42"/>
      <rect x="1066" y="346" width="68" height="145" rx="6" fill="#282e33"/>
      <circle cx="1100" cy="368" r="8" fill="#56777d"/>
      <circle cx="1100" cy="406" r="21" fill="none" stroke="#4a555c" strokeWidth="2"/>
      <circle cx="1100" cy="456" r="21" fill="none" stroke="#4a555c" strokeWidth="2"/>
      <circle cx="1100" cy="517" r="4" fill="#bda267"/>
      <g opacity={placed} style={{translate:`0px ${interpolate(placed,[0,1],[-75,0])}px`}}>
        <rect x="421" y="324" width="279" height="176" rx="11" fill="#70787f"/>
        <rect x="430" y="333" width="261" height="157" rx="4" fill="#1f2934"/>
        <rect x="430" y="333" width="261" height="157" rx="4" fill="url(#laptop)" opacity={lit}/>
        <g opacity={app}>
          <rect x="444" y="345" width="233" height="132" rx="5" fill="#faf8f1"/>
          <rect x="444" y="345" width="48" height="132" rx="5" fill="#ede8da"/>
          <text x="459" y="370" fontSize="19" fill={gold} fontWeight="bold">M</text>
          <path d="M454 388h26m-26 12h20m-20 12h23" stroke="#c8bea8" strokeWidth="3" strokeLinecap="round"/>
          <text x="504" y="362" fontSize="9" fill="#393b3c" fontWeight="bold">MineShare</text>
          <circle cx="655" cy="358" r="3" fill="#58b590"/>
          <rect x="504" y="381" width="69" height="42" rx="5" stroke={gold} fill="#f1e9d6"/>
          <rect x="589" y="385" width="66" height="38" rx="5" stroke="#d8d3c7" fill="#fff"/>
          <path d="M574 402h15" stroke={gold} strokeWidth="2"/>
          <text x="538" y="405" textAnchor="middle" fontSize="8" fill="#766340">Laptop</text>
          <text x="622" y="406" textAnchor="middle" fontSize="8" fill="#555">PC</text>
          <rect x="500" y="434" width="164" height="31" rx="4" fill="#fff"/>
          <text x="507" y="453" fontSize="8.5" fill="#4b5053">{typed}</text>
        </g>
        <path d="M404 500h314l-14 14H420z" fill="#9ca2a7"/>
        <path d="M533 501h54l-7 5h-40z" fill="#747c83"/>
      </g>
      {/* All three cables terminate at PC tower; never at laptop. */}
      <path d="M623 534v52q0 10 13 10h472q20 0 20-20v-77" fill="none" stroke="#6f7e80" strokeWidth="3"/>
      <path d="M765 527v44q0 10 14 10h345q12 0 12-13v-62" fill="none" stroke={gold} strokeWidth="3"/>
      <path d={`M${hx+41} ${hy+30}Q${hx+70} 611 1010 611H1100q20 0 20-25v-94`} fill="none" stroke="#8d8b94" strokeWidth="3"/>
      <path d="M488 515h268l20 25H472z" fill="#5b6268"/>
      {[0,1,2].map(row=><path key={row} d={`M${494-row*3} ${521+row*6}h250`} stroke="#98a0a4" strokeWidth="2" strokeDasharray="9 4"/>)}
      <rect x="749" y="505" width="33" height="37" rx="15" fill="#636b70"/>
      <path d="M765 508v13" stroke="#b8bec0" strokeWidth="2"/>
      <CuteDeveloper s={s} arrival={arrival} placed={placed}/>
      <g style={{translate:`${hx}px ${hy}px`}}>
        <path d="M-40 12v-14q0-41 40-41t41 41v14" stroke="#555e64" strokeWidth="9" fill="none"/>
        <rect x="-48" y="2" width="16" height="32" rx="8" fill="#404a50"/>
        <rect x="33" y="2" width="16" height="32" rx="8" fill="#404a50"/>
      </g>
      {s>10.5 && <g opacity={.5+.4*Math.sin(frame/7)}><path d="M699 565q12 12 0 25m9-32q19 18 0 39" stroke={gold} strokeWidth="2" fill="none"/></g>}
      {s>5.7 && !(cx>699 && cx<726) && <g style={{translate:`${cx}px ${cy}px`}}>
        <path d="M0 0l5 25 6-8 9-2z" fill="#fff" stroke="#30383f" strokeWidth="2.5" strokeLinejoin="round"/>
      </g>}
      <text x="493" y="635" textAnchor="middle" fill="#777166" fontSize="13">KEYBOARD</text>
      <text x="790" y="635" textAnchor="middle" fill="#777166" fontSize="13">MOUSE</text>
      <text x="1031" y="635" textAnchor="middle" fill="#777166" fontSize="13">HEADSET</text>
    </svg>
    </div>
    <CanvasImage src={staticFile('ubuntu-cof.svg')} style={{position:'absolute',
      left:interpolate(s,[6.35,7.4,10.5,11.6],[846,894.64,894.64,846],ease),
      top:interpolate(s,[6.35,7.4,10.5,11.6],[228,211,211,228],ease),width:32,height:32}}/>
    <CanvasImage src={staticFile('windows-mark.svg')} style={{position:'absolute', opacity:placed,
      left:interpolate(s,[6.35,7.4,10.5,11.6],[544,496,496,544],ease),
      top:interpolate(s,[6.35,7.4,10.5,11.6],[286,294,294,286],ease),width:32,height:32}}/>
    <div style={{position:'absolute', left:67, top:230, width:178, height:28,
      borderRadius:14, background:'#e5eee5', color:'#426f58', fontSize:13, display:'flex',
      alignItems:'center', gap:10, paddingLeft:16, boxSizing:'border-box', opacity:s>5.7?1:0}}>
      <span style={{width:8,height:8,borderRadius:4,background:'#5c9b79'}} />
      {laptopFocused?'FOCUS · LAPTOP':'FOCUS · PC'}
    </div>
    <div style={{position:'absolute',left:66,bottom:33,fontSize:15,color:'#8a8375',letterSpacing:1}}>WORKFLOW ILLUSTRATION · NOT A SCREEN RECORDING</div>
    <div style={{position:'absolute',right:67,bottom:33,fontSize:17,color:'#706855'}}>Windows + Ubuntu <span style={{color:gold}}> / </span> Encrypted over your LAN</div>
    </div>
  </AbsoluteFill>;
};

export const DeskStory = Interactive.create({Component: DeskStoryInner, layout:'absolute-fill'});
