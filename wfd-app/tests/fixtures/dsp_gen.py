"""Generate dsp parity fixtures for wfd-app/tests/dsp.rs from the Python reference (app.py).

Writes dsp_blocks.bin (raw int16 LE samples of every block, concatenated) and dsp_expected.json.
Run from wfd-control: uv run -q --with-requirements requirements.txt python -I <this> <fixtures dir>
"""
import json
import sys
import types
from pathlib import Path

sys.path.insert(0, str(Path.cwd()))
import numpy as np  # noqa: E402

import app  # noqa: E402

out_dir = Path(sys.argv[1])
rng = np.random.default_rng(1234)
N = 1024


def tone(freqs, amp, sr, ch, block_i):
    t = (np.arange(N) + block_i * N) / sr
    x = sum(amp * np.sin(2 * np.pi * f * t) for f in freqs)
    return np.repeat(x[:, None], ch, axis=1)


def noise(amp, ch):
    return rng.normal(0, amp, (N, ch))


def make_scenario(ch, sr, plan):
    blocks = []
    for kind, count, args in plan:
        for i in range(count):
            if kind == "silence":
                x = np.zeros((N, ch))
            elif kind == "tone":
                x = tone(args[0], args[1], sr, ch, i)
            elif kind == "noise":
                x = noise(args[0], ch)
            else:  # tone + noise, channels differ a bit
                x = tone(args[0], args[1], sr, ch, i) + noise(args[2], ch)
                x *= np.linspace(0.5, 1.0, ch)[None, :]
            blocks.append(np.clip(np.round(x * 32767), -32768, 32767).astype(np.int16))
    return blocks


def reference(blocks, ch, sr):
    """The audio_reader block from app.py, verbatim except for the device read."""
    app._band_peak[:] = [1e-4] * 7
    energies, bands = [], []
    for b in blocks:
        raw = b.tobytes()
        x = np.frombuffer(raw, np.int16).astype(np.float32) / 32768.0
        if ch > 1:
            x = x.reshape(-1, ch).mean(axis=1)
        energies.append(float(np.sqrt(np.mean(x * x)) + 1e-9))
        mag = np.abs(np.fft.rfft(x * np.hanning(len(x))))
        freqs = np.fft.rfftfreq(len(x), 1 / sr)
        g = [0.0] * 7
        for bi in range(7):
            sel = (freqs >= app._BAND_EDGES[bi]) & (freqs < app._BAND_EDGES[bi + 1])
            e = float(np.sqrt(np.mean(mag[sel] ** 2))) if sel.any() else 0.0
            app._band_peak[bi] = max(e, app._band_peak[bi] * 0.99)
            g[bi] = 0.0 if app._band_peak[bi] < 1e-5 else min(1.0, e / app._band_peak[bi])
        bands.append(g)
    return energies, bands


def levels_run(energy_pairs, gain_pairs):
    """ClockLink.audio_levels from app.py on a fake ClockLink."""
    fake = types.SimpleNamespace(peak={"out": 1e-5, "mic": 1e-5}, disp={"out": 0.0, "mic": 0.0})
    out = []
    for (eo, em), (go, gm) in zip(energy_pairs, gain_pairs):
        app.energy["out"], app.energy["mic"] = eo, em
        lv = app.ClockLink.audio_levels(fake, {"gain_out": go, "gain_mic": gm})
        out.append([lv["out"], lv["mic"]])
    return out


plan = [
    ("silence", 3, None), ("tone", 5, ([440.0], 0.3)), ("mix", 5, ([3000.0, 90.0], 0.2, 0.02)),
    ("noise", 5, (0.1,)), ("silence", 10, None), ("tone", 5, ([70.0, 200.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0], 0.05)),
    ("tone", 3, ([100.0], 0.9)), ("noise", 2, (0.001,)), ("silence", 4, None),
]
scenarios = []
blob = bytearray()
for name, ch, sr, pl in [
    ("stereo48k", 2, 48000, plan),
    ("eight48k", 8, 48000, plan[:3]),
    ("mono44k", 1, 44100, [("tone", 4, ([1000.0], 0.5)), ("mix", 4, ([150.0], 0.3, 0.05)), ("silence", 3, None)]),
]:
    blocks = make_scenario(ch, sr, pl)
    energies, bands = reference(blocks, ch, sr)
    scenarios.append({"name": name, "channels": ch, "rate": sr, "offset": len(blob) // 2,
                      "blocks": len(blocks), "energy": energies, "bands": bands})
    for b in blocks:
        blob += b.tobytes()

# levels: real energies from the scenarios plus a hand-made ramp/fall, varied gains
e_out = scenarios[0]["energy"]
e_mic = scenarios[1]["energy"] + scenarios[2]["energy"]
pairs = [(e_out[i], e_mic[i % len(e_mic)]) for i in range(len(e_out))]
pairs += [(0.001 * k, 0.0005 * (20 - k)) for k in range(20)] + [(0.0, 0.0)] * 15 + [(1e-7, 2e-6)] * 5
pairs += [(0.3, 0.05), (0.0, 0.0), (0.29, 0.06), (0.05, 0.01)] * 4
gains = [(1.0, 1.0) if i < 30 else (2.5, 8.0) if i < 70 else (0.4, 1.0) for i in range(len(pairs))]
levels = levels_run(pairs, gains)

# energies go out as repr() strings: exact round trip (serde_json's default number parsing
# can be 1 ulp off, and energies are compared bit for bit)
for sc in scenarios:
    sc["energy"] = [repr(e) for e in sc["energy"]]
pairs = [[repr(float(a)), repr(float(b))] for a, b in pairs]
(out_dir / "dsp_blocks.bin").write_bytes(bytes(blob))
(out_dir / "dsp_expected.json").write_text(json.dumps(
    {"scenarios": scenarios, "levels": {"energy": pairs, "gain": gains, "expected": levels}}))
print("blocks", len(blob) // 2, "levels", len(levels), "max level", max(max(x) for x in levels))
