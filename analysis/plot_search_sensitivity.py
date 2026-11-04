"""Plot bounded search sensitivity without importing evaluation coordinates."""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    report = json.loads(args.report.read_text(encoding='utf-8'))
    if not report.get('execution_passed'):
        raise ValueError('Plot only a completed sensitivity experiment')
    args.output.mkdir(parents=True, exist_ok=False)
    stages = [report['baseline'], *report['stages']]
    labels = ['Original', *[f'+{s["policy"]["frequency_extra_hz"]:g} Hz' for s in report['stages']]]
    if any(not s.get('best') for s in stages):
        raise ValueError('This summary figure requires a winner at every stage')
    origin = stages[0]['best']['observer']
    lat = math.radians(origin['latitude_deg'])
    denominator = 1 - 6.6943799901413165e-3 * math.sin(lat)**2
    lat_m = 6378137 * (1 - 6.6943799901413165e-3) / denominator**1.5 * math.pi / 180
    lon_m = 6378137 / math.sqrt(denominator) * math.cos(lat) * math.pi / 180
    rows = []
    for label, stage in zip(labels, stages, strict=True):
        best = stage['best']; observer = best['observer']; score = best['score']
        rows.append({'stage': label, 'latitude_deg': observer['latitude_deg'],
                     'longitude_deg': observer['longitude_deg'],
                     'east_from_original_m': (observer['longitude_deg']-origin['longitude_deg'])*lon_m,
                     'north_from_original_m': (observer['latitude_deg']-origin['latitude_deg'])*lat_m,
                     'event1_f0_hz': score['fitted_f0_hz'][0], 'event2_f0_hz': score['fitted_f0_hz'][1],
                     'combined_rmse_hz': score['combined_rmse_hz'],
                     'clock_offset_s': score['clock_offset_s'],
                     'winning_boundary': stage['best_diagnostics']['coverage_incomplete']})
    with (args.output/'comparison.csv').open('x', encoding='utf-8', newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=rows[0].keys())
        writer.writeheader(); writer.writerows(rows)
    plt.rcParams.update({'font.family': 'DejaVu Sans', 'font.size': 10,
                         'axes.spines.top': False, 'axes.spines.right': False})
    fig, axes = plt.subplots(1, 3, figsize=(15, 5.6), gridspec_kw={'width_ratios': [1, 1, 1.2]})
    colors = ['#667085', '#3b82f6', '#0891b2', '#155e75']
    east = [r['east_from_original_m'] for r in rows]; north = [r['north_from_original_m'] for r in rows]
    axes[0].plot(east, north, color='#98a2b3', linewidth=1.5, zorder=1)
    for index, row in enumerate(rows):
        axes[0].scatter(east[index], north[index], s=80-index*10, color=colors[index % len(colors)],
                        marker='x' if row['winning_boundary'] else 'o', label=labels[index], zorder=3)
    axes[0].axhline(0, color='#e4e7ec', linewidth=.8); axes[0].axvline(0, color='#e4e7ec', linewidth=.8)
    axes[0].set(xlabel='East from original estimate (m)', ylabel='North from original estimate (m)',
                title='Leading location', aspect='equal')
    pad = max(25, max(max(east)-min(east), max(north)-min(north))*.25)
    axes[0].set_xlim(min(east)-pad, max(east)+pad); axes[0].set_ylim(min(north)-pad, max(north)+pad)
    axes[0].legend(loc='best', fontsize=9, frameon=False)
    shifts = [math.hypot(east[i]-east[i-1], north[i]-north[i-1]) for i in range(1,len(rows))]
    axes[1].bar(range(1,len(rows)), shifts, color=colors[1:len(rows)], width=.55)
    axes[1].axhline(50, color='#b42318', linestyle='--', linewidth=1.2, label='Declared 50 m threshold')
    for x, change in enumerate(shifts, start=1):
        axes[1].annotate(f'{change:.2f} m', (x, change), xytext=(0,5), textcoords='offset points', ha='center')
    axes[1].set_xticks(range(1,len(rows)), [f'{labels[i-1]}\nto {labels[i]}' for i in range(1,len(rows))])
    axes[1].set(ylabel='Movement between estimates (m)', title='Location change (rule: final two transitions)', ylim=(0,max(60,max(shifts)*1.25)))
    axes[1].legend(loc='upper right', fontsize=8, frameon=False)
    pairs = []
    for stage in report['stages']:
        for pair in stage['pairs']:
            key = tuple(pair['flight_ids'])
            if key not in pairs:
                pairs.append(key)
    palette = plt.get_cmap('tab10')
    for index, pair in enumerate(pairs):
        xvalues, errors = [], []
        if pair == tuple(stages[0]['best']['score']['flight_ids']):
            xvalues.append(0); errors.append(rows[0]['combined_rmse_hz'])
        for i, stage in enumerate(report['stages'], start=1):
            item = next((p for p in stage['pairs'] if tuple(p['flight_ids']) == pair), None)
            if item and item.get('fine') and item['fine'].get('best'):
                xvalues.append(i); errors.append(item['fine']['best']['score']['combined_rmse_hz'])
        axes[2].plot(xvalues, errors, '-o', color=palette(index % 10), markersize=4,
                     label=' / '.join(pair))
    axes[2].set_xticks(range(len(labels)), labels)
    axes[2].set(ylabel='Combined frequency RMSE (Hz; log scale)', title='Retained competing pairs', yscale='log')
    axes[2].grid(axis='y', alpha=.2, which='both')
    axes[2].legend(loc='best', fontsize=7, frameon=False)
    fig.suptitle('Bounded frequency and spatial widening', x=.055, ha='left', fontsize=17, fontweight='bold')
    stable = report['stability']['stable']
    subtitle = 'Leading estimate meets the declared stability rule.' if stable else 'Leading estimate does not meet the declared stability rule.'
    if report['stability']['any_retained_pair_coverage_incomplete']:
        subtitle += ' Retained alternatives still reach tested limits.'
    fig.text(.055, .91, subtitle, fontsize=10, color='#344054')
    fig.text(.055, .05, 'Same measured audio points, tracks, clock constraints and physical model. Spatial extents widen with the frequency ranges.\n'
             'Numerical stability is conditional on these inputs; this is not a location-accuracy or global-uniqueness result. Cross marker = boundary-limited.',
             fontsize=9, color='#475467')
    fig.subplots_adjust(left=.065, right=.985, bottom=.23, top=.79, wspace=.34)
    fig.savefig(args.output/'search-sensitivity.png', dpi=160)
    fig.savefig(args.output/'search-sensitivity.pdf')
    plt.close(fig)
    files = [args.report, Path(__file__), *sorted(args.output.iterdir())]
    receipt = {str(path.resolve()): hashlib.sha256(path.read_bytes()).hexdigest() for path in files if path.is_file()}
    (args.output/'provenance.json').write_text(json.dumps({'source_report': str(args.report.resolve()),
        'no_evaluation_reference_input': True, 'sha256': receipt}, indent=2)+'\n', encoding='utf-8')
    print(args.output/'search-sensitivity.png')


if __name__ == '__main__':
    main()
