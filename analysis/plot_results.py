"""Plot Rust-exported acoustic hypotheses without recomputing their predictions.

Example:
  python analysis/plot_results.py --overlays output/regional/overlays.json \
      --results output/regional/results.json --out output/regional/figures

Optional --reference-file JSON may contain latitude_deg, longitude_deg and label.
It is used only for evaluation and plotting; it cannot affect the Rust search.
"""
from pathlib import Path
import argparse
import json
import math
import textwrap
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from matplotlib.colors import Normalize

ROOT = Path(__file__).resolve().parents[1]
COLORS = ['#65ff9a', '#56c9ff', '#ffb657', '#db96ff', '#ff7878']


def load_json(path):
    return json.loads(Path(path).read_text(encoding='utf-8'))


def distance_m(lat1, lon1, lat2, lon2):
    p1, p2 = math.radians(lat1), math.radians(lat2)
    dp, dl = p2-p1, math.radians(lon2-lon1)
    h = math.sin(dp/2)**2 + math.cos(p1)*math.cos(p2)*math.sin(dl/2)**2
    return 6371008.8 * 2 * math.asin(math.sqrt(min(1.0, max(0.0, h))))


def plot_overlays(overlays, specpath, outdir, count):
    candidates = overlays['candidates'][:count]
    if not candidates:
        raise ValueError('No candidate overlay is available.')
    source = np.load(specpath) if specpath else None
    if source is not None:
        f, t, p = source['f_hz'], source['t_s'], source['p_db']
        baseline = np.median(p, axis=1, keepdims=True)
    fig, axes = plt.subplots(2, 1, figsize=(15, 9), layout='constrained')
    for event_idx, ax in enumerate(axes):
        top_event = candidates[0]['events'][event_idx]
        start, end = top_event['window_recording_s']
        all_predicted = [sample['predicted_frequency_hz'] for c in candidates for sample in c['events'][event_idx]['samples']]
        low = max(5, min(all_predicted)-20)
        high = min(500, max(all_predicted)+20)
        if source is not None:
            fm, tm = (f >= low) & (f <= high), (t >= start) & (t <= end)
            if not fm.any() or not tm.any():
                raise ValueError('Requested overlay does not overlap the source spectrogram.')
            values = (p-baseline)[fm][:, tm]
            image = ax.imshow(values, origin='lower', aspect='auto', extent=[t[tm][0], t[tm][-1], f[fm][0], f[fm][-1]], cmap='magma', vmin=-5, vmax=24)
            fig.colorbar(image, ax=ax, label='Measured power above frequency median (dB)', pad=.015)
        else:
            ax.set_facecolor('#151820')
        observed = [s for s in top_event['samples'] if s['observed_frequency_hz'] is not None]
        ax.scatter([s['t_rec_s'] for s in observed], [s['observed_frequency_hz'] for s in observed], s=7, c='white', alpha=.8, label='Measured ridge samples', zorder=5)
        for rank, candidate in enumerate(candidates):
            event = candidate['events'][event_idx]
            samples = event['samples']
            ax.plot([s['t_rec_s'] for s in samples], [s['predicted_frequency_hz'] for s in samples], color=COLORS[rank % len(COLORS)], lw=1.8 if rank == 0 else 1.2, ls='-' if rank == 0 else '--', label=f'Pair {rank+1}: model, RMSE {event["recomputed_rmse_hz"]:.2f} Hz', zorder=6)
        ax.set(xlim=(start, end), ylim=(low, high), xlabel='Time in downloaded recording (s)', ylabel='Frequency (Hz)', title=f'Event {event_idx+1} — one shared recording clock for both events')
        ax.legend(loc='upper right', fontsize=9, facecolor='#171b25', edgecolor='#596271', labelcolor='white', framealpha=.93)
    fig.suptitle('Measured aircraft bands vs. historical-flight predictions\nLower ridge error is better; overlays are hypotheses, not a verification verdict', fontsize=15)
    target = outdir/'model_overlays.png'
    fig.savefig(target, dpi=160)
    plt.close(fig)
    return str(target)


def plot_map(report, outdir, reference=None):
    ranked = report['candidates']
    if not ranked:
        raise ValueError('No ranked candidates available to plot.')
    # Retain the best independent flight-pair hypothesis at each tested position.
    best_at_position = {}
    for candidate in ranked:
        o = candidate['observer']
        key = (o['latitude_deg'], o['longitude_deg'], o['ellipsoid_height_m'])
        if key not in best_at_position or candidate['score']['combined_rmse_hz'] < best_at_position[key]['score']['combined_rmse_hz']:
            best_at_position[key] = candidate
    selected = list(best_at_position.values())
    lon = np.array([c['observer']['longitude_deg'] for c in selected])
    lat = np.array([c['observer']['latitude_deg'] for c in selected])
    error = np.array([c['score']['combined_rmse_hz'] for c in selected])
    fig, (ax, bars) = plt.subplots(1, 2, figsize=(15, 7), layout='constrained', gridspec_kw={'width_ratios':[1.3, 1]})
    norm = Normalize(vmin=float(error.min()), vmax=max(float(np.percentile(error, 97)), float(error.min()) + 0.1))
    dots = ax.scatter(lon, lat, c=error, cmap='viridis_r', norm=norm, s=35, marker='s', edgecolors='none')
    fig.colorbar(dots, ax=ax, label='Best combined ridge RMSE at each grid point (Hz)', shrink=.8, extend='max' if float(error.max()) > norm.vmax else 'neither')
    pairs = report.get('best_per_flight_pair', [])[:10]
    for idx, candidate in enumerate(pairs):
        o = candidate['observer']
        if idx == 0:
            ax.scatter(o['longitude_deg'],o['latitude_deg'],marker='*',s=220,c='#e23535',edgecolors='white',linewidths=.8,zorder=6,label='Best acoustic hypothesis')
        else:
            ax.scatter(o['longitude_deg'],o['latitude_deg'],marker='x',s=45,c='#252525',linewidths=1.2,zorder=5)
        ax.annotate(str(idx+1),(o['longitude_deg'],o['latitude_deg']),xytext=(6,6),textcoords='offset points',fontsize=9,fontweight='bold',zorder=8,bbox={'facecolor':'white','edgecolor':'none','alpha':.7,'pad':.8})
    ref_distance = None
    if reference is not None:
        ref_lat, ref_lon = float(reference['latitude_deg']), float(reference['longitude_deg'])
        if not (-90 <= ref_lat <= 90 and -180 <= ref_lon <= 180):
            raise ValueError('Reference coordinates are outside valid degree bounds.')
        ax.scatter(ref_lon,ref_lat,marker='+',s=200,c='#e300cc',linewidths=2,zorder=7,label=textwrap.fill(reference.get('label','Reference shown only after ranking'),48))
        first = ranked[0]['observer']
        ref_distance = distance_m(first['latitude_deg'], first['longitude_deg'], ref_lat, ref_lon)
    ax.set(xlabel='Longitude (degrees)',ylabel='Latitude (degrees)',title=f'Bounded observer grid: {len(selected):,} scored positions')
    ax.set_aspect(1/max(.05,math.cos(math.radians(float(np.mean(lat))))))
    ax.ticklabel_format(useOffset=False,style='plain')
    ax.grid(alpha=.18)
    ax.legend(loc='upper right',fontsize=8)
    if pairs:
        y = np.arange(len(pairs))
        e1 = [c['score']['rmse_hz'][0] for c in pairs]
        e2 = [c['score']['rmse_hz'][1] for c in pairs]
        combined = [c['score']['combined_rmse_hz'] for c in pairs]
        bars.barh(y-.18,e1,height=.34,color='#317aa6',label='Event 1')
        bars.barh(y+.18,e2,height=.34,color='#d88831',label='Event 2')
        bars.scatter(combined,y,c='#111111',marker='D',s=25,label='Combined',zorder=5)
        bars.set_yticks(y,[f'Pair {i+1}' for i in range(len(pairs))]); bars.invert_yaxis()
        bars.set(xlabel='Ridge RMSE (Hz), lower is better',title='Best observer for each distinct flight pair')
        bars.legend(loc='lower right',fontsize=9); bars.grid(axis='x',alpha=.18)
    title = 'Acoustic ranking and competing flight pairs'
    if ref_distance is not None:
        title += f'\nBest hypothesis is {ref_distance:,.0f} m from the supplied reference (evaluation only)'
    else:
        title += '\nNo reference coordinate was supplied to this plot'
    fig.suptitle(title,fontsize=15)
    target = outdir/'candidate_map_and_scores.png'
    fig.savefig(target,dpi=160)
    plt.close(fig)
    return str(target), ref_distance


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--overlays',required=True,type=Path)
    parser.add_argument('--results',required=True,type=Path)
    parser.add_argument('--out',required=True,type=Path)
    parser.add_argument('--spectrogram',type=Path,default=ROOT/'output'/'audio-validation'/'channel_0_spectrogram.npz')
    parser.add_argument('--reference-file',type=Path)
    parser.add_argument('--plot-pairs',type=int,default=3,choices=range(1,11))
    args=parser.parse_args()
    args.out.mkdir(parents=True,exist_ok=True)
    overlays,results=load_json(args.overlays),load_json(args.results)
    reference=load_json(args.reference_file) if args.reference_file else None
    # Check identity to avoid displaying overlays beside a different ranking.
    if overlays['candidates'] and results['candidates']:
        a,b=overlays['candidates'][0],results['candidates'][0]
        if a['flight_ids'] != b['score']['flight_ids'] or a['observer'] != b['observer']:
            raise ValueError('Overlay and ranking files describe different leading hypotheses.')
    overlay_path=plot_overlays(overlays,args.spectrogram,args.out,args.plot_pairs)
    map_path,error_m=plot_map(results,args.out,reference)
    manifest={'overlay_figure':overlay_path,'map_figure':map_path,'predictions_source':str(args.overlays),'ranking_source':str(args.results),'reference_file':str(args.reference_file) if args.reference_file else None,'reference_used_to_compute_model_predictions':False,'search_provenance':results.get('provenance',[]),'best_to_reference_distance_m':error_m,'location_verified':False,'interpretation':'Figures expose observed/model agreement and alternative hypotheses. They do not establish independent ground truth or quantify location confidence.'}
    (args.out/'plot_manifest.json').write_text(json.dumps(manifest,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(manifest,indent=2))


if __name__=='__main__':
    main()


