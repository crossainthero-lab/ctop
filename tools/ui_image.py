#!/usr/bin/env python3 -I
"""Headless-Chrome check of the IMAGE workflow against the simulation stack: upload a dinosaur picture, see the converted
pen strokes, press Draw, wait for Finished. usage: ui_image.py URL_WITH_TOKEN OUTDIR IMAGE"""
import sys, os, time, base64
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); sys.dont_write_bytecode = True
from cdp import Chrome
url, out, img = sys.argv[1], sys.argv[2], sys.argv[3]; os.makedirs(out, exist_ok=True)
c = Chrome(url, port=9345, size='1400,850')
def wait(expr, t=40):
    for _ in range(int(t * 5)):
        if c.js(expr): return True
        time.sleep(0.2)
    raise SystemExit('timeout waiting for ' + expr)
try:
    wait("!!document.querySelector('.tabs')")
    wait("document.querySelector('.bar b') && document.querySelector('.bar b').textContent.length>0")
    c.js("[...document.querySelectorAll('.tabs button')].find(b=>b.textContent.trim()==='Image').click()")
    b64 = base64.b64encode(open(img, 'rb').read()).decode()
    c.js("""(async()=>{const r=await fetch('data:image/png;base64,%s');const f=new File([await r.blob()],'dino.png',{type:'image/png'});
      const dt=new DataTransfer();dt.items.add(f);const i=document.querySelector('input[type=file]');i.files=dt.files;i.dispatchEvent(new Event('change',{bubbles:true}));})()""" % b64)
    wait("document.querySelector('.side').innerText.includes('strokes')")
    print('side:', c.js("document.querySelector('.side').innerText.replace(/\\n+/g,' | ').slice(0,400)"))
    time.sleep(1.0); c.shot(out + '/image_preview.png')
    print('bar:', c.js("document.querySelector('.bar').innerText.replace(/\\n/g,' | ')"))
    print('draw enabled:', c.js("!document.querySelector('button.big.primary').disabled"))
    wait("!document.querySelector('button.big.primary').disabled", 60)
    c.js("document.querySelector('button.big.primary').click()")
    seen = set()
    for _ in range(300):
        t = c.js("document.querySelector('.bar b').textContent"); seen.add(t)
        if t == 'Finished' or 'wrong' in t: break
        time.sleep(1.0)
    print('statuses seen:', sorted(seen)); c.shot(out + '/image_after.png')
    print('final:', c.js("document.querySelector('.bar').innerText.replace(/\\n/g,' | ')"))
finally:
    c.close()
