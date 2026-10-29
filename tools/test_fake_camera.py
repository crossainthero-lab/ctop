import http.client, io, json, os, sys, tempfile, threading, unittest, socket
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
from PIL import Image
import fake_camera as fc

def start(blank=False, **kw):
    s = socket.socket(); s.bind(('127.0.0.1', 0)); port = s.getsockname()[1]; s.close()
    paper = fc.Paper(40, 20, kw.get('rot', 0), 210, 297, 8, 30, blank)
    srv = fc.serve(fc.Fake(paper, settle=0.2), port); threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, port, paper

def get(port, path, host=None):
    c = http.client.HTTPConnection('127.0.0.1', port); c.request('GET', path, headers={'Host': host or '127.0.0.1:%d' % port})
    r = c.getresponse(); return r.status, r.read()

class T(unittest.TestCase):
    def test_all(self):
        for rot in (0, 17):
            srv, port, paper = start(rot=rot)
            try:
                st, b = get(port, '/frame.jpg'); self.assertEqual(st, 200); im = Image.open(io.BytesIO(b)); self.assertEqual(im.size, (1280, 720))
                a = np.asarray(im.convert('RGB')).astype(int); self.assertTrue(((a[..., 2] - a[..., 0] > 25) & (a[..., 2] > 170)).sum() > 500)   # ruled lines present
                an = json.loads(get(port, '/api/analysis')[1]); self.assertTrue(an['paper']['found']); self.assertFalse(an['geometry']['valid'])
                with tempfile.TemporaryDirectory() as d:
                    p = os.path.join(d, 'cal.json'); json.dump(fc.cal_json((270, 305)), open(p, 'w')); cal = json.load(open(p))
                Hn = np.array(cal['h']).reshape(3, 3); self.assertEqual(cal['pose'], [270, 305]); self.assertEqual(len(cal['points']), 6)
                mm = fc.apply(Hn, an['paper']['corners']); exp = paper.corners_mm()
                for e in exp: self.assertLess(np.linalg.norm(mm - e, axis=1).min(), 0.01)
                self.assertEqual(len({int(np.argmin(np.linalg.norm(mm - e, axis=1))) for e in exp}), 4)
                self.assertEqual(get(port, '/api/stats')[0], 200)
                # runtime move
                c = http.client.HTTPConnection('127.0.0.1', port); c.request('POST', '/api/_move', json.dumps({'dx': 25}), {'Host': '127.0.0.1:%d' % port}); self.assertEqual(c.getresponse().status, 200)
                self.assertFalse(json.loads(get(port, '/api/analysis')[1])['paper']['found'])
                import time; time.sleep(0.3); an = json.loads(get(port, '/api/analysis')[1]); self.assertTrue(an['paper']['found'])
                self.assertLess(np.linalg.norm(fc.apply(Hn, an['paper']['corners']) - (exp + [25, 0])[None, :, :].reshape(4, 2)[0], axis=1).min(), 0.01)
                self.assertEqual(get(port, '/api/analysis', host='evil.com')[0], 403)
                self.assertEqual(get(port, '/frame.jpg', host='evil.com:%d' % port)[0], 403)
            finally: srv.shutdown()

    def test_blank(self):
        srv, port, _ = start(blank=True)
        try:
            a = np.asarray(Image.open(io.BytesIO(get(port, '/frame.jpg')[1])).convert('RGB')).astype(int)
            self.assertEqual(((a[..., 2] - a[..., 0] > 25) & (a[..., 2] > 170)).sum(), 0)
        finally: srv.shutdown()

if __name__ == '__main__': unittest.main()
