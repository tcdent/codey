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
    item_count: int = 0


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
                item_count=s.get("item_count", 0),
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


def print_analysis(profile: ProfileData):
    """Print detailed analysis with cumulative impact."""
    print("=" * 72)
    print("CODEY PERFORMANCE ANALYSIS")
    print("=" * 72)
    print()

    total_time = sum(s.total_us for s in profile.stats.values())
    if total_time == 0:
        print("No profiling data collected.")
        return

    print(f"Session Duration:  {format_duration(profile.duration_ms * 1000)}")
    print(f"Draw Calls:        {profile.draw_count}")
    if profile.draw_count > 0 and profile.duration_ms > 0:
        fps = profile.draw_count / (profile.duration_ms / 1000)
        print(f"Avg FPS:           {fps:.1f}")
    print(f"Avg Draw Time:     {format_duration(int(profile.avg_draw_time_us()))}")
    print()

    if profile.resources.samples > 0:
        print("--- Resource Usage ---")
        print(f"  Peak Memory:   {format_bytes(profile.resources.peak_memory_bytes)}")
        print(f"  Final Memory:  {format_bytes(profile.resources.final_memory_bytes)}")
        print(f"  Avg CPU:       {profile.resources.avg_cpu_percent:.1f}%")
        print(f"  Peak CPU:      {profile.resources.peak_cpu_percent:.1f}%")
        print(f"  Samples:       {profile.resources.samples}")
        print()

    print("--- Span Analysis (sorted by cumulative impact) ---")
    print()
    print(f"{'Span':<35} {'Calls':>7} {'Total':>9} {'%':>6} {'Avg':>9} {'Min':>9} {'Max':>9}")
    print("-" * 72)

    sorted_stats = sorted(profile.stats.items(), key=lambda x: x[1].total_us, reverse=True)
    cumulative_pct = 0.0
    for name, stat in sorted_stats:
        pct = 100.0 * stat.total_us / total_time
        cumulative_pct += pct
        avg = stat.total_us // max(stat.calls, 1)
        line = (
            f"{name:<35} {stat.calls:>7} {format_duration(stat.total_us):>9} "
            f"{pct:>5.1f}% {format_duration(avg):>9} "
            f"{format_duration(stat.min_us):>9} {format_duration(stat.max_us):>9}"
        )
        print(line)
        if stat.item_count > 0:
            items_per_call = stat.item_count / max(stat.calls, 1)
            us_per_item = stat.total_us / max(stat.item_count, 1)
            print(f"  {'':>35} items: {stat.item_count}  ({items_per_call:.0f}/call, {format_duration(us_per_item)}/item)")

    print()
    print(f"Total profiled time: {format_duration(total_time)}")

    # Hotspot warnings
    print()
    print("--- Hotspot Warnings ---")
    has_warnings = False
    for name, stat in sorted_stats:
        pct = 100.0 * stat.total_us / total_time
        avg = stat.total_us // max(stat.calls, 1)
        if pct > 30:
            print(f"  [!] {name} consumes {pct:.1f}% of profiled time")
            has_warnings = True
        if avg > 16000:  # > 16ms = over frame budget at 60fps
            print(f"  [!] {name} avg {format_duration(avg)} exceeds 16ms frame budget")
            has_warnings = True
    if not has_warnings:
        print("  No hotspots detected.")
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
        "| Span | Calls | Total | % | Avg | Items |",
        "|------|-------|-------|---|-----|-------|",
    ])

    for name, stat in sorted_stats[:15]:
        pct = 100.0 * stat.total_us / max(total_time, 1)
        avg = stat.total_us // max(stat.calls, 1)
        items = str(stat.item_count) if stat.item_count > 0 else "-"
        lines.append(f"| {name} | {stat.calls} | {format_duration(stat.total_us)} | {pct:.1f}% | {format_duration(avg)} | {items} |")

    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Codey Performance Profile Viewer")
    parser.add_argument("profile", type=Path, help="Path to profile JSON file")
    parser.add_argument("--analyze", action="store_true", help="Detailed analysis with cumulative impact and hotspot warnings")
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
    elif args.analyze:
        print_analysis(profile)
    else:
        print_summary(profile)


if __name__ == "__main__":
    main()
