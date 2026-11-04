"""Visualize saved native discovery output; never an input to discovery."""
from __future__ import annotations
import argparse
import json
import math
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np
import soundfile as sf
from scipy import signal

from benchmark_discovery import digest, load_json, write_json


def draw(report_file, destination):
    report_file, destination = Path(report_file).resolve(), Path(destination).resolve()
    destination.mkdir(parents=True, exist_ok=False)
    report = load_json(report_file)
    selected = ['control-001', 'control-004', 'control-005', 'control-015', 'control-016', 'control-017']
    names = ['Noise', 'Harmonic flyover', 'Overlapping flyovers',
             'Missing fundamental', 'Interrupted tone', 'Switching stationary tones']
    by_id = {row['case_id']: row for row in report['synthetic']}
    figure, axes = plt.subplots(3, 2, figsize=(13, 10), constrained_layout=True)
    for axis, key, title in zip(axes.flat, selected, names, strict=True):
        row = by_id[key]
        audio, rate = sf.read(row['input']['path'])
        if audio.ndim == 2:
            audio = audio[:, 0]
        factor = math.gcd(rate, 4000)
        audio = signal.resample_poly(audio, 4000 // factor, rate // factor)
        frequencies, times, power = signal.spectrogram(audio, fs=4000, window='hann', nperseg=8192, noverlap=6192, detrend=False)
        mask = (frequencies >= 40) & (frequencies <= 550)
        decibels = 10 * np.log10(np.maximum(power[mask], 1e-20))
        axis.pcolormesh(times, frequencies[mask], decibels, shading='auto', cmap='Greys', vmin=-65, vmax=-30, rasterized=True)
        prediction = load_json(row['prediction'])
        groups = list(dict.fromkeys(event['harmonic_group_id'] for event in prediction['events']))
        colors = {group: plt.cm.tab10(index % 10) for index, group in enumerate(groups)}
        for event in prediction['events']:
            axis.scatter([p['time_s'] for p in event['ridge']], [p['frequency_hz'] for p in event['ridge']],
                         s=3, color=colors[event['harmonic_group_id']], linewidths=0, zorder=3)
        if key == 'control-016':
            axis.axvspan(60, 67, color='#E7B64C', alpha=.25, zorder=2)
        axis.set(title=f'{title}: {len(groups)} proposed source groups', xlabel='Recording time (s)', ylabel='Frequency (Hz)', ylim=(40, 550))
    figure.suptitle('Native Rust discovery on frozen synthetic audio\nColored points are observed ridges; each color represents one grouped source', fontsize=14)
    figure.savefig(destination / 'synthetic_discovery.png', dpi=165)
    plt.close(figure)
    artifacts = ['synthetic_discovery.png']
    if len(report.get('real', [])) == 40:
        rows = report['real']
        figure, axis = plt.subplots(figsize=(14, 5), constrained_layout=True)
        values = [row.get('proposal_groups', 0) for row in rows]
        colors = ['#166F86' if row['status'] == 'passed' and count else '#AEB6BD' for row, count in zip(rows, values, strict=True)]
        axis.bar(range(40), values, color=colors)
        for index, row in enumerate(rows):
            if row['status'] != 'passed':
                axis.scatter(index, 0, marker='x', color='#C53C37', s=60, zorder=3)
        axis.set_xticks(range(40), [row['case_id'].removeprefix('clip-') for row in rows], rotation=90)
        axis.set(xlabel='Opaque recording ID', ylabel='Proposed source groups',
                 title='40 real aircraft recordings: automatic proposal counts\nThese counts are descriptive; event-level precision and recall are not established')
        axis.set_ylim(bottom=0)
        axis.grid(axis='y', alpha=.2)
        figure.savefig(destination / 'real_recording_proposals.png', dpi=165)
        plt.close(figure)
        artifacts.append('real_recording_proposals.png')
    write_json(destination / 'plot_receipt.json', {'input_report': str(report_file), 'input_sha256': digest(report_file),
                'scope': 'Post-run visualization only; no source annotations or upstream proposals used.',
                'artifacts': {name: digest(destination / name) for name in artifacts}})
    return artifacts


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    arguments = parser.parse_args()
    print(json.dumps(draw(arguments.report, arguments.out)))
