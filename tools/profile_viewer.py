#!/usr/bin/env python3
"""
Codey Performance Profile Viewer

A visualization and analysis tool for Codey's profiling data.
Designed for tight render loops with many small cumulative function calls.

Features:
- Flame graph visualization (SVG and terminal)
- LLM-friendly analysis summaries
- Cumulative time breakdown
- Frame-by-frame analysis
- Hot spot detection

Usage:
    python tools/profile_viewer.py profile.json
    python tools/profile_viewer.py profile.json --format svg --output flame.svg
    python tools/profile_viewer.py profile.json --analyze
    python tools/profile_viewer.py profile.json --llm-summary
"""

import argparse
import json
import sys
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Optional


@dataclass
class SpanData:
    """A single profiling span."""
    name: str
    parent_idx: Optional[int]
    start_us: int
    duration_us: int
    frame: int
    count: Optional[int] = None


@dataclass
class SpanStats:
    """Aggregated statistics for a span name."""
    calls: int
    total_us: int
    min_us: int
    max_us: int
    self_us: int
    total_count: Optional[int] = None


class ProfileData:
    """Container for loaded profile data."""

    def __init__(self, data: dict):
        self.session = data.get("session", {})
        self.duration_ms = data.get("duration_ms", 0)
        self.frame_count = data.get("frame_count", 0)

        # Parse stats
        self.stats: dict[str, SpanStats] = {}
        for name, s in data.get("stats", {}).items():
            self.stats[name] = SpanStats(
                calls=s["calls"],
                total_us=s["total_us"],
                min_us=s["min_us"],
                max_us=s["max_us"],
                self_us=s["self_us"],
                total_count=s.get("total_count"),
            )

        # Parse spans
        self.spans: list[SpanData] = []
        for s in data.get("spans", []):
            self.spans.append(SpanData(
                name=s["name"],
                parent_idx=s.get("parent_idx"),
                start_us=s["start_us"],
                duration_us=s["duration_us"],
                frame=s["frame"],
                count=s.get("count"),
            ))

        # Parse call tree
        self.call_tree = data.get("call_tree", {})

    @classmethod
    def load(cls, path: Path) -> "ProfileData":
        """Load profile data from JSON file."""
        with open(path) as f:
            return cls(json.load(f))

    def fps(self) -> float:
        """Calculate average frames per second."""
        if self.duration_ms == 0:
            return 0.0
        return self.frame_count / (self.duration_ms / 1000.0)

    def avg_frame_time_ms(self) -> float:
        """Calculate average frame time in milliseconds."""
        if self.frame_count == 0:
            return 0.0
        return self.duration_ms / self.frame_count


def format_duration(us: int) -> str:
    """Format microseconds as human-readable duration."""
    if us < 1000:
        return f"{us}µs"
    elif us < 1_000_000:
        return f"{us/1000:.2f}ms"
    else:
        return f"{us/1_000_000:.2f}s"


def format_percent(value: int, total: int) -> str:
    """Format as percentage."""
    if total == 0:
        return "0.0%"
    return f"{100.0 * value / total:.1f}%"


def print_summary(profile: ProfileData):
    """Print a summary of the profile data."""
    print("=" * 60)
    print("CODEY PERFORMANCE PROFILE SUMMARY")
    print("=" * 60)
    print()

    # Session info
    print(f"Version:        {profile.session.get('version', 'unknown')}")
    print(f"Platform:       {profile.session.get('platform', 'unknown')}")
    print(f"Terminal Size:  {profile.session.get('terminal_size', [0, 0])}")
    print(f"Started:        {profile.session.get('started_at', 'unknown')}")
    print()

    # Performance metrics
    print(f"Duration:       {format_duration(profile.duration_ms * 1000)}")
    print(f"Frames:         {profile.frame_count}")
    print(f"Avg FPS:        {profile.fps():.1f}")
    print(f"Avg Frame Time: {profile.avg_frame_time_ms():.2f}ms")
    print()

    # Top spans by total time
    print("-" * 60)
    print("TOP SPANS BY TOTAL TIME")
    print("-" * 60)
    print(f"{'Span':<40} {'Calls':>8} {'Total':>10} {'%':>6}")
    print("-" * 60)

    total_time = sum(s.total_us for s in profile.stats.values())
    sorted_stats = sorted(
        profile.stats.items(),
        key=lambda x: x[1].total_us,
        reverse=True
    )

    for name, stat in sorted_stats[:15]:
        pct = format_percent(stat.total_us, total_time)
        print(f"{name:<40} {stat.calls:>8} {format_duration(stat.total_us):>10} {pct:>6}")

    print()


def print_per_call_analysis(profile: ProfileData):
    """Analyze per-call performance - useful for finding expensive individual calls."""
    print("-" * 60)
    print("PER-CALL ANALYSIS (Average Time)")
    print("-" * 60)
    print(f"{'Span':<40} {'Avg':>10} {'Min':>10} {'Max':>10}")
    print("-" * 60)

    sorted_stats = sorted(
        profile.stats.items(),
        key=lambda x: x[1].total_us / max(x[1].calls, 1),
        reverse=True
    )

    for name, stat in sorted_stats[:15]:
        avg_us = stat.total_us // max(stat.calls, 1)
        print(f"{name:<40} {format_duration(avg_us):>10} {format_duration(stat.min_us):>10} {format_duration(stat.max_us):>10}")

    print()


def print_cumulative_analysis(profile: ProfileData):
    """Analyze cumulative time - useful for finding death-by-a-thousand-cuts issues."""
    print("-" * 60)
    print("CUMULATIVE IMPACT ANALYSIS")
    print("-" * 60)
    print("Spans with many small calls that add up to significant time:")
    print()
    print(f"{'Span':<35} {'Calls':>8} {'Avg':>8} {'Total':>10} {'Impact':>8}")
    print("-" * 60)

    total_time = sum(s.total_us for s in profile.stats.values())

    # Find spans with high call count and significant cumulative time
    cumulative_candidates = []
    for name, stat in profile.stats.items():
        avg_us = stat.total_us / max(stat.calls, 1)
        # High call count (>100) with small average (<1ms) but significant total (>1%)
        if stat.calls > 100 and avg_us < 1000:
            impact = stat.total_us / max(total_time, 1) * 100
            if impact > 0.5:  # At least 0.5% of total time
                cumulative_candidates.append((name, stat, avg_us, impact))

    cumulative_candidates.sort(key=lambda x: x[3], reverse=True)

    for name, stat, avg_us, impact in cumulative_candidates[:10]:
        print(f"{name:<35} {stat.calls:>8} {format_duration(int(avg_us)):>8} {format_duration(stat.total_us):>10} {impact:>7.1f}%")

    if not cumulative_candidates:
        print("  (No significant cumulative impact patterns found)")

    print()


def generate_llm_summary(profile: ProfileData) -> str:
    """Generate an LLM-friendly summary for analysis assistance."""
    total_time = sum(s.total_us for s in profile.stats.values())

    lines = [
        "# Codey Performance Profile Analysis",
        "",
        "## Session Overview",
        f"- **Duration**: {format_duration(profile.duration_ms * 1000)}",
        f"- **Frames**: {profile.frame_count}",
        f"- **Average FPS**: {profile.fps():.1f}",
        f"- **Average Frame Time**: {profile.avg_frame_time_ms():.2f}ms",
        f"- **Terminal Size**: {profile.session.get('terminal_size', 'unknown')}",
        "",
        "## Performance Breakdown",
        "",
        "| Span | Calls | Total Time | % of Total | Avg/Call |",
        "|------|-------|------------|------------|----------|",
    ]

    sorted_stats = sorted(
        profile.stats.items(),
        key=lambda x: x[1].total_us,
        reverse=True
    )

    for name, stat in sorted_stats[:20]:
        pct = 100.0 * stat.total_us / max(total_time, 1)
        avg = stat.total_us / max(stat.calls, 1)
        lines.append(
            f"| {name} | {stat.calls} | {format_duration(stat.total_us)} | {pct:.1f}% | {format_duration(int(avg))} |"
        )

    lines.extend([
        "",
        "## Analysis Questions",
        "",
        "Based on this profile, consider investigating:",
        "",
    ])

    # Add specific recommendations based on data
    recommendations = []

    for name, stat in sorted_stats[:5]:
        pct = 100.0 * stat.total_us / max(total_time, 1)
        if pct > 20:
            recommendations.append(
                f"1. **{name}** consumes {pct:.1f}% of total time ({stat.calls} calls). "
                f"Is this expected? Can it be optimized or called less frequently?"
            )

    # Check for cumulative issues
    for name, stat in profile.stats.items():
        avg_us = stat.total_us / max(stat.calls, 1)
        pct = 100.0 * stat.total_us / max(total_time, 1)
        if stat.calls > 1000 and avg_us < 100 and pct > 5:
            recommendations.append(
                f"2. **{name}** is called {stat.calls} times with tiny average duration "
                f"({format_duration(int(avg_us))}), but accounts for {pct:.1f}% of total time. "
                f"This is a 'death by a thousand cuts' pattern - consider batching or caching."
            )

    if not recommendations:
        recommendations.append("- No obvious hotspots detected. Profile looks healthy.")

    lines.extend(recommendations)

    lines.extend([
        "",
        "## Raw Data",
        "",
        "```json",
        json.dumps({
            "top_spans": [
                {
                    "name": name,
                    "calls": stat.calls,
                    "total_us": stat.total_us,
                    "avg_us": stat.total_us // max(stat.calls, 1),
                    "percent": round(100.0 * stat.total_us / max(total_time, 1), 2),
                }
                for name, stat in sorted_stats[:10]
            ]
        }, indent=2),
        "```",
    ])

    return "\n".join(lines)


def generate_flame_svg(profile: ProfileData, width: int = 1200, row_height: int = 20) -> str:
    """Generate an SVG flame graph."""
    total_time = profile.duration_ms * 1000  # Convert to microseconds

    if total_time == 0:
        return "<svg><text>No profiling data</text></svg>"

    # Build flame graph data structure
    # Group spans by frame and build call stacks
    frame_stacks: dict[int, list[SpanData]] = defaultdict(list)
    for span in profile.spans:
        frame_stacks[span.frame].append(span)

    # For simplicity, we'll create a simplified flame graph showing
    # aggregated time per span name at each depth level

    # Calculate depths (for now, assume single level from stats)
    sorted_stats = sorted(
        profile.stats.items(),
        key=lambda x: x[1].total_us,
        reverse=True
    )[:20]  # Top 20 spans

    height = (len(sorted_stats) + 2) * row_height

    svg_parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">',
        '<style>',
        '  .frame { stroke: #333; stroke-width: 0.5; }',
        '  .frame:hover { stroke: #000; stroke-width: 1.5; }',
        '  .label { font-family: monospace; font-size: 11px; fill: #333; pointer-events: none; }',
        '  .title { font-family: sans-serif; font-size: 14px; font-weight: bold; }',
        '</style>',
        f'<text x="10" y="18" class="title">Codey Performance Profile - {profile.frame_count} frames, {format_duration(total_time)} total</text>',
    ]

    y = row_height + 10

    for name, stat in sorted_stats:
        # Calculate width proportional to time
        w = max(1, int((stat.total_us / total_time) * (width - 20)))
        pct = 100.0 * stat.total_us / total_time

        # Color based on percentage (red = hot, blue = cold)
        hue = max(0, 240 - int(pct * 8))  # 240 (blue) to 0 (red)
        color = f"hsl({hue}, 70%, 60%)"

        svg_parts.extend([
            f'<g>',
            f'  <rect class="frame" x="10" y="{y}" width="{w}" height="{row_height - 2}" fill="{color}">',
            f'    <title>{name}\n{stat.calls} calls, {format_duration(stat.total_us)} total ({pct:.1f}%)</title>',
            f'  </rect>',
        ])

        # Add label if it fits
        if w > 50:
            label = f"{name} ({pct:.1f}%)"
            if len(label) * 7 > w:
                label = name[:w // 7 - 3] + "..."
            svg_parts.append(
                f'  <text class="label" x="15" y="{y + row_height - 6}">{label}</text>'
            )

        svg_parts.append('</g>')
        y += row_height

    svg_parts.append('</svg>')
    return '\n'.join(svg_parts)


def print_terminal_flame(profile: ProfileData, width: int = 80):
    """Print a simple terminal-based flame graph."""
    print("-" * width)
    print("FLAME GRAPH (Terminal)")
    print("-" * width)

    total_time = sum(s.total_us for s in profile.stats.values())
    bar_width = width - 50  # Leave room for labels

    sorted_stats = sorted(
        profile.stats.items(),
        key=lambda x: x[1].total_us,
        reverse=True
    )[:20]

    for name, stat in sorted_stats:
        pct = stat.total_us / max(total_time, 1)
        bar_len = int(pct * bar_width)
        bar = "█" * bar_len + "░" * (bar_width - bar_len)

        # Truncate name if needed
        display_name = name[:30] if len(name) > 30 else name
        print(f"{display_name:<30} {bar} {100*pct:>5.1f}%")

    print()


def main():
    parser = argparse.ArgumentParser(
        description="Codey Performance Profile Viewer",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__
    )
    parser.add_argument("profile", type=Path, help="Path to profile JSON file")
    parser.add_argument("--format", choices=["text", "svg", "json"], default="text",
                       help="Output format")
    parser.add_argument("--output", "-o", type=Path, help="Output file (default: stdout)")
    parser.add_argument("--analyze", action="store_true", help="Include detailed analysis")
    parser.add_argument("--llm-summary", action="store_true",
                       help="Generate LLM-friendly markdown summary")
    parser.add_argument("--width", type=int, default=1200, help="SVG width in pixels")

    args = parser.parse_args()

    if not args.profile.exists():
        print(f"Error: Profile file not found: {args.profile}", file=sys.stderr)
        sys.exit(1)

    try:
        profile = ProfileData.load(args.profile)
    except Exception as e:
        print(f"Error loading profile: {e}", file=sys.stderr)
        sys.exit(1)

    output = ""

    if args.format == "text":
        if args.llm_summary:
            output = generate_llm_summary(profile)
        else:
            # Capture stdout for file output
            import io
            buf = io.StringIO()
            old_stdout = sys.stdout
            sys.stdout = buf

            print_summary(profile)
            if args.analyze:
                print_per_call_analysis(profile)
                print_cumulative_analysis(profile)
            print_terminal_flame(profile)

            sys.stdout = old_stdout
            output = buf.getvalue()

    elif args.format == "svg":
        output = generate_flame_svg(profile, width=args.width)

    elif args.format == "json":
        output = generate_llm_summary(profile)

    if args.output:
        args.output.write_text(output)
        print(f"Output written to: {args.output}", file=sys.stderr)
    else:
        print(output)


if __name__ == "__main__":
    main()
