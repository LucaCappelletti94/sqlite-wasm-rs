"""Summarizes results/*.jsonl into results/summary.csv, results/report.md and SVG figures: `python3 analyze.py`."""

import csv
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

import matplotlib

matplotlib.use("svg")
# Text stays text, which keeps the SVGs small and searchable.
matplotlib.rcParams["svg.fonttype"] = "none"
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.lines import Line2D  # noqa: E402
from matplotlib.patches import Patch  # noqa: E402

HERE = Path(__file__).parent
RESULTS = HERE / "results"
# Smoke, reproduction and stress runs are diagnostics, not part of the matrix.
EXCLUDED_SUFFIXES = ("-quick", "-repro", "-stress")
RUNTIMES = ["chrome", "firefox", "webkit", "node", "bun"]
COLORS = dict(zip(RUNTIMES, ["#1f77b4", "#d62728", "#7f7f7f", "#2ca02c", "#9467bd"]))
WORKLOADS = ["point", "range", "scan", "sort", "fts", "insert", "mix"]
MODALITIES = [
    ("threadsafe", "memvfs_shared", False, "memvfs, one shared database"),
    ("threadsafe", "memvfs_per_worker", False, "memvfs, one database per worker"),
    ("threadsafe", "memory_per_worker", False, "private :memory: database per worker"),
    ("threadsafe", "sahpool_per_worker", False, "OPFS sahpool, one pool per worker (flush-bound control)"),
    ("cipher", "cipher_shared", False, "SQLite3MC, one shared encrypted database"),
    ("cipher", "cipher_per_worker", False, "SQLite3MC, one encrypted database per worker"),
    ("threadsafe", "memvfs_shared", True, "memvfs shared, NOMUTEX connections"),
    ("threadsafe", "memory_per_worker", True, "private :memory: per worker, NOMUTEX connections"),
]


def load():
    rows, environments = [], {}
    for path in sorted(RESULTS.glob("*.jsonl")):
        if path.stem.endswith(EXCLUDED_SUFFIXES):
            continue
        for line in path.read_text().splitlines():
            record = json.loads(line)
            if "environment" in record:
                environments[path.stem] = record["environment"]
            elif record["round"] >= 0:
                record["source"] = path.name
                rows.append(record)
    return rows, environments


def config_of(row):
    return (row["runtime"], row["variant"], row["modality"], row["nomutex"], row["topology"], row["workload"], row["batched"])


def summarize(rows):
    """Maps (config, ops total, workers) to statistics. Operation totals stay apart, so rescaled runs never blend."""
    groups = defaultdict(list)
    for r in rows:
        groups[(config_of(r), r["ops"], r["workers"])].append(r)
    summary = {}
    for key, samples in groups.items():
        ms = sorted(sample["ms"] for sample in samples)
        quartiles = statistics.quantiles(ms, n=4) if len(ms) > 1 else [ms[0]] * 3
        loads = [sample["loadavg1"] for sample in samples if sample.get("loadavg1") is not None]
        summary[key] = {
            "median_ms": statistics.median(ms), "q1_ms": quartiles[0], "q3_ms": quartiles[2], "rounds": len(ms),
            "loadavg1": statistics.median(loads) if loads else float("nan"),
            "sources": sorted({sample["source"] for sample in samples}),
        }
    for (config, ops, workers), value in summary.items():
        one = summary.get((config, ops, 1))
        value["throughput"] = ops / value["median_ms"] * 1000
        value["speedup"] = one["median_ms"] / value["median_ms"] if one else float("nan")
        value["speedup_low"] = one["median_ms"] / value["q3_ms"] if one else float("nan")
        value["speedup_high"] = one["median_ms"] / value["q1_ms"] if one else float("nan")
        value["noisy"] = value["q3_ms"] > 2 * value["q1_ms"]
    return summary


def buckets(summary, config):
    """Returns {ops total: {workers: statistics}} for one configuration."""
    found = defaultdict(dict)
    for (key_config, ops, workers), value in summary.items():
        if key_config == config:
            found[ops][workers] = value
    return found


def paired(summary, config_a, config_b, workers):
    """Pairs two configurations at one worker count only where both ran the same operation total."""
    a, b = buckets(summary, config_a), buckets(summary, config_b)
    return [(a[ops][workers], b[ops][workers]) for ops in a if ops in b and workers in a[ops] and workers in b[ops]]


def warn_split_buckets(summary):
    configs = {key[0] for key in summary}
    for config in sorted(configs, key=str):
        found = buckets(summary, config)
        if len(found) > 1:
            print(f"warning: {config} has several operation totals {sorted(found)}, kept apart", file=sys.stderr)


def write_csv(summary):
    with open(RESULTS / "summary.csv", "w", newline="") as file:
        writer = csv.writer(file)
        writer.writerow(["runtime", "variant", "modality", "nomutex", "topology", "workload", "batched", "ops_total",
                         "workers", "rounds", "median_ms", "q1_ms", "q3_ms", "ops_per_second", "speedup_vs_one_worker",
                         "noisy_q3_over_q1_above_2", "median_loadavg1", "sources"])
        for (config, ops, workers) in sorted(summary, key=str):
            v = summary[(config, ops, workers)]
            writer.writerow([*config, ops, workers, v["rounds"], f"{v['median_ms']:.2f}", f"{v['q1_ms']:.2f}",
                             f"{v['q3_ms']:.2f}", f"{v['throughput']:.0f}", f"{v['speedup']:.3f}", v["noisy"],
                             f"{v['loadavg1']:.1f}", " ".join(v["sources"])])


def workload_variants():
    for workload in WORKLOADS + ["keyed_open", "cold_scan"]:
        yield workload, False
        if workload in ("point", "range", "sort", "fts"):
            yield workload, True


def label(workload, batched):
    return f"{workload}{' in one read transaction' if batched else ''}"


def plot_speedup(ax, summary, config, color, label_text, style="-"):
    plotted = False
    for ops, points in buckets(summary, config).items():
        x = sorted(points)
        if 1 not in points:
            continue
        values = [points[w] for w in x]
        ax.plot(x, [v["speedup"] for v in values], style, color=color, label=label_text)
        ax.fill_between(x, [v["speedup_low"] for v in values], [v["speedup_high"] for v in values],
                        color=color, alpha=0.12, linewidth=0)
        for w, v in zip(x, values):
            ax.plot([w], [v["speedup"]], "o", color=color, markerfacecolor="none" if v["noisy"] else color, markersize=4)
        plotted = True
        label_text = None
    return plotted


def finish_axes(ax, title, xlabel=None):
    ax.plot([1, 32], [1, 32], linestyle=":", color="black", linewidth=0.8)
    ax.set_xscale("log", base=2)
    ax.set_yscale("log", base=2)
    ax.set_title(title, fontsize=9)
    ax.grid(True, which="both", linewidth=0.3)


LINEAR = (Line2D([], [], linestyle=":", color="black", linewidth=0.8), "linear scaling")
SPREAD = [
    (Line2D([], [], linestyle="", marker="o", color="dimgray"), "median of rounds"),
    (Line2D([], [], linestyle="", marker="o", color="dimgray", markerfacecolor="none"), "median, q3 above twice q1"),
    (Patch(color="dimgray", alpha=0.25, linewidth=0), "interquartile range"),
]


def line(style, text):
    return Line2D([], [], linestyle=style, color="black"), text


def add_legends(fig, runtimes, lines, bottom=0.11):
    """Explains colors and line styles once for the whole figure, below the panels."""
    colors = [(Line2D([], [], color=COLORS[r], linewidth=3), r) for r in RUNTIMES if r in runtimes]
    fig.tight_layout(rect=(0, bottom, 1, 1))
    fig.legend(*zip(*colors), title="color: runtime", loc="lower left", bbox_to_anchor=(0.02, 0.0),
               ncol=len(colors), fontsize=9, title_fontsize=9, frameon=False)
    fig.legend(*zip(*lines), title="line and marker", loc="lower right", bbox_to_anchor=(0.98, 0.0),
               ncol=min(len(lines), 3), fontsize=9, title_fontsize=9, frameon=False)


def speedup_figures(summary):
    variants = [(w, b) for w, b in workload_variants() if w in WORKLOADS]
    for variant, modality, nomutex, title in MODALITIES:
        fig, axes = plt.subplots(2, 6, figsize=(22, 8.3), sharex=True)
        present = set()
        for ax, (workload, batched) in zip(axes.flat, variants):
            for runtime in RUNTIMES:
                config = (runtime, variant, modality, nomutex, "own_connection", workload, batched)
                if plot_speedup(ax, summary, config, COLORS[runtime], None):
                    present.add(runtime)
            finish_axes(ax, label(workload, batched))
        for ax in axes[-1]:
            ax.set_xlabel("workers")
        for ax in axes[:, 0]:
            ax.set_ylabel("speedup against one worker")
        for ax in axes.flat[len(variants):]:
            ax.axis("off")
        if present:
            fig.suptitle(f"{title}, median of 7 rounds")
            add_legends(fig, present, [line("-", "measured speedup"), LINEAR, *SPREAD])
            fig.savefig(RESULTS / f"speedup-{modality}{'-nomutex' if nomutex else ''}.svg")
        plt.close(fig)


def baseline_figure(summary):
    fig, axes = plt.subplots(1, 2, figsize=(13, 5.6))
    present = set()
    for ax, workload in zip(axes, ["point", "range"]):
        for runtime in RUNTIMES:
            for topology, style in [("own_connection", "-"), ("one_server_worker", "--")]:
                for ops, points in buckets(summary, (runtime, "threadsafe", "memvfs_shared", False, topology, workload, False)).items():
                    x = sorted(points)
                    ax.plot(x, [points[w]["throughput"] for w in x], style, marker="o", markersize=3, color=COLORS[runtime])
                    present.add(runtime)
        ax.set_xscale("log", base=2)
        ax.set_yscale("log")
        ax.set_title(f"{workload}, autocommit, one shared memvfs database", fontsize=10)
        ax.set_xlabel("client workers")
        ax.set_ylabel("operations per second")
        ax.grid(True, which="both", linewidth=0.3)
    add_legends(fig, present, [line("-", "each worker on its own connection"),
                               line("--", "one worker serves all over postMessage")], bottom=0.11)
    fig.savefig(RESULTS / "baseline-postmessage.svg")
    plt.close(fig)


def overhead_figure(summary):
    modalities = ["memvfs_shared", "memvfs_per_worker", "memory_per_worker", "sahpool_per_worker"]
    variants = [(w, b) for w, b in workload_variants() if w in WORKLOADS]
    fig, axes = plt.subplots(1, len(modalities), figsize=(22, 5), sharey=True)
    for ax, modality in zip(axes, modalities):
        width = 0.8 / len(RUNTIMES)
        for i, runtime in enumerate(RUNTIMES):
            ratios = []
            for workload, batched in variants:
                pairs = paired(summary, (runtime, "threadsafe", modality, False, "own_connection", workload, batched),
                               (runtime, "single", modality, False, "own_connection", workload, batched), 1)
                ratios.append(pairs[0][0]["median_ms"] / pairs[0][1]["median_ms"] if pairs else float("nan"))
            ax.bar([j + i * width for j in range(len(variants))], ratios, width, color=COLORS[runtime], label=runtime)
        ax.axhline(1, color="black", linewidth=0.8)
        ax.set_xticks([j + 0.4 for j in range(len(variants))])
        ax.set_xticklabels([f"{w}{' txn' if b else ''}" for w, b in variants], rotation=60, fontsize=8)
        ax.set_title(modality)
    axes[0].set_ylabel("one worker, threadsafe build time over default build time")
    axes[0].legend(title="runtime", fontsize=8)
    fig.suptitle("Single-thread cost, one private connection on one worker in both builds, same operation total")
    fig.tight_layout()
    fig.savefig(RESULTS / "single-thread-cost.svg")
    plt.close(fig)


def comparison_figure(summary, name_a, variant_b, name_b, filename, title, cases):
    fig, axes = plt.subplots(1, len(cases), figsize=(22, 5.6), sharey=True)
    present = set()
    for ax, (modality, workload, batched) in zip(axes, cases):
        for runtime in RUNTIMES:
            for variant, style in [("threadsafe", "-"), (variant_b, "--")]:
                if plot_speedup(ax, summary, (runtime, variant, modality, False, "own_connection", workload, batched),
                                COLORS[runtime], None, style):
                    present.add(runtime)
        finish_axes(ax, f"{label(workload, batched)}, {modality}")
        ax.set_xlabel("workers")
    axes[0].set_ylabel("speedup against one worker")
    fig.suptitle(title)
    add_legends(fig, present, [line("-", name_a), line("--", name_b), LINEAR, *SPREAD], bottom=0.16)
    fig.savefig(RESULTS / filename)
    plt.close(fig)


def sqlcipher_figure(summary):
    fig, axes = plt.subplots(1, 2, figsize=(13, 6.2))
    present = set()
    titles = {"keyed_open": "keyed opens (PBKDF2, 256 000 iterations each)", "cold_scan": "table scans through a 16-page cache"}
    for ax, workload in zip(axes, ["keyed_open", "cold_scan"]):
        for runtime in RUNTIMES:
            plotted = plot_speedup(ax, summary, (runtime, "sqlcipher", "sqlcipher_shared", False, "own_connection", workload, False),
                                   COLORS[runtime], None)
            plot_speedup(ax, summary, (runtime, "sqlcipher-tcache", "sqlcipher_shared", False, "own_connection", workload, False),
                         COLORS[runtime], None, "--")
            if workload == "cold_scan" and plotted:
                plot_speedup(ax, summary, (runtime, "sqlcipher", "memvfs_shared", False, "own_connection", workload, False),
                             COLORS[runtime], None, "-.")
            if plotted:
                present.add(runtime)
        finish_axes(ax, titles[workload])
        ax.set_xlabel("workers, each on its own keyed connection")
    axes[0].set_ylabel("speedup against one worker")
    fig.suptitle("SQLCipher 4.19 with libtomcrypt, one shared memvfs database")
    add_legends(fig, present, [line("-", "keyed, default allocator"), line("--", "keyed, per-thread allocation caches"),
                               line("-.", "same build without a key (scans only)"), LINEAR, *SPREAD], bottom=0.17)
    fig.savefig(RESULTS / "sqlcipher.svg")
    plt.close(fig)


def report(summary, environments):
    lines = ["# Benchmark tables", "",
             "Cell: one-worker median, then speedup at 8 and 32 workers against one worker of the same run and operation total.",
             "Runtimes ran at different times under different outside load, so compare curves within a runtime.", ""]
    lines.append("## Environments\n")
    for stem, env in sorted(environments.items()):
        lines.append(f"- `{stem}`: {env.get('runtime')} {env.get('version', '')[:80]}, cpus {env.get('cpus')}, "
                     f"load {env.get('loadavg', ['?'])[0] if isinstance(env.get('loadavg'), list) else '?'}, {env.get('date', '')}")
    configs = [(v, m, n, "own_connection", t) for v, m, n, t in MODALITIES] + [
        ("threadsafe", "memvfs_shared", False, "one_server_worker", "one serving worker over postMessage, clients on x"),
        ("tcache", "memory_per_worker", False, "own_connection", "private :memory:, per-thread allocation caches"),
        ("tcache", "memvfs_per_worker", False, "own_connection", "memvfs per worker, per-thread allocation caches"),
        ("tcache", "memvfs_shared", False, "own_connection", "memvfs shared, per-thread allocation caches"),
        ("threadsafe-nomemstatus", "memory_per_worker", False, "own_connection", "private :memory:, memstatus off"),
        ("threadsafe-nomemstatus", "memvfs_shared", False, "own_connection", "memvfs shared, memstatus off"),
        ("sqlcipher", "sqlcipher_shared", False, "own_connection", "SQLCipher, one shared keyed database"),
        ("sqlcipher", "memvfs_shared", False, "own_connection", "SQLCipher build, unencrypted shared database"),
        ("sqlcipher-tcache", "sqlcipher_shared", False, "own_connection", "SQLCipher, per-thread allocation caches")]
    for variant, modality, nomutex, topology, title in configs:
        lines.append(f"\n## {title}\n")
        lines.append("workload | " + " | ".join(RUNTIMES))
        lines.append("--- | " + " | ".join("---" for _ in RUNTIMES))
        for workload, batched in workload_variants():
            cells = []
            for runtime in RUNTIMES:
                found = buckets(summary, (runtime, variant, modality, nomutex, topology, workload, batched))
                cell = []
                for ops, points in found.items():
                    if 1 in points and 8 in points and 32 in points:
                        mark = lambda v: "?" if v["noisy"] else ""
                        cell.append(f"{points[1]['median_ms']:.0f} ms, x{points[8]['speedup']:.1f}{mark(points[8])}, "
                                    f"x{points[32]['speedup']:.1f}{mark(points[32])}")
                cells.append(" / ".join(cell))
            if any(cells):
                lines.append(f"{label(workload, batched)} | " + " | ".join(cell or "n/a" for cell in cells))
    lines.append("\n`?` marks a point whose q3 exceeds twice its q1.")
    (RESULTS / "report.md").write_text("\n".join(lines) + "\n")


def main():
    rows, environments = load()
    if not rows:
        sys.exit("no full results in results/")
    summary = summarize(rows)
    warn_split_buckets(summary)
    write_csv(summary)
    speedup_figures(summary)
    baseline_figure(summary)
    overhead_figure(summary)
    comparison_figure(summary, "threadsafe build, std allocator", "tcache", "same build, per-thread allocation caches", "allocator-cache.svg",
                      "Same workloads with only the Rust global allocator changed",
                      [("memory_per_worker", "point", False), ("memory_per_worker", "scan", False),
                       ("memory_per_worker", "fts", False), ("memory_per_worker", "insert", False),
                       ("memvfs_per_worker", "insert", False), ("memvfs_shared", "sort", False)])
    comparison_figure(summary, "threadsafe build, memory statistics on", "threadsafe-nomemstatus", "same build, memory statistics off", "memstatus.svg",
                      "Same workloads with SQLite memory statistics off (Node only)",
                      [("memory_per_worker", "point", False), ("memory_per_worker", "scan", False),
                       ("memory_per_worker", "fts", False), ("memvfs_shared", "point", False),
                       ("memvfs_shared", "insert", False), ("memvfs_shared", "mix", False)])
    sqlcipher_figure(summary)
    report(summary, environments)
    print(f"{len(rows)} rows, {len(summary)} points, written to {RESULTS}")


if __name__ == "__main__":
    main()
