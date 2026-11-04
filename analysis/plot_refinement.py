"""Plot completed Rust fits and post-search sensitivity evidence."""
import argparse
import json
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.ticker import FuncFormatter
import numpy as np


def read(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--refinement", type=Path, required=True)
    parser.add_argument("--spectrogram", type=Path, required=True)
    parser.add_argument("--clock-sensitivity", type=Path)
    args = parser.parse_args()
    summary = read(args.refinement / "summary.json")
    overlays = read(args.refinement / "baseline/fine/overlays.json")
    spectrum = np.load(args.spectrogram)
    output = args.refinement / "figures"
    output.mkdir(exist_ok=True)
    plt.rcParams.update({"font.family": "DejaVu Sans", "font.size": 11,
                         "axes.spines.top": False, "axes.spines.right": False})
    fit = overlays["candidates"][0]
    fig, axes = plt.subplots(2, 2, figsize=(15, 8), gridspec_kw={"height_ratios": [3, 1]}, layout="constrained")
    for column, event in enumerate(fit["events"]):
        records = event["samples"]
        times = np.array([p["t_rec_s"] for p in records])
        predicted = np.array([p["predicted_frequency_hz"] for p in records])
        measured = [(p["t_rec_s"], p["observed_frequency_hz"], p["predicted_frequency_hz"])
                    for p in records if p["observed_frequency_hz"] is not None]
        measured = np.array(measured)
        tm = (spectrum["t_s"] >= times.min()) & (spectrum["t_s"] <= times.max())
        fm = (spectrum["f_hz"] >= 65) & (spectrum["f_hz"] <= 195)
        power = spectrum["p_db"][np.ix_(fm, tm)]
        lo, hi = np.percentile(power, [12, 99.5])
        ax = axes[0, column]
        ax.pcolormesh(spectrum["t_s"][tm], spectrum["f_hz"][fm], power,
                      shading="auto", cmap="magma", vmin=lo, vmax=hi, rasterized=True)
        ax.plot(measured[:, 0], measured[:, 1], color="#55e4ea", linewidth=2.5,
                alpha=.9, label="Audio ridge selected before flight matching")
        ax.plot(times, predicted, color="white", linewidth=1.3, label="Rust trajectory model")
        ax.set_title(f"Event {column+1}  |  {event['flight_id']}  |  RMSE {event['recomputed_rmse_hz']:.2f} Hz", loc="left", pad=12)
        ax.set_ylabel("Frequency (Hz)")
        ax.set_ylim(65, 195)
        ax.set_xlim(times.min(), times.max())
        ax.legend(loc="upper right", fontsize=8.5, facecolor="#171220", labelcolor="white", framealpha=.85)
        residual = axes[1, column]
        residual.plot(measured[:, 0], measured[:, 1] - measured[:, 2], color="#236881", linewidth=1.25)
        residual.axhline(0, color="#5b6470", linewidth=.8)
        residual.set_ylabel("Ridge - model (Hz)")
        residual.set_xlabel("Time in downloaded recording (mm:ss)")
        residual.set_xlim(times.min(), times.max())
        residual.grid(axis="y", alpha=.2)
        for item in (ax, residual):
            item.xaxis.set_major_formatter(FuncFormatter(lambda value, pos: f"{int(value)//60:02d}:{int(value)%60:02d}"))
    fig.suptitle("GeoSonnar: refined fit to the downloaded recording", fontsize=20, fontweight="bold", x=.055, ha="left")
    fig.supxlabel("One shared clock; separate fixed tones. Timing shift +5 s reaches its allowed bound.\nSource-informed regional search with a piecewise-linear flight model; no surveyed microphone ground truth.", fontsize=10, color="#4d5663")
    fig.savefig(output / "refined_audio_fit.png", dpi=160)
    plt.close(fig)

    scenarios = summary["scenarios"]
    base = scenarios[0]["fine"]["best"]["observer"]
    latitude = np.deg2rad(base["latitude_deg"])
    lat_scale = 111132.0
    lon_scale = 111320.0 * np.cos(latitude)
    xy = lambda o: ((o["longitude_deg"] - base["longitude_deg"]) * lon_scale,
                    (o["latitude_deg"] - base["latitude_deg"]) * lat_scale)
    fine = read(args.refinement / "baseline/fine/results.json")
    coords = np.array([xy(c["observer"]) for c in fine["candidates"]])
    errors = np.array([c["score"]["combined_rmse_hz"] for c in fine["candidates"]])
    fig, (map_ax, table_ax) = plt.subplots(1, 2, figsize=(14, 7), gridspec_kw={"width_ratios": [1, 1.25]}, layout="constrained")
    mesh = map_ax.scatter(coords[:, 0], coords[:, 1], c=errors, s=80, marker="s", cmap="viridis_r", alpha=.8)
    fig.colorbar(mesh, ax=map_ax, shrink=.65, label="Baseline combined RMSE (Hz)")
    colors = ["#174d70", "#e29527", "#a6414c", "#72549a", "#1c8a78"]
    labels = ["Baseline", "Sound 338 m/s", "Sound 348 m/s", "Aircraft -50 m", "Aircraft +50 m"]
    for index, (scenario, color, label) in enumerate(zip(scenarios, colors, labels)):
        point = scenario["fine"]["best"]
        x, y = xy(point["observer"])
        map_ax.scatter(x, y, color=color, edgecolors="white", linewidths=1.1, s=150, marker="*" if index == 0 else "o", zorder=5)
        map_ax.annotate(str(index+1), (x,y), xytext=(7,6), textcoords="offset points", color=color, fontweight="bold", zorder=6)
    evaluation = summary.get("evaluation", {}).get("reference")
    if evaluation:
        x, y = xy(evaluation)
        map_ax.scatter(x, y, marker="D", color="white", edgecolors="#151c26", linewidths=1.5, s=100, zorder=6)
        map_ax.annotate("Video coordinate", (x,y), xytext=(10,-16), textcoords="offset points", fontsize=9, zorder=7)
    map_ax.set_aspect("equal", adjustable="box")
    map_ax.set_xlabel("East of baseline estimate (m)")
    map_ax.set_ylabel("North of baseline estimate (m)")
    map_ax.set_title("Local results under stated assumptions", loc="left", pad=12)
    map_ax.grid(alpha=.15)
    table_ax.axis("off")
    rows = []
    for index, (scenario, label) in enumerate(zip(scenarios, labels)):
        score = scenario["fine"]["best"]["score"]
        rows.append([f"{index+1}. {label}", f"{score['combined_rmse_hz']:.2f}",
                     f"{scenario['distance_from_baseline_m']:.0f} m",
                     f"{scenario.get('distance_from_evaluation_reference_m', float('nan')):.0f} m",
                     f"{score['shared_shift_s']:+.1f} s"])
    table = table_ax.table(cellText=rows,
                           colLabels=["Scenario", "RMSE\n(Hz)", "From\nbaseline", "From video\ncoordinate", "Shared\nshift"],
                           cellLoc="left", colLoc="left", loc="upper center",
                           colWidths=[.35,.14,.16,.19,.16], bbox=[0,.48,1,.47])
    table.auto_set_font_size(False)
    table.set_fontsize(10)
    for (row, col), cell in table.get_celld().items():
        cell.set_edgecolor("#dce1e6")
        if row == 0:
            cell.set_facecolor("#e9eef2")
            cell.set_text_props(fontweight="bold")
        else:
            cell.set_facecolor("#f6f8fa" if row % 2 else "white")
            if col == 0:
                cell.set_text_props(color=colors[row-1])
    table_ax.text(0,.38,"The same flight pair wins all five runs.", fontsize=14, fontweight="bold", transform=table_ax.transAxes)
    table_ax.text(0,.29,"Baseline: a1dfeb / ace391\n40.876800, -73.524995", fontsize=12, transform=table_ax.transAxes, linespacing=1.6)
    table_ax.text(0,.05,"The timing sweep reaches +5 s in four runs.\nThe alternative pair's local optimum remains at a grid edge.\nThese variations are sensitivity checks, not confidence bounds.\nThe video coordinate is an acoustic result, not surveyed truth.", fontsize=10.5, color="#4d5663", transform=table_ax.transAxes, linespacing=1.7)
    fig.suptitle("Regional replication: refinement and sensitivity", fontsize=20, fontweight="bold", x=.055, ha="left")
    fig.supxlabel("100 m refinement within 600 m of each retained pair's coarse winner, then 25 m around each scenario's winner.\nTerrain and geoid sampled at every point; microphone height assumed 3 m. Reference used only after all searches.", fontsize=10, color="#4d5663")
    fig.savefig(output / "refinement_sensitivity.png", dpi=160)
    plt.close(fig)
    if args.clock_sensitivity:
        clock = read(args.clock_sensitivity / "summary.json")
        clock_best = clock["fine"]["best"]
        clock_results = read(args.clock_sensitivity / "fine/results.json")
        clock_xy = np.array([xy(c["observer"]) for c in clock_results["candidates"]])
        clock_errors = np.array([c["score"]["combined_rmse_hz"] for c in clock_results["candidates"]])
        fig, (left, right) = plt.subplots(1, 2, figsize=(13, 6), gridspec_kw={"width_ratios": [1, 1.25]}, layout="constrained")
        scatter = left.scatter(clock_xy[:, 0], clock_xy[:, 1], c=clock_errors, marker="s", s=70, cmap="viridis_r")
        fig.colorbar(scatter, ax=left, shrink=.7, label="Wider-clock combined RMSE (Hz)")
        x, y = xy(clock_best["observer"])
        left.scatter(x, y, s=250, facecolors="none", edgecolors="#d6651f", linewidths=2, label="Wider clock: +5.25 s", zorder=6)
        left.scatter(0, 0, s=130, marker="*", color="#174d70", edgecolors="white", linewidths=.7, label="Baseline: +5 s", zorder=7)
        if evaluation:
            ex, ey = xy(evaluation)
            left.scatter(ex, ey, s=90, marker="D", color="white", edgecolors="#182330", label="Video acoustic coordinate", zorder=6)
        left.set_xlabel("East of baseline estimate (m)")
        left.set_ylabel("North of baseline estimate (m)")
        left.set_aspect("equal", adjustable="box")
        left.legend(fontsize=8.5, loc="upper left")
        left.grid(alpha=.15)
        right.axis("off")
        base_score = scenarios[0]["fine"]["best"]["score"]
        new_score = clock_best["score"]
        rows = [["Original baseline", "-5 to +5 s", f"{base_score['shared_shift_s']:+.2f} s", f"{base_score['combined_rmse_hz']:.3f}"],
                ["Wider-clock check", "-10 to +10 s", f"{new_score['shared_shift_s']:+.2f} s", f"{new_score['combined_rmse_hz']:.3f}"]]
        table = right.table(cellText=rows, colLabels=["Run", "Allowed shift", "Best shift", "RMSE (Hz)"],
                             colWidths=[.34,.27,.2,.19], cellLoc="left", colLoc="left", bbox=[0,.58,1,.32])
        table.auto_set_font_size(False)
        table.set_fontsize(10)
        for (row, col), cell in table.get_celld().items():
            cell.set_edgecolor("#dce1e6")
            if row == 0:
                cell.set_facecolor("#e9eef2")
                cell.set_text_props(fontweight="bold")
            else:
                cell.set_facecolor("white")
        right.text(0,.44,"The optimum is inside the wider timing range.", fontsize=13, fontweight="bold", transform=right.transAxes)
        right.text(0,.28,f"Same flight pair and same 25 m grid point.\nLocation moved {clock['fine']['distance_from_baseline_m']:.0f} m; distance from video coordinate remains {clock['fine']['distance_from_video_coordinate_m']:.1f} m.", fontsize=11, transform=right.transAxes, linespacing=1.8)
        right.text(0,.08,"343 m/s sound speed; original aircraft heights; microphone at 3 m.\nBoth events retain one shared clock.\nThe original baseline is preserved separately.", fontsize=10, color="#4d5663", transform=right.transAxes, linespacing=1.7)
        fig.suptitle("Wider timing window: same location, interior optimum", fontsize=19, fontweight="bold", x=.055, ha="left")
        fig.get_layout_engine().set(h_pad=.12)
        fig.supxlabel("Explicit follow-up after the baseline reached its +5 s limit.\nConditional regional refinement; no surveyed microphone ground truth.", fontsize=10, color="#4d5663")
        fig.savefig(output / "clock_window_sensitivity.png", dpi=160)
        plt.close(fig)
    print(output.resolve())


if __name__ == "__main__":
    main()
