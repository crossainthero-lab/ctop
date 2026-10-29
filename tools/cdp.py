"""Tiny Chrome DevTools client (stdlib only): launch headless Chrome, evaluate JS, take screenshots."""
import os, json, base64, socket, struct, subprocess, time, urllib.request, shutil, tempfile

class Chrome:
    def __init__(self, url, port=9333, size='1600,900'):
        self.dir = tempfile.mkdtemp(prefix='camview_chrome_', dir=os.environ.get('CAMVIEW_TMP'))
        self.p = subprocess.Popen(['google-chrome', '--headless=new', '--no-sandbox', '--disable-gpu', '--remote-debugging-port=%d' % port, '--user-data-dir=' + self.dir,
                                   '--window-size=' + size, '--no-first-run', url], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(60):
            try:
                tabs = json.load(urllib.request.urlopen('http://127.0.0.1:%d/json' % port, timeout=1)); t = [x for x in tabs if x['type'] == 'page'][0]; break
            except Exception: time.sleep(0.25)
        else: raise RuntimeError('chrome did not start')
        u = t['webSocketDebuggerUrl'][5:]; host, path = u.split('/', 1); h, pt = host.split(':'); self.s = socket.create_connection((h, int(pt)))
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall(('GET /%s HTTP/1.1\r\nHost: %s\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\n\r\n' % (path, host, key)).encode())
        buf = b''
        while b'\r\n\r\n' not in buf: buf += self.s.recv(1024)
        self.rest = buf.split(b'\r\n\r\n', 1)[1]; self.id = 0
    def _recv(self, n):
        while len(self.rest) < n: self.rest += self.s.recv(1 << 20)
        d, self.rest = self.rest[:n], self.rest[n:]; return d
    def _msg(self):
        out = b''
        while True:
            b0, b1 = self._recv(2); ln = b1 & 127
            if ln == 126: ln = struct.unpack('>H', self._recv(2))[0]
            elif ln == 127: ln = struct.unpack('>Q', self._recv(8))[0]
            out += self._recv(ln)
            if b0 & 0x80: return json.loads(out)
    def call(self, method, **params):
        self.id += 1; d = json.dumps(dict(id=self.id, method=method, params=params)).encode(); mask = os.urandom(4); n = len(d)
        hdr = bytes([0x81, 0x80 | (n if n < 126 else 126)]) + (struct.pack('>H', n) if 126 <= n < 65536 else b'') + (struct.pack('>Q', n)[0:0] if n < 65536 else b'\x7f' + struct.pack('>Q', n))
        self.s.sendall(hdr + mask + bytes(c ^ mask[i % 4] for i, c in enumerate(d)))
        while True:
            m = self._msg()
            if m.get('id') == self.id: return m
    def js(self, expr):
        r = self.call('Runtime.evaluate', expression=expr, returnByValue=True, awaitPromise=True)
        if 'exceptionDetails' in r.get('result', {}): raise RuntimeError(r['result']['exceptionDetails'])
        return r['result']['result'].get('value')
    def shot(self, path):
        open(path, 'wb').write(base64.b64decode(self.call('Page.captureScreenshot', format='png')['result']['data']))
    def close(self):
        try: self.call('Browser.close')
        except Exception: pass
        try: self.p.wait(5)
        except Exception: self.p.kill()
        shutil.rmtree(self.dir, ignore_errors=True)
