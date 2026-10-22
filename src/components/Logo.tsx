import { useId } from "react";

/** The GameMode mark: a D-pad whose right arm is a play button. */
export function Logo({ size = 28 }: { size?: number }) {
  const id = useId();
  return (
    <svg className="logo" width={size} height={size} viewBox="0 0 1024 1024" aria-hidden>
      <defs>
        <linearGradient id={`${id}-bg`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#151b27" />
          <stop offset="1" stopColor="#05070b" />
        </linearGradient>
        <linearGradient id={`${id}-mark`} x1="212" y1="212" x2="842" y2="812" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#7dffd6" />
          <stop offset="0.5" stopColor="#5ef2c8" />
          <stop offset="1" stopColor="#4aa3ff" />
        </linearGradient>
      </defs>
      <rect width="1024" height="1024" rx="232" fill={`url(#${id}-bg)`} />
      <path
        transform="translate(-28 0)"
        fill={`url(#${id}-mark)`}
        d="M470 212H554A48 48 0 0 1 602 260V334L838 476Q890 512 838 548L602 690V764A48 48 0 0 1 554 812H470A48 48 0 0 1 422 764V602H260A48 48 0 0 1 212 554V470A48 48 0 0 1 260 422H422V260A48 48 0 0 1 470 212Z"
      />
    </svg>
  );
}
