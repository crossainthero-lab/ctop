#!/usr/bin/env python3
"""FAKE controller for CtoP tests: the REAL daemon4.Controller running against FakeMarlin (a pty board), served on a
SEPARATE unix socket. Never touches /dev/ttyUSB*, USB, or the real /run/user/1000/pend.sock.
usage: fake_controller.py --sock /run/user/1000/ctop-fake.sock [--speedup 40]"""
import os, sys, argparse
ROOT = os.environ.get("CTOP_CONTROLLER_DIR", os.path.expanduser("~/ctop-controller"))
sys.path.insert(0, ROOT); sys.path.insert(0, os.path.join(ROOT, "tests"))
sys.dont_write_bytecode = True
import daemon4
from fake_marlin import FakeMarlin

ap = argparse.ArgumentParser(); ap.add_argument("--sock", required=True); ap.add_argument("--speedup", type=float, default=40.0)
a = ap.parse_args()
if os.path.abspath(a.sock) == "/run/user/1000/pend.sock": sys.exit("refusing to serve on the real controller socket")
fake = FakeMarlin(speedup=a.speedup, pos=(100.0, 100.0, 24.5))
fd = os.open(fake.path, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
ctl = daemon4.Controller(daemon4.Port(fd), "fake-pty")
r = ctl.handle("!adopt 100 100 24.5 16 24"); assert r.startswith("ADOPTED"), r
print(ctl.handle("!box 30 280 30 308"), flush=True)
ctl.start_heartbeat()
print("fake controller ready on", a.sock, "board", fake.path, flush=True)
daemon4.serve(ctl, a.sock)
