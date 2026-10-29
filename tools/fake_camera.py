#!/usr/bin/env python3 -I
"""SIMULATED stand-in for the camview hub (frame.jpg, /api/analysis, /api/stats). No real camera involved.
Camera model: exact homography pen-mm -> 1280x720 px, 180-degree rotation + mild perspective.
Corner order (same as cvlib.detect_paper): tl,tr,br,bl = clockwise on screen (y down), starting at the corner nearest the image top-left."""
import argparse, io, json, math, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse
import numpy as np
from PIL import Image

W, H = 1280, 720
S, BED, PERSP = 2.2, (150.0, 150.0), (1.0e-4, 6.0e-5)   # px/mm, bed centre (mm), perspective terms (per mm)
# mm -> px, centred on BED. Camera at the printer rear (as the real one): machine +X -> image left, +Y -> image DOWN (a proper 180-degree rotation, no mirroring).
_Hc = np.array([[-S, 0, W / 2], [0, S, H / 2], [PERSP[0], PERSP[1], 1.0]])
_T = np.array([[1, 0, -BED[0]], [0, 1, -BED[1]], [0, 0, 1.0]])
HPX = _Hc @ _T                      # pen mm -> px
HINV = np.linalg.inv(HPX)           # px -> pen mm

def apply(M, pts):
    p = np.c_[np.asarray(pts, float), np.ones(len(pts))] @ M.T
    return p[:, :2] / p[:, 2:3]

class Paper:
    def __init__(s, x, y, rot, w, h, line, first, blank):
        s.x, s.y, s.rot, s.w, s.h, s.line, s.first, s.blank = x, y, rot, w, h, line, first, blank
        s.moved_at = None; s.bed_dy = 0.0; s.frozen = False   # frozen = the bed does not follow the controller (simulates unpowered motors)
    def corners_mm(s):
        c, n = math.cos(math.radians(s.rot)), math.sin(math.radians(s.rot))
        loc = np.array([[0, 0], [s.w, 0], [s.w, s.h], [0, s.h]], float)
        return loc @ np.array([[c, n], [-n, c]]) + [s.x, s.y + s.bed_dy]
    def shift(s, dx=0, dy=0, rot=0):
        s.x += dx; s.y += dy; s.rot += rot; s.moved_at = time.monotonic()
    def corners_px_ordered(s):
        """-> (px corners tl,tr,br,bl in detector order, mm corners in same order)"""
        mm = s.corners_mm(); px = apply(HPX, mm)
        e1, e2 = px[1] - px[0], px[2] - px[1]; cr = e1[0] * e2[1] - e1[1] * e2[0]
        idx = [0, 1, 2, 3] if cr > 0 else [0, 3, 2, 1]
        k0 = int(np.argmin([px[j][0] + px[j][1] for j in idx])); o = [idx[(k0 + k) % 4] for k in range(4)]
        return px[o], mm[o]
    def render(s):
        rng = np.random.default_rng(1234)
        img = np.empty((H, W, 3), float); img[:] = (52, 54, 56)
        for _ in range(400):                                    # bed speckles
            yy, xx = rng.integers(0, H), rng.integers(0, W); img[yy, xx] = rng.integers(30, 90)
        g = np.mgrid[0:H, 0:W].reshape(2, -1)[::-1].T + 0.5      # (x,y) pixel centres
        mm = apply(HINV, g); c, n = math.cos(math.radians(s.rot)), math.sin(math.radians(s.rot))
        d = mm - [s.x, s.y + s.bed_dy]; u = d[:, 0] * c + d[:, 1] * n; v = -d[:, 0] * n + d[:, 1] * c   # paper frame
        ins = (u >= 0) & (u <= s.w) & (v >= 0) & (v <= s.h)
        out = img.reshape(-1, 3); out[ins] = (236, 236, 230)
        if not s.blank:
            def blend(dist_mm, colour, mask):
                a = np.clip((0.9 - dist_mm * S) / 0.9, 0, 1) * 0.95 * mask       # soft ~0.25 mm line
                out[:] = out * (1 - a[:, None]) + np.array(colour, float) * a[:, None]
            k = np.round((v - s.first) / s.line); vl = s.first + np.clip(k, 0, None) * s.line
            blend(np.abs(v - vl), (150, 175, 215), ins & (v >= s.first - 0.5))
            blend(np.abs(u - 25.0), (205, 70, 80), ins)
        out += rng.integers(-2, 3, out.shape)
        return Image.fromarray(np.clip(out, 0, 255).astype(np.uint8).reshape(H, W, 3))

class Fake:
    def __init__(s, paper, settle=1.0):
        s.paper, s.settle, s.lock, s.cache = paper, settle, threading.Lock(), None
    def key(s): p = s.paper; return (p.x, p.y, p.rot, round(p.bed_dy, 2))
    def jpg(s):
        with s.lock:
            if not s.cache or s.cache[0] != s.key():
                b = io.BytesIO(); s.paper.render().save(b, 'JPEG', quality=95, subsampling=0); s.cache = (s.key(), b.getvalue())
            return s.cache[1]
    def analysis(s):
        p, t = s.paper, time.time()
        base = dict(paper={'found': False, 'reason': 'paper moved, re-detecting', 't': t}, geometry=dict(valid=False, source=None, invalid_reason='fake camera: no reference'),
                    motion={'state': 'fake', 't': t}, frame_size=[W, H], analysis_size=[W, H], fake=True)
        if p.moved_at is not None and time.monotonic() - p.moved_at < s.settle: return base
        px, _ = p.corners_px_ordered(); ln = [float(np.hypot(*(px[(k + 1) % 4] - px[k]))) for k in range(4)]
        vis = [bool(1 <= q[0] <= W - 1 and 1 <= q[1] <= H - 1) for q in px]
        base['paper'] = dict(found=True, corners_px=px.tolist(), corners=(px / [W, H]).tolist(), corner_visible=vis, edge_visible_frac=[1.0] * 4,
                             edge_len_px=ln, width_px=(ln[0] + ln[2]) / 2, height_px=(ln[1] + ln[3]) / 2, fill=0.97, area_frac=float(abs(poly_area(px)) / (W * H)),
                             analysis_size=[W, H], width_px_frame=(ln[0] + ln[2]) / 2, height_px_frame=(ln[1] + ln[3]) / 2, t=t)
        return base

def poly_area(q): return 0.5 * sum(q[i][0] * q[(i + 1) % 4][1] - q[(i + 1) % 4][0] * q[i][1] for i in range(4))

def make_handler(fake, port):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.0'
        def log_message(self, *a): pass
        def send(self, code, body, ctype='application/json'):
            if not isinstance(body, (bytes, bytearray)): body = json.dumps(body).encode()
            self.send_response(code); self.send_header('Content-Type', ctype); self.send_header('Content-Length', str(len(body))); self.send_header('Cache-Control', 'no-store'); self.end_headers(); self.wfile.write(body)
        def host_ok(self):
            h = self.headers.get('Host', ''); name, _, pt = h.rpartition(':')
            ok = (name in ('127.0.0.1', 'localhost') and pt == str(port)) or (h in ('127.0.0.1', 'localhost') and False)
            o = self.headers.get('Origin')
            return ok and not (o and urlparse(o).hostname not in ('127.0.0.1', 'localhost'))
        def do_GET(self):
            if not self.host_ok(): return self.send(403, {'error': 'bad Host/Origin'})
            p = urlparse(self.path).path
            if p == '/frame.jpg': return self.send(200, fake.jpg(), 'image/jpeg')
            if p == '/api/analysis': return self.send(200, fake.analysis())
            if p == '/api/stats': return self.send(200, {'fps': 30, 'fake': True, 'camera': {'state': 'streaming (fake)', 'frame_age_s': 0.05}})
            self.send(404, {'error': 'not found'})
        def do_POST(self):
            if not self.host_ok(): return self.send(403, {'error': 'bad Host/Origin'})
            try:
                n = int(self.headers.get('Content-Length') or 0); b = json.loads(self.rfile.read(n) or b'{}') if n < 10000 else {}
                if urlparse(self.path).path == '/api/clean_ref': return self.send(200, {'ok': True})
                if urlparse(self.path).path == '/api/_freeze': fake.paper.frozen = bool(b.get('on')); return self.send(200, {'ok': True})
                if urlparse(self.path).path != '/api/_move': return self.send(404, {'error': 'not found'})
                fake.paper.shift(float(b.get('dx', 0)), float(b.get('dy', 0)), float(b.get('rot', 0))); self.send(200, {'ok': True})
            except (ValueError, TypeError) as e: self.send(400, {'error': repr(e)})
    return Handler

def serve(fake, port):
    srv = ThreadingHTTPServer(('127.0.0.1', port), make_handler(fake, port)); srv.daemon_threads = True; return srv

def cal_json(pose):
    h = np.linalg.inv(HPX) @ np.diag([W, H, 1.0]); h = h / h[2, 2]       # normalized image -> pen mm
    pts = [(0.15, 0.2), (0.5, 0.2), (0.85, 0.2), (0.15, 0.8), (0.5, 0.8), (0.85, 0.8)]
    pen = apply(HINV, [(u * W, v * H) for u, v in pts])
    return dict(version=1, h=h.flatten().tolist(), pose=list(pose), rms_mm=0.0, created=int(time.time()),
                points=[{'img': list(a), 'pen': b.tolist()} for a, b in zip(pts, pen)])

def main():
    a = argparse.ArgumentParser()
    a.add_argument('--port', type=int, default=7892)
    a.add_argument('--paper-x', type=float, default=40); a.add_argument('--paper-y', type=float, default=20); a.add_argument('--paper-rot', type=float, default=0)
    a.add_argument('--paper-w', type=float, default=210); a.add_argument('--paper-h', type=float, default=297)
    a.add_argument('--line-mm', type=float, default=8); a.add_argument('--first-line-mm', type=float, default=30)
    a.add_argument('--blank', action='store_true'); a.add_argument('--move-after', type=float, default=0)
    a.add_argument('--ctl-sock', help='follow this controller: the BED moves with machine Y, so the paper appears to shift when the pen moves in Y'); a.add_argument('--write-cal'); a.add_argument('--pose', type=float, nargs=2, default=[270, 305]); a.add_argument('--selftest-geometry', action='store_true')
    a = a.parse_args()
    paper = Paper(a.paper_x, a.paper_y, a.paper_rot, a.paper_w, a.paper_h, a.line_mm, a.first_line_mm, a.blank)
    if a.write_cal: json.dump(cal_json(a.pose), open(a.write_cal, 'w'), indent=1); print('wrote', a.write_cal); return
    if a.selftest_geometry:
        px, mm = paper.corners_px_ordered()
        for n, m, p in zip(('tl', 'tr', 'br', 'bl'), mm, px): print('%s  pen mm (%.3f, %.3f)  px (%.2f, %.2f)' % (n, *m, *p))
        return
    fake = Fake(paper)
    if a.move_after > 0: threading.Timer(a.move_after, lambda: paper.shift(dx=25)).start()
    if a.ctl_sock:
        import socket
        def follow():
            while True:
                try:
                    c = socket.socket(socket.AF_UNIX); c.settimeout(2); c.connect(a.ctl_sock); c.sendall(b'!status\n'); c.shutdown(socket.SHUT_WR)
                    y = json.loads(c.makefile().read())['pos'][1]; c.close(); dy = -(y - a.pose[1])
                    if not paper.frozen and abs(dy - paper.bed_dy) > 0.02: paper.bed_dy = dy; paper.moved_at = time.monotonic()
                except Exception: pass
                time.sleep(0.15)
        threading.Thread(target=follow, daemon=True).start()
    print('fake camera on http://127.0.0.1:%d' % a.port, flush=True)
    try: serve(fake, a.port).serve_forever()
    except KeyboardInterrupt: pass

if __name__ == '__main__': main()
