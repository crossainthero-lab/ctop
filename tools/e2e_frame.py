#!/usr/bin/env python3 -I
"""Camera-free end-to-end check of the fixed-frame workflow, in SIMULATION only: fake controller (real daemon4 on a fake board),
NO camera at all, and a private `ctop --serve` that is stopped and started again to prove the calibration persists.
Needs a debug build (`cargo build`) and `npx vite build`. Never touches the real printer or the real service on 7795."""
import json, os, signal, subprocess, sys, time, urllib.request, shutil
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUN = f'/run/user/{os.getuid()}/ctop-frame-e2e'; PORT = 7897
shutil.rmtree(RUN, ignore_errors=True); os.makedirs(f'{RUN}/data')
SOCK = f'{RUN}/controller.sock'
env = dict(os.environ, CTOP_DATA=f'{RUN}/data', CTOP_WEB_PORT=str(PORT), CTOP_CONTROLLER_SOCK=SOCK, CTOP_DIST=f'{ROOT}/dist')
env.pop('CTOP_WEB_BIND', None)
json.dump({'controllerSocket': SOCK, 'cameraUrl': 'http://127.0.0.1:7899'}, open(f'{RUN}/data/ctop-settings.json', 'w'))  # nothing listens on 7899: no camera
procs = []
def spawn(*a, **k): p = subprocess.Popen(a, env=env, stdout=open(f'{RUN}/log.txt', 'ab'), stderr=subprocess.STDOUT, **k); procs.append(p); return p
fails = []
def check(name, ok, extra=''):
    print(('PASS ' if ok else 'FAIL ') + name, extra); (None if ok else fails.append(name))
def cleanup():
    for p in procs:
        try: p.send_signal(signal.SIGTERM)
        except Exception: pass
spawn('python3', '-I', f'{ROOT}/tools/fake_controller.py', '--sock', SOCK, '--speedup', '40')
for _ in range(60):
    if os.path.exists(SOCK): break
    time.sleep(0.2)
BIN = f'{ROOT}/src-tauri/target/debug/ctop'
def start():
    p = spawn(BIN, '--serve')
    for _ in range(60):
        try: urllib.request.urlopen(f'http://127.0.0.1:{PORT}/', timeout=1); return p
        except Exception: time.sleep(0.2)
    raise SystemExit('service did not start')
svc = start()
def page_token():
    html = urllib.request.urlopen(f'http://localhost:{PORT}/').read().decode()
    return html.split('name="ctop-token" content="')[1].split('"')[0]
TOKEN = page_token()
def call(cmd, **a):
    r = urllib.request.Request(f'http://127.0.0.1:{PORT}/api/call', json.dumps({'cmd': cmd, 'args': a}).encode(), {'X-CtoP-Token': TOKEN, 'Content-Type': 'application/json'})
    return json.load(urllib.request.urlopen(r, timeout=60))
def state(): return call('state')['result']
def wait(f, what, t=60):
    for _ in range(int(t * 4)):
        s = state()
        if f(s): return s
        time.sleep(0.25)
    raise SystemExit(f'FAIL timeout: {what}: {s["phase"]} {s["message"]}')
def last_job():
    d = f'{RUN}/data/ctl_jobs'; f = sorted(os.listdir(d))[-1]; return json.load(open(f'{d}/{f}'))
try:
    check('page carries its own token on localhost', len(TOKEN) >= 32)
    s = state()
    check('camera is not reachable and that is fine', not s['camera']['reachable'] and s['controller']['ready'])
    check('no frame: drawing refused', not s['canDraw'] and not call('draw', strokes=[[[50, 100], [100, 100]]], key='a')['ok'])
    prof = {'name': 'E2E frame', 'created': 0, 'updated': 0, 'pageW': 210, 'pageH': 297, 'originPen': [29.4, 39.1], 'rotation': 0, 'skewDeg': 0, 'surfaceZ': 0, 'marginMm': 8, 'source': 'manual', 'verified': False, 'verifiedAt': None, 'pageToPen': [[1, 0, 0], [0, 1, 0]]}
    check('probe needs explicit confirmation', not call('probe_begin', profile=prof, confirmed=False)['ok'])
    b = call('probe_begin', profile=prof, confirmed=True); check('pen moves over the sheet at travel height', b['ok'] and b['result']['z'] >= 24, str(b))
    r = call('probe_step', dz=-5); check('step larger than 1 mm refused', not r['ok'])
    z = b['result']['z']
    for _ in range(6): r = call('probe_step', dz=-1)
    check('controller floor stops lowering', not r['ok'] or r['result']['z'] >= 16, str(r))
    fin = call('probe_finish', record=True); check('contact height recorded and pen lifted', fin['ok'] and fin['result']['z'] is not None, str(fin))
    prof['surfaceZ'] = 20.3
    check('unverified frame cannot draw', not (call('draw', strokes=[[[50, 100], [100, 100]]], key='b')['ok'] and True) or state()['phase'] != 'writing')
    call('dismiss')
    t = call('frame_test', profile=prof); check('test pattern drawn', t['ok'], str(t))
    wait(lambda s: s['phase'] == 'finished', 'test pattern'); call('dismiss')
    # ── joystick / manual calibration endpoints (fake controller) ──
    call('dismiss')
    p0 = state()['controller']['pos']
    r = call('jog', dx=2.0, dy=0.0, feed=600, t=int(time.time() * 1000)); check('jog moves the pen sideways', r['ok'] and abs(r['result']['pos'][0] - (p0[0] + 2)) < 0.05, str(r))
    check('jog larger than 6 mm refused', not call('jog', dx=7.0, dy=0.0, feed=600)['ok'])
    r = call('jog', dx=1.0, dy=0.0, feed=600, t=int(time.time() * 1000) - 5000); check('stale jog dropped', not r['ok'] and r['error'] == 'stale', str(r))
    for _ in range(40): r = call('jog', dx=0.0, dy=6.0, feed=2400)
    s = state(); check('jog stops at the travel limit (clamped)', r['ok'] and r['result']['clamped'] and s['controller']['pos'][1] <= s['controller']['box'][3] - 0.4, str(s['controller']['pos']))
    for _ in range(10): call('jog', dx=0.0, dy=-6.0, feed=2400)
    r = call('pen_hover'); check('aim height is just above the pen-up height', r['ok'] and abs(r['result']['pos'][2] - r['result']['hover']) < 0.05, str(r))
    check('Z step of more than 1 mm refused', not call('z_nudge', dz=2.0)['ok'])
    r = call('dot'); check('a dot below the controller floor is refused and the pen stays up', not r['ok'] and 'safe limit' in r['error'] and state()['controller']['pos'][2] >= 24, str(r))
    call('set_setting', key='surfaceZ', value=20.3)
    r = call('dot'); check('test dot lowers then returns to aim height', r['ok'] and abs(r['result']['pos'][2] - state()['controller']['pos'][2]) < 0.05 and r['result']['downZ'] >= 13.5, str(r))
    r = call('pen_raise'); check('pen up raises to travel height', r['ok'] and r['result']['pos'][2] >= 24, str(r))
    # marks as the user would make them on a sheet at origin (-9, 45) tilted 0.5 degrees: only reachable points
    import math
    th = math.radians(0.5); at = lambda u, v: (-9 + math.cos(th) * u - math.sin(th) * v, 45 + math.sin(th) * u + math.cos(th) * v)
    marks = [dict(kind='corner', name='br', x=at(210, 0)[0], y=at(210, 0)[1]), dict(kind='edge', name='right', x=at(210, 40)[0], y=at(210, 40)[1]), dict(kind='edge', name='right', x=at(210, 240)[0], y=at(210, 240)[1]),
             dict(kind='edge', name='front', x=at(150, 0)[0], y=at(150, 0)[1]), dict(kind='edge', name='front', x=at(60, 0)[0], y=at(60, 0)[1])]
    r = call('calib_solve', marks=marks); check('four reachable marks fix origin and tilt', r['ok'] and abs(r['result']['origin'][0] + 9) < 0.1 and abs(r['result']['skewDeg'] - 0.5) < 0.05, str(r))
    check('one mark alone is not enough', not call('calib_solve', marks=marks[:1])['ok'])
    r = call('calib_save', marks=marks, name='A4 Frame'); check('calibration saved as A4 Frame', r['ok'] and r['result']['verified'] and r['result']['source'] == 'manual-marks', str(r)[:200])
    prof['name'] = 'E2E frame'; prof['originPen'] = [29.4, 39.1]; prof['skewDeg'] = 0
    prof['verified'] = True; prof['verifiedAt'] = int(time.time())
    check('profile saved', call('frame_save', profile=prof, activate=True)['ok'])
    svc.send_signal(signal.SIGTERM); svc.wait(); svc = start(); TOKEN = page_token()
    s = state(); check('calibration survives a restart', s['frame']['ready'] and s['frame']['active']['name'] == 'E2E frame' and abs(s['frame']['active']['surfaceZ'] - 20.3) < 1e-9)
    names = [p['name'] for p in call('frame_list')['result']['profiles']]; check('the marked calibration is stored too', 'A4 Frame' in names, str(names))
    check('can draw with no camera', s['canDraw'])
    # geometry: paper (x, y top-left origin) -> pen (29.4 + x, 39.1 + 297 - y)
    st = [[[40, 150], [140, 150]], [[40, 160], [140, 160]]]
    r = call('draw', strokes=st, key='g1', stages=[{'label': 'Ruling the page', 'count': 1}, {'label': 'Writing text', 'count': 1}]); check('draw accepted', r['ok'], str(r))
    s = wait(lambda s: s['job'] and s['job']['live'], 'job live')
    check('progress comes from the job', s['job']['total'] == 2 and s['job']['operation'], str(s['job']))
    wait(lambda s: s['phase'] == 'finished', 'finish')
    j = last_job(); pts = [p for s_ in j['strokes'] for p in s_]
    check('pen coordinates match the stored frame', abs(pts[0][0] - 69.4) < .01 and abs(pts[0][1] - 186.1) < .01 and abs(pts[1][0] - 169.4) < .01, str(pts[:2]))
    check('contact plane is the calibrated one', abs(j['params']['surface']['z0'] - 20.3) < 1e-9, str(j['params']['surface']))
    call('dismiss')
    r = call('draw', strokes=[[[2, 100], [100, 100]]], key='e1'); s = wait(lambda s: s['phase'] == 'error', 'edge refusal'); check('outside the safe margin refused', 'outside the paper' in s['error']['detail']); call('dismiss')
    r = call('draw', strokes=[[[50, 20], [100, 20]]], key='e2')
    s = wait(lambda s: s['phase'] == 'error', 'reach refusal') if r['ok'] else None; check('outside the plotter reach refused', s and 'reach' in s['error']['detail'], s and s['error']['detail']); call('dismiss')
    big = [[[30 + (i % 20) * 8, 150 + (i // 20) * 4], [34 + (i % 20) * 8, 150 + (i // 20) * 4]] for i in range(200)]
    call('draw', strokes=big, key='big'); wait(lambda s: s['job'] and s['job']['live'], 'big job')
    time.sleep(1.2); call('pause'); s = wait(lambda s: s['phase'] == 'paused', 'paused'); d0 = s['job']['done']
    check('pause holds the job', 0 < d0 < 200 and s['job']['resumable'], str(s['job']))
    time.sleep(1); check('paused job does not progress', state()['job']['done'] == d0)
    call('resume'); s = wait(lambda s: s['job'] and s['job']['state'] == 'running' and s['job']['done'] > d0, 'resumed')
    call('stop'); s = wait(lambda s: s['phase'] == 'ready' and s['message'] == 'Stopped.', 'cancelled')
    check('cancel stops the job and lifts the pen clear of the paper', s['controller']['pos'][2] >= 20.3 + 2.5, str(s['controller']['pos']))
    # ── vertical control and saved pen heights ──
    call('dismiss'); call('pen_raise'); call('jog', dx=0.0, dy=0.0, feed=600) if False else None
    a = call('pen_hover'); check('aim height reached', a['ok'], str(a))
    t = lambda: int(time.time() * 1000)
    r = call('z_nudge', dz=-1.0, t=t()); z1 = r['result']['pos'][2]; check('a down step lowers Z (physical down = smaller Z)', r['ok'] and z1 < a['result']['pos'][2] - 0.9, str(r))
    r = call('z_nudge', dz=0.5, t=t()); check('an up step raises Z', r['ok'] and r['result']['pos'][2] > z1 + 0.4, str(r))
    for _ in range(4): r = call('z_nudge', dz=-1.0, t=t())
    zc = state()['controller']['pos'][2]
    r = call('z_nudge', dz=-1.0, t=t()); check('near the paper a big step down is refused', (not r['ok']) and '0.35' in r['error'] or zc - 20.3 > 3, str(r) + f' z={zc}')
    r = call('z_nudge', dz=-0.3, t=t()); check('small steps near the paper are allowed', r['ok'], str(r))
    check('stale Z request dropped', call('z_nudge', dz=0.1, t=t() - 3000)['error'] == 'stale')
    check('Z step over 1 mm refused', not call('z_nudge', dz=1.5, t=t())['ok'])
    call('pen_raise'); call('jog', dx=6.0, dy=0.0, feed=2400, t=t())
    for _ in range(30): call('jog', dx=6.0, dy=0.0, feed=2400, t=t())
    call('pen_hover')
    for _ in range(3): call('z_nudge', dz=-1.0, t=t())
    for _ in range(8): r = call('z_nudge', dz=-0.3, t=t())
    check('the pen is not lowered onto the paper off the sheet', not r['ok'] and 'not over the sheet' in r['error'], str(r) + str(state()['controller']['pos']))
    call('pen_raise')
    check('pen heights below the verified minimum refused', not call('pen_save', contact=14.0, depth=0.8, penUp=30)['ok'])
    r = call('pen_save', contact=20.3, depth=0.5, penUp=26); check('pen heights saved', r['ok'] and r['result']['penDepth'] == 0.5 and r['result']['penUpZ'] == 26, str(r)[:160])
    check('saved pen-up height is applied', state()['settings'].get('travelZ') == 26)
    svc.send_signal(signal.SIGTERM); svc.wait(); svc = start(); TOKEN = page_token()
    s = state(); f = s['frame']['active']; check('pen heights survive a restart', f['penDepth'] == 0.5 and f['penUpZ'] == 26 and abs(f['surfaceZ'] - 20.3) < 1e-9 and s['settings'].get('travelZ') == 26, str((f['penDepth'], f['penUpZ'])))
    call('dismiss'); call('draw', strokes=[[[60, 150], [120, 150]]], key='pd1'); wait(lambda s: s['phase'] == 'finished', 'pen-depth job'); call('dismiss')
    check('drawing uses the saved pressure', abs(last_job()['params']['depth'] - 0.5) < 1e-9, str(last_job()['params']))
    pos = state()['controller']['pos']
    check('homing refused without confirmation', not call('home_xy', confirmed=False)['ok'])
finally:
    cleanup()
print('\nALL PASSED' if not fails else '\nFAILED: ' + ', '.join(fails)); sys.exit(1 if fails else 0)
