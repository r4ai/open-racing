"""Writes circuit.gpx: a noisy 10 Hz GPS lap (every ~4 m) of a made-up 5.8 km circuit
with a hairpin and esses, near Suzuka's latitude. Deterministic."""
import math, random, sys

random.seed(1)
# Plan shape in metres: a closed polyline with a hairpin and esses, smoothed.
ctrl0 = [(0, 0), (600, 0), (800, 80), (850, 250), (700, 380), (720, 520), (900, 600),
        (950, 800), (820, 900), (650, 820), (600, 700), (450, 650), (300, 760),
        (120, 700), (60, 500), (-60, 350), (-120, 150)]
ctrl = [(1.6 * x, 1.6 * y) for x, y in ctrl0]
def catmull(p0, p1, p2, p3, t):
    return tuple(0.5 * ((2 * b) + (-a + c) * t + (2 * a - 5 * b + 4 * c - d) * t * t
                        + (-a + 3 * b - 3 * c + d) * t ** 3) for a, b, c, d in zip(p0, p1, p2, p3))
pts = []
n = len(ctrl)
for i in range(n):
    p0, p1, p2, p3 = ctrl[i - 1], ctrl[i], ctrl[(i + 1) % n], ctrl[(i + 2) % n]
    seg = math.dist(p1, p2)
    for k in range(int(seg / 4)):
        pts.append(catmull(p0, p1, p2, p3, k / int(seg / 4)))
lat0, lon0 = 34.843, 136.54
out = ['<?xml version="1.0"?><gpx><trk><trkseg>']
for j, (x, y) in enumerate(pts + [pts[0]]):
    x += random.gauss(0, 0.8); y += random.gauss(0, 0.8)
    lat = lat0 + y / 111195.0
    lon = lon0 + x / (111195.0 * math.cos(math.radians(lat0)))
    ele = 40 + 15 * math.sin(j / len(pts) * 2 * math.pi) + random.gauss(0, 0.2)
    out.append(f'<trkpt lat="{lat:.8f}" lon="{lon:.8f}"><ele>{ele:.2f}</ele></trkpt>')
out.append("</trkseg></trk></gpx>")
open(sys.argv[1], "w").write("\n".join(out))
print(len(pts), "points")
