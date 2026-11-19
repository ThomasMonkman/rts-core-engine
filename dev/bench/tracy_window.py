#!/usr/bin/env python3
"""Calls and total time per Tracy zone over a window of sim ticks.

    tracy_window.py FIRST LAST A.tracy [B.tracy] > out.csv

Counts every zone that starts within sim ticks [FIRST, LAST) (tick n is the
n-th "sim tick" zone), from `tracy-csvexport -u`. With two traces (two
builds of the same game), prints both side by side and the change. Times
are totals (zone start to end, children included): Tracy's export has no
self time per event. Needs `tracy-csvexport` (Homebrew's `tracy`).
"""
import collections
import subprocess
import sys


def export(trace, *args):
    return subprocess.Popen(['tracy-csvexport', '-u', '-s', '\t', *args, trace], stdout=subprocess.PIPE, text=True, bufsize=1 << 20)


def window(trace, first, last):
    p = export(trace, '-c', '-f', 'sim tick')
    starts = sorted(int(f[3]) for f in (line.split('\t') for line in p.stdout) if f[0] == 'sim tick')
    p.wait()
    if len(starts) < first:
        sys.exit(f'{trace}: only {len(starts)} sim ticks')
    t0 = starts[first - 1]
    t1 = starts[last - 1] if last <= len(starts) else float('inf')
    acc = collections.defaultdict(lambda: [0, 0])
    p = export(trace)
    next(p.stdout)
    for line in p.stdout:
        f = line.split('\t')
        if len(f) >= 6 and t0 <= int(f[3]) < t1:
            e = acc[f[0]]
            e[0] += 1
            e[1] += int(f[4])
    p.wait()
    return acc


def main():
    if len(sys.argv) not in (4, 5):
        sys.exit(__doc__)
    first, last = int(sys.argv[1]), int(sys.argv[2])
    runs = [window(t, first, last) for t in sys.argv[3:]]
    q = lambda n: '"' + n.replace('"', "'") + '"'
    if len(runs) == 1:
        print('name,calls,total_ns')
        for n, (c, t) in sorted(runs[0].items(), key=lambda kv: -kv[1][1]):
            print(f'{q(n)},{c},{t}')
        return
    a, b = runs
    print('name,calls_a,total_ns_a,calls_b,total_ns_b,delta_ns')
    names = set(a) | set(b)
    rows = [(n, *a.get(n, (0, 0)), *b.get(n, (0, 0))) for n in names]
    for n, ca, ta, cb, tb in sorted(rows, key=lambda r: -abs(r[4] - r[2])):
        print(f'{q(n)},{ca},{ta},{cb},{tb},{tb - ta}')


main()
