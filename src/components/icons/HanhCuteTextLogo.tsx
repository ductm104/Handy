import React from "react";

const HanhCuteTextLogo = ({
  width,
  height,
  className,
}: {
  width?: number;
  height?: number;
  className?: string;
}) => {
  return (
    <svg
      width={width || 600}
      height={height || 200}
      className={className}
      viewBox="0 0 600 200"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
    >
      <defs>
        <filter id="logo-shadow">
          <feDropShadow dx="0" dy="3" stdDeviation="3" floodOpacity="0.25" />
        </filter>
      </defs>
      <text
        x="50%"
        y="52%"
        dominantBaseline="middle"
        textAnchor="middle"
        className="logo-stroke"
        style={{
          fontFamily:
            '"Comic Sans MS", "Chalkboard SE", "Chalkboard", sans-serif',
          fontWeight: 700,
          fontStyle: "normal",
          fontSize: 130,
          strokeWidth: 5,
        }}
      >
        HanhCute
      </text>
      <text
        x="50%"
        y="52%"
        dominantBaseline="middle"
        textAnchor="middle"
        className="logo-primary"
        style={{
          fontFamily:
            '"Comic Sans MS", "Chalkboard SE", "Chalkboard", sans-serif',
          fontWeight: 700,
          fontStyle: "normal",
          fontSize: 130,
        }}
      >
        HanhCute
      </text>
    </svg>
  );
};

export default HanhCuteTextLogo;
