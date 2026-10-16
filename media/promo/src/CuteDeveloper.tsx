import {interpolate} from 'remotion';

type Props = {s:number; arrival:number; placed:number};

/** A round mascot approaches straight from the viewer, always at fixed X. */
export const CuteDeveloper = ({s,arrival,placed}:Props) => {
  const step = Math.sin(s*12)*(1-arrival);
  const y = interpolate(arrival,[0,1],[790,580])+Math.abs(step)*3;
  return <g transform={`translate(640 ${y})`}>
    <ellipse cx="0" cy="60" rx="58" ry="9" fill="#b4a489" opacity=".22"/>
    <ellipse cx="-23" cy={50+step*6} rx="19" ry="11" fill="#72a79b"/>
    <ellipse cx="23" cy={50-step*6} rx="19" ry="11" fill="#72a79b"/>
    <circle r="52" fill="#98cabe"/>
    <path d="M-34-29q12-14 29-15" fill="none" stroke="#c6e6d9" strokeWidth="7" strokeLinecap="round"/>
    <ellipse cx="-30" cy="6" rx="11" ry="7" fill="#edb6ac"/>
    <ellipse cx="30" cy="6" rx="11" ry="7" fill="#edb6ac"/>
    <ellipse cx="-17" cy="-6" rx="4.5" ry="6" fill="#354c49"/>
    <ellipse cx="17" cy="-6" rx="4.5" ry="6" fill="#354c49"/>
    <circle cx="-18" cy="-8" r="1.5" fill="#fff"/>
    <circle cx="16" cy="-8" r="1.5" fill="#fff"/>
    <path d="M-7 10q7 10 14 0" fill="none" stroke="#4f756c" strokeWidth="3" strokeLinecap="round"/>
    <g opacity={1-placed}>
      <rect x="-52" y="20" width="104" height="14" rx="7" fill="#8e99a1"/>
      <path d="M-43 21h86" stroke="#d8dfe2" strokeWidth="3" strokeLinecap="round"/>
      <path d="M-44 0q-19 15-4 25m92-25q19 15 4 25" fill="none" stroke="#78b2a4" strokeWidth="17" strokeLinecap="round"/>
      <circle cx="-48" cy="25" r="10" fill="#b5ddcc"/>
      <circle cx="48" cy="25" r="10" fill="#b5ddcc"/>
    </g>
    <g opacity={placed}>
      <path d="M-43-5Q-67-23-45-49" fill="none" stroke="#78b2a4" strokeWidth="18" strokeLinecap="round"/>
      <path d="M43-5Q86-5 125-47" fill="none" stroke="#78b2a4" strokeWidth="18" strokeLinecap="round"/>
      <ellipse cx="-45" cy="-49" rx="13" ry="9" fill="#b5ddcc"/>
      <ellipse cx="125" cy="-47" rx="12" ry="9" fill="#b5ddcc"/>
    </g>
  </g>;
};
