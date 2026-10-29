#!/usr/bin/env python3 -I
"""Drive the browser UI in HEADLESS Chrome (no window on your desktop) against the simulation stack (tools/sim_stack.sh).
usage: ui_check.py URL_WITH_TOKEN OUTDIR [--write]"""
import sys, os, time, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); sys.dont_write_bytecode = True
from cdp import Chrome
url, out = sys.argv[1], sys.argv[2]; os.makedirs(out, exist_ok=True)
c = Chrome(url, port=9344, size='1400,850')
def wait(expr, t=15):
    for _ in range(int(t * 5)):
        if c.js(expr): return True
        time.sleep(0.2)
    raise SystemExit('timeout waiting for ' + expr)
try:
    wait("!!document.querySelector('textarea')")
    wait("document.querySelector('.bar b') && document.querySelector('.bar b').textContent.length>0")
    c.shot(out + '/1_empty.png')
    text = "Dear diary,\\n\\nToday I tested CtoP with a pencil plotter. The text wraps to the page width and stays inside the margins."
    c.js("(()=>{const t=document.querySelector('textarea');t.value=\"%s\";t.dispatchEvent(new Event('input',{bubbles:true}));})()" % text)
    time.sleep(1.5); c.shot(out + '/2_typed.png')
    print('bar:', c.js("document.querySelector('.bar').innerText"))
    print('write enabled:', c.js("!document.querySelector('button.big.primary').disabled"))
    if '--write' in sys.argv:
        c.js("document.querySelector('button.big.primary').click()")
        for i in range(6):
            time.sleep(1.0); print(i, c.js("document.querySelector('.bar').innerText.replace(/\\n/g,' | ')"))
        c.shot(out + '/3_after.png')
finally:
    c.close()
