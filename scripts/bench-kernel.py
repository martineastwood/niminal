#!/usr/bin/env python3
"""Alternate kernel interpreter trials; compare exact rendered bytes and report medians."""
import argparse
import csv
import os
from pathlib import Path
import statistics
import shutil
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--rounds', type=int, default=3)
parser.add_argument('--blocks', type=int, default=2048)
parser.add_argument('--trials', type=int, default=7)
parser.add_argument('--output', type=Path)
parser.add_argument('--reference', type=Path)
args = parser.parse_args()
if min(args.rounds, args.blocks, args.trials) < 1:
    parser.error('rounds, blocks and trials must be positive')
os.chdir(Path(__file__).resolve().parent.parent)
args.output = args.output or Path('target/kernel-bench')
args.output.mkdir(parents=True, exist_ok=True)
subprocess.run(['cargo', 'build', '--release', '-p', 'niminal-lang', '--example',
                'bench_kernel'], check=True)
binary = args.output / 'bench-current'
shutil.copy2('target/release/examples/bench_kernel', binary)
modes = ['current']
binaries = {'current': binary}
if args.reference:
    modes.insert(0, 'reference')
    binaries['reference'] = args.reference.resolve()
results = {}
for round_index in range(args.rounds):
    for mode in modes if round_index % 2 == 0 else list(reversed(modes)):
        audio = args.output / f'{mode}.bin'
        env = dict(os.environ, NIMINAL_JIT='0',
                   NIMINAL_BENCH_AUDIO=str(audio))
        run = subprocess.run([str(binaries[mode].resolve()), str(args.blocks),
                              str(args.trials)], env=env, capture_output=True, text=True, check=True)
        if run.stderr:
            print(run.stderr, flush=True)
        (args.output / f'{mode}-{round_index}.csv').write_text(run.stdout)
        for row in csv.DictReader(run.stdout.splitlines()):
            results.setdefault(row['case'], {}).setdefault(mode, []).append(row)
        print(f'Completed round {round_index + 1}: {mode}', flush=True)
    reference = (args.output / f'{modes[0]}.bin').read_bytes()
    for mode in modes[1:]:
        if reference != (args.output / f'{mode}.bin').read_bytes():
            raise RuntimeError(f'{mode}: rendered audio differs from interpreter')
    print('All rendered audio is bitwise identical', flush=True)
summary = args.output / 'summary.csv'
with summary.open('w', newline='') as out:
    writer = csv.writer(out)
    writer.writerow(['case', 'mode', 'ns_frame', 'speedup', 'compile_ms', 'p99_ns_block', 'max_ns_block', 'first_compile_ms'])
    for name, sides in results.items():
        baseline = statistics.median(float(r['median_ns_frame']) for r in sides[modes[0]])
        for mode in modes:
            median = lambda key: statistics.median(float(r[key]) for r in sides[mode])
            ns = median('median_ns_frame')
            writer.writerow([name, mode, f'{ns:.1f}', f'{baseline/ns:.2f}',
                             f'{median("compile_ms"):.3f}', f'{median("p99_ns_block"):.0f}',
                             f'{median("max_ns_block"):.0f}', f'{median("first_compile_ms"):.3f}'])
print(summary.read_text())
