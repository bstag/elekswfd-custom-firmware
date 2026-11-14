"""7x7 grid animations for the EleksWFD. frame() returns 7 row-bytes (bit c = column c).

extra dict passed to frame(): {"bands":[7], "cpu":int, "gpu":int, "ram":int,
"level":0..12, "custom":[[r0..r6], ...]}
"""
import math
import random

CENTER = 3
RING = ([(0, c) for c in range(7)] + [(r, 6) for r in range(1, 7)] +
        [(6, c) for c in range(5, -1, -1)] + [(r, 0) for r in range(5, 0, -1)])

HEART = [(1, 1), (1, 2), (1, 4), (1, 5),
         (2, 0), (2, 1), (2, 2), (2, 3), (2, 4), (2, 5), (2, 6),
         (3, 0), (3, 1), (3, 2), (3, 3), (3, 4), (3, 5), (3, 6),
         (4, 1), (4, 2), (4, 3), (4, 4), (4, 5),
         (5, 2), (5, 3), (5, 4), (6, 3)]
HEART_SMALL = [(2, 2), (2, 4), (3, 2), (3, 3), (3, 4), (4, 3)]

ANIMATIONS = ["spin", "bounce", "rain", "wave", "pulse", "sparkle", "scan", "life",
              "audio", "cpu", "mouth", "heart", "custom"]


class GridEngine:
    def __init__(self):
        self.name = None
        self.t = 0
        self.reset()

    def reset(self):
        self.ball = [3.0, 3.0]
        self.vel = [0.55, 0.37]
        self.drops = []
        self.spark = [[0.0] * 7 for _ in range(7)]
        self.life = [[random.random() < 0.32 for _ in range(7)] for _ in range(7)]

    def frame(self, name, extra):
        if name != self.name:
            self.name = name
            self.t = 0
            self.reset()
        self.t += 1
        g = [[0] * 7 for _ in range(7)]
        getattr(self, "a_" + name, self.a_spin)(g, extra)
        return [sum((1 << c) for c in range(7) if g[r][c]) for r in range(7)]

    def a_spin(self, g, e):
        n = len(RING)
        for k in range(5):
            r, c = RING[(self.t - k) % n]
            g[r][c] = 1

    def a_bounce(self, g, e):
        for i in (0, 1):
            self.ball[i] += self.vel[i]
            if self.ball[i] < 0:
                self.ball[i], self.vel[i] = 0, abs(self.vel[i])
            elif self.ball[i] > 6:
                self.ball[i], self.vel[i] = 6, -abs(self.vel[i])
        g[round(self.ball[0])][round(self.ball[1])] = 1

    def a_rain(self, g, e):
        self.drops = [(c, r + 1) for (c, r) in self.drops if r + 1 < 7]
        if random.random() < 0.55:
            self.drops.append((random.randint(0, 6), 0))
        for c, r in self.drops:
            g[r][c] = 1

    def a_wave(self, g, e):
        for c in range(7):
            r = round(3 + 3 * math.sin(self.t * 0.35 + c * 0.7))
            g[max(0, min(6, r))][c] = 1

    def a_pulse(self, g, e):
        rad = (self.t // 2) % 4
        for r in range(7):
            for c in range(7):
                if max(abs(r - CENTER), abs(c - CENTER)) == rad:
                    g[r][c] = 1

    def a_sparkle(self, g, e):
        for r in range(7):
            for c in range(7):
                self.spark[r][c] *= 0.78
        for _ in range(2):
            self.spark[random.randint(0, 6)][random.randint(0, 6)] = 1.0
        for r in range(7):
            for c in range(7):
                if self.spark[r][c] > 0.3:
                    g[r][c] = 1

    def a_scan(self, g, e):
        c = self.t % 7
        for r in range(7):
            g[r][c] = 1

    def a_life(self, g, e):
        if self.t % 3 == 0:
            cur = self.life
            nxt = [[False] * 7 for _ in range(7)]
            alive = 0
            for r in range(7):
                for c in range(7):
                    n = sum(cur[(r + dr) % 7][(c + dc) % 7]
                            for dr in (-1, 0, 1) for dc in (-1, 0, 1) if dr or dc)
                    nxt[r][c] = n == 3 or (cur[r][c] and n == 2)
                    alive += nxt[r][c]
            if alive == 0 or self.t % 90 == 0:
                nxt = [[random.random() < 0.32 for _ in range(7)] for _ in range(7)]
            self.life = nxt
        for r in range(7):
            for c in range(7):
                if self.life[r][c]:
                    g[r][c] = 1

    def a_audio(self, g, e):
        bands = e.get("bands", [0] * 7)
        for c in range(7):
            h = round(bands[c] * 7)
            for r in range(7):
                if (6 - r) < h:
                    g[r][c] = 1

    def a_cpu(self, g, e):
        pct = e.get("cpu", 0) / 100.0
        h = round(pct * 7)
        for r in range(7):
            for c in range(7):
                if (6 - r) < h:                    # fill bottom-up
                    g[r][c] = 1
        if h < 7 and random.random() < pct:        # flicker at the surface
            g[6 - h][random.randint(0, 6)] = 1

    def a_mouth(self, g, e):
        bands = e.get("bands", [0] * 7)
        op = round(min(1.0, max(bands)) * 3)
        if op == 0:
            for c in range(7):
                g[3][c] = 1
        else:
            for c in range(7):
                edge = 1 if c in (0, 6) else 0
                top = max(0, 3 - op + edge)
                bot = min(6, 3 + op - edge)
                g[top][c] = 1
                g[bot][c] = 1

    def a_heart(self, g, e):
        phase = self.t % 16                        # lub-dub beat
        big = phase < 4 or 6 <= phase < 9
        for (r, c) in (HEART if big else HEART_SMALL):
            g[r][c] = 1

    def a_custom(self, g, e):
        frames = e.get("custom") or []
        if not frames:
            return
        rows = frames[self.t % len(frames)]
        for r in range(7):
            for c in range(7):
                if rows[r] & (1 << c):
                    g[r][c] = 1
