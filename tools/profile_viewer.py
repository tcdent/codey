#!/usr/bin/env python3
"""
Codey Performance Profile Viewer

Usage:
    uv run python tools/profile_viewer.py profile.json
    uv run python tools/profile_viewer.py profile.json --llm-summary
"""

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass
class Stats:
    """Aggregated statistics for a span."""
    calls: int
    total_us: int
    min_us: int
    max_us: int


@dataclass
class Resources:
    """Resource usage summary."""
    peak_memory_bytes: int
    final_memory_bytes: int
    avg_cpu_percent: float
    peak_cpu_percent: float
    samples: int


class ProfileData:
    """Container for loaded profile data."""

    def __init__(self, data: dict):
        self.duration_ms = data.get("duration_ms", 0)
        self.draw_count = data.get("draw_count", 0)
        self.terminal_size = data.get("terminal_size", (0, 0))

        # Parse stats
        self.stats: dict[str, Stats] = {}
        for name, s in data.get("stats", {}).items():
            self.stats[name] = Stats(
                calls=s["calls"],
                total_us=s["total_us"],
                min_us=s["min_us"],
                max_us=s["max_us"],
            )

        # Parse resources
        res = data.get("resources", {})
        self.resources = Resources(
            peak_memory_bytes=res.get("peak_memory_bytes", 0),
            final_memory_bytes=res.get("final_memory_bytes", 0),
            avg_cpu_percent=res.get("avg_cpu_percent", 0.0),
            peak_cpu_percent=res.get("peak_cpu_percent", 0.0),
            samples=res.get("samples", 0),
        )

    @classmethod
    def load(cls, path: Path) -> "ProfileData":
        with open(path) as f:
            return cls(json.load(f))

    def avg_draw_time_us(self) -> float:
        if "App::draw" in self.stats:
            stat = self.stats["App::draw"]
            return stat.total_us / max(stat.calls, 1)
        return 0.0


def format_bytes(b: int) -> str:
    if b < 1024:
        return f"{b}B"
    elif b < 1024 * 1024:
        return f"{b/1024:.1f}KB"
    elif b < 1024 * 1024 * 1024:
        return f"{b/(1024*1024):.1f}MB"
    else:
        return f"{b/(1024*1024*1024):.2f}GB"


def format_duration(us: int) -> str:
    if us < 1000:
        return f"{us}µs"
    elif us < 1_000_000:
        return f"{us/1000:.2f}ms"
    else:
        return f"{us/1_000_000:.2f}s"


def print_summary(profile: ProfileData):
    print("=" * 60)
    print("CODEY PERFORMANCE PROFILE")
    print("=" * 60)
    print()
    print(f"Duration:       {format_duration(profile.duration_ms * 1000)}")
    print(f"Draw Calls:     {profile.draw_count}")
    print(f"Avg Draw Time:  {format_duration(int(profile.avg_draw_time_us()))}")
    print(f"Terminal Size:  {profile.terminal_size}")
    print()

    if profile.resources.samples > 0:
        print(f"Peak Memory:    {format_bytes(profile.resources.peak_memory_bytes)}")
        print(f"Final Memory:   {format_bytes(profile.resources.final_memory_bytes)}")
        print(f"Avg CPU:        {profile.resources.avg_cpu_percent:.1f}%")
        print(f"Peak CPU:       {profile.resources.peak_cpu_percent:.1f}%")
        print()

    print("-" * 60)
    print(f"{'Span':<40} {'Calls':>8} {'Total':>10} {'Avg':>10}")
    print("-" * 60)

    sorted_stats = sorted(profile.stats.items(), key=lambda x: x[1].total_us, reverse=True)
    for name, stat in sorted_stats[:15]:
        avg = stat.total_us // max(stat.calls, 1)
        print(f"{name:<40} {stat.calls:>8} {format_duration(stat.total_us):>10} {format_duration(avg):>10}")
    print()


def generate_llm_summary(profile: ProfileData) -> str:
    total_time = sum(s.total_us for s in profile.stats.values())
    sorted_stats = sorted(profile.stats.items(), key=lambda x: x[1].total_us, reverse=True)

    lines = [
        "# Codey Performance Profile",
        "",
        "## Overview",
        f"- **Duration**: {format_duration(profile.duration_ms * 1000)}",
        f"- **Draw Calls**: {profile.draw_count}",
        f"- **Avg Draw Time**: {format_duration(int(profile.avg_draw_time_us()))}",
    ]

    if profile.resources.samples > 0:
        lines.extend([
            "",
            "## Resources",
            f"- **Peak Memory**: {format_bytes(profile.resources.peak_memory_bytes)}",
            f"- **Final Memory**: {format_bytes(profile.resources.final_memory_bytes)}",
            f"- **Avg CPU**: {profile.resources.avg_cpu_percent:.1f}%",
            f"- **Peak CPU**: {profile.resources.peak_cpu_percent:.1f}%",
        ])

    lines.extend([
        "",
        "## Timing",
        "",
        "| Span | Calls | Total | % | Avg |",
        "|------|-------|-------|---|-----|",
    ])

    for name, stat in sorted_stats[:15]:
        pct = 100.0 * stat.total_us / max(total_time, 1)
        avg = stat.total_us // max(stat.calls, 1)
        lines.append(f"| {name} | {stat.calls} | {format_duration(stat.total_us)} | {pct:.1f}% | {format_duration(avg)} |")

    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Codey Performance Profile Viewer")
    parser.add_argument("profile", type=Path, help="Path to profile JSON file")
    parser.add_argument("--llm-summary", action="store_true", help="Generate LLM-friendly markdown")
    args = parser.parse_args()

    if not args.profile.exists():
        print(f"Error: File not found: {args.profile}", file=sys.stderr)
        sys.exit(1)

    try:
        profile = ProfileData.load(args.profile)
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)

    if args.llm_summary:
        print(generate_llm_summary(profile))
    else:
        print_summary(profile)


if __name__ == "__main__":
    main()
