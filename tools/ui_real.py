#!/usr/bin/env python3 -I
"""Headless-Chrome run of the real browser UI against a RUNNING ctop --serve (real plotter!). Presses Write and follows the status.
usage: ui_real.py URL_WITH_TOKEN OUTDIR"""
import sys, os, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); sys.dont_write_bytecode = True
from cdp import Chrome
url, out = sys.argv[1], sys.argv[2]; os.makedirs(out, exist_ok=True)
c = Chrome(url, port=9346, size='1400,850')
def wait(expr, t=60):
    for _ in range(int(t * 5)):
        if c.js(expr): return True
        time.sleep(0.2)
    raise SystemExit('timeout waiting for ' + expr)
try:
    wait("!!document.querySelector('.bar b')")
    wait("!document.querySelector('button.big.primary').disabled", 90)
    c.shot(out + '/real_preview.png'); print('bar:', c.js("document.querySelector('.bar').innerText.replace(/\\n/g,' | ')"), flush=True)
    c.js("document.querySelector('button.big.primary').click()")
    seen = []
    for _ in range(900):
        t = c.js("document.querySelector('.bar').innerText.replace(/\\n/g,' | ')")
        if not seen or seen[-1] != t: seen.append(t); print(t, flush=True)
        if t.startswith('Finished') or 'wrong' in t: break
        time.sleep(1.0)
    time.sleep(2); c.shot(out + '/real_after.png')
finally:
    c.close()
