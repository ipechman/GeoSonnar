"""Run all40 published Livermore recordings through blind automatic MP3 intake.

No annotation, track or UTC is given to the CLI. This tests proposal generation,
not location accuracy or confirmed event-level precision. Requires ffmpeg/bwrap.
"""
import argparse
from pathlib import Path
import collections, json, subprocess, sys, time

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument("source",type=Path,help="Restored zenodo-23424278 directory")
parser.add_argument("--output",type=Path,required=True,help="New unused result directory")
args=parser.parse_args()
repo=Path(__file__).resolve().parents[1]
source=args.source.resolve()
output=args.output.resolve()
clips=sorted((source/'extracted').glob('*/*/audio.wav'))
if len(clips)!=40:
 parser.error("Expected all40 restored audio.wav recordings below source/extracted")
output.mkdir(parents=True,exist_ok=False)
inputs=output/'inputs'; inputs.mkdir()
mapping=[]
for index,path in enumerate(clips):
 key=f'clip-{index:03}'
 target=inputs/f'{key}.mp3'
 process=subprocess.run(['ffmpeg','-v','error','-nostdin','-i',str(path),'-map','0:a:0','-vn','-map_metadata','-1','-c:a','libmp3lame','-b:a','192k',str(target)],capture_output=True)
 mapping.append({'case_id':key,'source_case':path.parent.name,'category':path.parent.parent.name,'encode_returncode':process.returncode})
 if process.returncode: (output/f'{key}-encode-error.txt').write_bytes(process.stderr)
(output/'case-map.json').write_text(json.dumps(mapping,indent=2)+'\n')
print('Staged',len(clips),'audio-only opaque MP3 inputs',flush=True)
python=sys.executable
common=['bwrap','--die-with-parent','--unshare-net','--ro-bind','/','/','--dev','/dev','--proc','/proc','--tmpfs','/tmp','--bind',str(output),str(output),'--tmpfs',str(source),'--ro-bind','/dev/null',str(output/'case-map.json')]
for line in subprocess.check_output(['git','worktree','list','--porcelain'],cwd=repo,text=True).splitlines():
 if line.startswith('worktree '):
  truth=Path(line[9:])/'benchmarks/truth'
  if truth.is_dir(): common.extend(['--tmpfs',str(truth)])
common+=['--setenv','PYTHONPATH',str(repo/'src'),'--setenv','MPLCONFIGDIR','/tmp/mpl','--setenv','HOME','/tmp','--chdir',str(repo)]
rows=[]; started=time.monotonic()
for entry in mapping:
 key=entry['case_id']; jobs=output/key
 if entry['encode_returncode']:
  row={'case_id':key,'status':'failed','reason_code':'encoding_failed','proposal_groups':0}
 else:
  command=common+[python,'-m','dopplergeist.cli','analyze',str(inputs/f'{key}.mp3'),'--jobs',str(jobs),'--max-runtime','180']
  began=time.monotonic()
  with (output/f'{key}-process.log').open('wb') as log:
   process=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT,timeout=210)
  results=list(jobs.glob('*/result.json'))
  if len(results)==1:
   result=json.loads(results[0].read_text())
   row={'case_id':key,'status':result['status'],'reason_code':result.get('reason_code'),'proposal_groups':len({event.get('harmonic_group_id',event['event_id']) for event in result.get('events',[])}),'event_records':len(result.get('events',[])),'elapsed_s':time.monotonic()-began,'result':str(results[0].relative_to(output))}
  else: row={'case_id':key,'status':'failed','reason_code':'missing_result','proposal_groups':0,'exit_code':process.returncode}
 rows.append(row)
 (output/'outcomes.json').write_text(json.dumps(rows,indent=2)+'\n')
 print(key,row['status'],row['reason_code'],'groups',row['proposal_groups'],flush=True)
summary={'scope':'Normal automatic CLI on all40 real positive clips, fixed192kbps MP3, no bands/windows/UTC/flights; discovery only, not location accuracy','source_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),'cases':len(rows),'clips_with_proposals':sum(r['proposal_groups']>0 for r in rows),'clips_without_proposals':sum(r['proposal_groups']==0 for r in rows),'proposal_groups':sum(r['proposal_groups'] for r in rows),'status_counts':dict(collections.Counter(r['status'] for r in rows)),'elapsed_s':time.monotonic()-started,'limitations':['All clips are published aircraft-positive passages; absent/extra groups do not provide event-level miss/false-alarm labels.','Annotations/KML and case mapping masked from workers; no independent microphone/sample-zero UTC evidence; not a location benchmark.']}
(output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2),flush=True)
