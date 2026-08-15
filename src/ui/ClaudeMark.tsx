/**
 * A twelve-armed radial burst in Anthropic's clay tone — the visual anchor of the
 * overlay. Arms are tapered spokes so it reads as a mark, not a loading spinner.
 *
 * NOTE ON BRANDING: this is drawn from scratch for personal use. If you ever
 * distribute this app publicly, swap it for your own glyph — the Claude/Anthropic
 * mark is a trademark and shipping it in a third-party product is not yours to do.
 */
export function ClaudeMark({
  size = 18,
  spin = false,
  className = "",
}: {
  size?: number;
  spin?: boolean;
  className?: string;
}) {
  const arms = 12;
  const spokes = Array.from({ length: arms }, (_, i) => {
    const angle = (i / arms) * 360;
    // Alternate arm length so the burst has rhythm instead of looking like a gear.
    // These numbers are mirrored in scripts/make-icons.py — keep them in step.
    const long = i % 2 === 0;
    const outer = long ? 47 : 32;
    const width = long ? 7 : 5;
    return (
      <rect
        key={i}
        x={50 - width / 2}
        y={50 - outer}
        width={width}
        height={outer - 9}
        rx={width / 2}
        transform={`rotate(${angle} 50 50)`}
      />
    );
  });

  return (
    <svg
      viewBox="0 0 100 100"
      width={size}
      height={size}
      className={className}
      style={spin ? { animation: "ca-spin 9s linear infinite" } : undefined}
      aria-hidden="true"
      fill="currentColor"
    >
      {spokes}
    </svg>
  );
}
