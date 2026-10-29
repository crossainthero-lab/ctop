#!/usr/bin/env python3 -I
"""End-to-end check of the whole pipeline in SIMULATION. Start `tools/sim_stack.sh` first (fake controller + fake camera
+ `ctop --serve`); this talks to it over the same HTTP API the browser uses. Nothing here touches real hardware."""
import json, os, sys, time, urllib.request
RUN = os.environ.get('CTOP_SIM_DIR', f'/run/user/{os.getuid()}/ctop-sim'); PORT = os.environ.get('CTOP_WEB_PORT', '7896'); CAM = os.environ.get('CAMPORT', '7892')
TOKEN = open(f'{RUN}/data/web_token').read().strip()
def call(cmd, **args):
    r = urllib.request.Request(f'http://127.0.0.1:{PORT}/api/call', json.dumps({'cmd': cmd, 'args': args}).encode(), {'X-CtoP-Token': TOKEN, 'Content-Type': 'application/json'})
    return json.load(urllib.request.urlopen(r, timeout=30))
def state(): return call('state')['result']
def wait(f, what, t=30):
    for _ in range(int(t * 4)):
        s = state()
        if f(s): return s
        time.sleep(0.25)
    raise SystemExit(f'FAIL timeout: {what}: {s["phase"]} {s["message"]} {s["blockers"]}')
fails = []
def check(name, ok, extra=''):
    print(('PASS ' if ok else 'FAIL ') + name, extra); (None if ok else fails.append(name))
def strokes(n=20, y=40): return [[[30 + i * 6, y], [33 + i * 6, y - 8], [36 + i * 6, y]] for i in range(n)]
def pen_bbox():
    d = f'{RUN}/data/ctl_jobs'; f = sorted(os.listdir(d))[-1]; j = json.load(open(f'{d}/{f}'))
    xs = [p[0] for s in j['strokes'] for p in s]; ys = [p[1] for s in j['strokes'] for p in s]; return min(xs), max(xs), min(ys), max(ys)

# 0. security
r = urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{PORT}/api/call', b'{"cmd":"state"}', {'Content-Type': 'application/json'})); check('local API needs no token', json.load(r)['ok'])
r = urllib.request.Request(f'http://127.0.0.1:{PORT}/', headers={'Host': 'evil.example'})
try: urllib.request.urlopen(r); check('foreign Host refused', False)
except urllib.error.HTTPError as e: check('foreign Host refused', e.code == 403)

# 1. paper seen, but not writable until the plotter is at its camera position
call('dismiss'); call('goto_camera')
s = wait(lambda s: s['canWrite'], 'ready to write')
g = s['paper']['geom']
check('paper size recovered', abs(g['widthMm'] - 210) < .5 and abs(g['heightMm'] - 297) < .5, f"{g['widthMm']:.1f}x{g['heightMm']:.1f}")
lock = s['paper']['lockId']

# 2. refusals
call('write', strokes=[[[1, 20], [50, 20]]], key='edge', lockId=lock); s = wait(lambda s: s['phase'] == 'error', 'edge refusal')
check('stroke too close to paper edge refused', 'outside the paper' in s['error']['detail'], s['error']['detail']); call('dismiss')
r = call('write', strokes=strokes(), key='stale', lockId=lock + 5); check('stale paper lock refused', not r['ok'])
call('write', strokes=[[[20, 20], [50, 20]]], key='reach-test', lockId=lock)  # on paper, reachable: will run
wait(lambda s: s['phase'] == 'finished', 'short write'); call('dismiss')

# 2b. unpowered motors: the controller acknowledges moves but nothing moves -> must be reported, not drawn
def cam_post(path, **b): urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{CAM}{path}', json.dumps(b).encode(), {'Host': f'127.0.0.1:{CAM}'}))
call('goto_camera'); wait(lambda s: s['canWrite'], 'ready before freeze test')
cam_post('/api/_freeze', on=True)
r = call('verify_motion'); check('motion check detects "acknowledged but did not move"', not r['ok'] and 'saw nothing move' in r['error'], r.get('error', ''))
cam_post('/api/_freeze', on=False)
call('goto_camera'); wait(lambda s: s['canWrite'], 'ready after freeze test')
r = call('verify_motion'); check('motion check passes when the bed moves', r['ok'] and r['result']['ok'], str(r))
call('goto_camera'); lock = wait(lambda s: s['canWrite'], 'ready after motion test')['paper']['lockId']

# 3. real write: geometry lands on the paper
call('goto_camera'); lock = wait(lambda s: s['canWrite'], 'ready again')['paper']['lockId']  # the bed moves while writing, so each job ends with a fresh lock
r = call('write', strokes=strokes(), key='job1', lockId=lock); check('write accepted', r['ok'], str(r))
r2 = call('write', strokes=strokes(), key='job1', lockId=lock); check('duplicate refused', not r2['ok'], r2.get('error', ''))
wait(lambda s: s['phase'] == 'finished', 'job finish')
x0, x1, y0, y1 = pen_bbox()   # paper at pen (40,6), height 297: page (30..,y=40) -> pen (70.., 303-40=263)
check('pen coordinates = paper placement', abs(x0 - 70) < .1 and abs(x1 - 190) < .1 and abs(y0 - 263) < .1 and abs(y1 - 271) < .1, f'{x0},{x1},{y0},{y1}')
check('cannot write again from the wrong pose', not call('write', strokes=strokes(), key='job2', lockId=lock)['ok'] or wait(lambda s: s['phase'] == 'error', 'pose refusal'))
call('dismiss')

# 4. stop mid-job
call('goto_camera'); lock = wait(lambda s: s['canWrite'], 'ready for stop test')['paper']['lockId']
big = [[[30 + (i % 25) * 6, 30 + (i // 25) * 8], [33 + (i % 25) * 6, 24 + (i // 25) * 8], [36 + (i % 25) * 6, 30 + (i // 25) * 8]] for i in range(300)]
call('write', strokes=big, key='big', lockId=lock); wait(lambda s: s['phase'] == 'writing', 'writing'); time.sleep(1.5); call('stop')
s = wait(lambda s: s['phase'] == 'ready' and s['message'] == 'Stopped.', 'stopped')
check('stop halts the job part-way', 0 < s['progress']['done'] < 300, str(s['progress']))

# 5. paper moves: lock changes, geometry follows
req = urllib.request.Request(f'http://127.0.0.1:{CAM}/api/_move', json.dumps({'dx': 12, 'dy': 0, 'rot': 0}).encode(), {'Host': f'127.0.0.1:{CAM}'})
urllib.request.urlopen(req); call('dismiss'); call('goto_camera')  # parking at the camera pose is opt-in (autoMove) now
s = wait(lambda s: s['paper']['state'] == 'ready' and s['paper']['lockId'] > lock, 'relock after move')
check('moved paper re-detected', abs(s['paper']['geom']['cornersPen'][0][0] - 52) < .3, str(s['paper']['geom']['cornersPen'][0]))
check('old preview cannot be written', not call('write', strokes=strokes(), key='old', lockId=lock)['ok'])

# 6. camera calibration from scratch (clicks are simulated from the fake camera's ground truth; drawing is the fake controller)
truth = json.load(open(f'{RUN}/data/camera_cal.json'))['h']   # image(norm) -> pen
def inv3(m):
    import itertools
    a = [m[0:3], m[3:6], m[6:9]]; det = lambda a: (a[0][0]*(a[1][1]*a[2][2]-a[1][2]*a[2][1]) - a[0][1]*(a[1][0]*a[2][2]-a[1][2]*a[2][0]) + a[0][2]*(a[1][0]*a[2][1]-a[1][1]*a[2][0]))
    d = det(a); c = lambda i, j: (a[(j+1)%3][(i+1)%3]*a[(j+2)%3][(i+2)%3] - a[(j+1)%3][(i+2)%3]*a[(j+2)%3][(i+1)%3]) / d
    return [c(i, j) for i in range(3) for j in range(3)]
def ap(m, p): w = m[6]*p[0] + m[7]*p[1] + m[8]; return [(m[0]*p[0]+m[1]*p[1]+m[2])/w, (m[3]*p[0]+m[4]*p[1]+m[5])/w]
call('cal_clear'); call('dismiss'); call('goto_camera')
s = wait(lambda s: s['paper']['state'] == 'uncalibrated', 'uncalibrated'); check('no calibration -> cannot write', not s['canWrite'])
marks = call('cal_marks_draw')['result']; wait(lambda s: s['phase'] == 'finished', 'marks drawn', 60); call('dismiss'); call('goto_camera')
import random; clicks = [ap(inv3(truth), m) for m in marks]; random.Random(3).shuffle(clicks)
clicks = [[u + random.Random(i).uniform(-3e-4, 3e-4), v + random.Random(i+9).uniform(-3e-4, 3e-4)] for i, (u, v) in enumerate(clicks)]
r = call('cal_solve_clicks', clicks=clicks); check('calibration solved from shuffled clicks', r['ok'], str(r))
s = wait(lambda s: s['canWrite'], 'writable after calibration'); g = s['paper']['geom']
check('calibrated geometry matches ground truth', abs(g['cornersPen'][0][0] - 52) < 1.0 and abs(g['cornersPen'][0][1] - 6) < 1.0, str(g['cornersPen'][0]))
bad = call('cal_solve_clicks', clicks=clicks[:5] + [[0.5, 0.5]]); check('stray click rejected', not bad['ok'], bad.get('error', ''))
print('\nALL PASSED' if not fails else '\nFAILED: ' + ', '.join(fails)); sys.exit(1 if fails else 0)
