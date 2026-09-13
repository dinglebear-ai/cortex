#!/usr/bin/env python3
"""Qualify only compiled mutations with an attributable, baseline-passing libtest."""
import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import signal
import subprocess
import sys


def test_outcome(output, expected, passed):
    """Require exactly the mapped test, its terminal line, and a libtest summary."""
    rows = re.findall(r'^test (\S+) \.\.\. (ok|FAILED|ignored)$', output, re.M)
    state = 'ok' if passed else 'FAILED'
    summary = ('ok. 1 passed; 0 failed;' if passed else 'FAILED. 0 passed; 1 failed;')
    return rows == [(expected, state)] and 'test result: ' + summary in output


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--manifest', required=True)
    parser.add_argument('--source', required=True)
    parser.add_argument('--workspace', required=True)
    parser.add_argument('--killer', required=True, nargs='+')
    parser.add_argument('--build', nargs='+', default=['cargo', 'test', '--lib', '--no-run', '--locked'])
    parser.add_argument('--timeout', type=int, default=1800)
    args = parser.parse_args()
    manifest = json.loads(pathlib.Path(args.manifest).read_text())
    source = pathlib.Path(args.source).resolve()
    out = pathlib.Path(args.workspace).resolve()
    if out.exists() or out == source or source in out.parents:
        raise SystemExit('workspace must be a new directory outside the source tree')
    if not manifest['mutants']:
        raise SystemExit('empty mutation manifest')
    shutil.copytree(source, out, symlinks=True, ignore=shutil.ignore_patterns(
        'target', '.mutation-target', '.mutation-build', '.git', '.worktrees', '.cache', '.full-review*', 'node_modules', '.env', 'data'))

    # Cargo fingerprints use source mtimes, so a shared target can silently
    # reuse the previous checkout's mutant after copytree preserves older mtimes.
    # Both artifact and intermediate directories belong exclusively to this run.
    cargo_target = out / '.mutation-target'
    cargo_build = out / '.mutation-build'
    print(f'isolated Cargo target: {cargo_target}', file=sys.stderr, flush=True)
    print(f'isolated Cargo build: {cargo_build}', file=sys.stderr, flush=True)

    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)

    def run(command, mutant):
        env = {**os.environ, 'CARGO_TARGET_DIR': str(cargo_target),
               'CARGO_BUILD_BUILD_DIR': str(cargo_build), 'MUTANT_ID': mutant['id'],
               'MUTANT_FINGERPRINT': mutant['fingerprint'],
               'MUTANT_KILLER': mutant['killer'],
               'MUTANT_TEST': mutant.get('expected_test', '')}
        # File-backed capture avoids buffering compiler output in process memory.
        import tempfile
        with tempfile.TemporaryFile() as output:
            try:
                child = subprocess.Popen(command, cwd=out, env=env, stdout=output,
                                         stderr=subprocess.STDOUT, start_new_session=True)
                try:
                    code = child.wait(timeout=args.timeout)
                except (KeyboardInterrupt, SystemExit):
                    # Includes interruption: detached children cannot outlive the
                    # qualification driver or race the caller's workspace cleanup.
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    child.wait()
                    raise
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
                    code = 124
            except OSError:
                code = 127
            output.seek(0)
            captured = output.read(2 * 1024 * 1024 + 1)
        if len(captured) > 2 * 1024 * 1024:
            return 125, 'output exceeded evidence budget'
        return code, captured.decode('utf-8', errors='replace')

    baseline = {'id': 'baseline', 'fingerprint': 'unmodified', 'killer': 'baseline'}
    code, output = run(args.build, baseline)
    if code != 0:
        print(json.dumps({'schema': 'cortex-live-mutation-report-v1', 'all_killed': False,
                          'baseline': 'failed', 'baseline_output': output, 'results': []}))
        return 1
    baselines = {}
    for mutant in manifest['mutants']:
        expected = mutant.get('expected_test')
        if expected and expected not in baselines:
            baseline_code, baseline_output = run(args.killer, {**mutant, 'id': 'baseline'})
            baselines[expected] = {'exit': baseline_code, 'output': baseline_output,
                'passed': baseline_code == 0 and test_outcome(baseline_output, expected, True)}
            print(f"baseline {expected}: {'passed' if baselines[expected]['passed'] else 'failed'}", file=sys.stderr, flush=True)
    results = []

    def save_progress():
        progress = {'schema': 'cortex-live-mutation-report-v1', 'baseline': 'built',
                    'baselines': baselines, 'cargo_target_dir': str(cargo_target),
                    'cargo_build_dir': str(cargo_build), 'all_killed': len(results) == len(manifest['mutants'])
                    and all(row['status'] == 'killed' for row in results), 'results': results}
        path = out / '.mutation-report.json'
        temporary = path.with_suffix('.tmp')
        temporary.write_text(json.dumps(progress, separators=(',', ':')))
        temporary.chmod(0o600)
        temporary.replace(path)
        return progress

    save_progress()
    for mutant in manifest['mutants']:
        expected = mutant.get('expected_test')
        if not expected:
            results.append({**mutant, 'status': 'unqualified',
                            'reason': mutant.get('qualification_gap', 'no mapped behavioral test')})
            save_progress()
            continue
        target = (out / mutant['target']).resolve()
        if out not in target.parents or not target.is_file():
            raise SystemExit(f'unsafe target: {mutant["id"]}')
        original = target.read_text()
        if original.count(mutant['needle']) != 1 or mutant['needle'] == mutant['replacement']:
            results.append({**mutant, 'status': 'invalid', 'reason': 'mutation must uniquely change source'})
            save_progress()
            continue
        baseline_result = baselines[expected]
        if not baseline_result['passed']:
            results.append({**mutant, 'status': 'baseline-failed', 'output': baseline_result['output']})
            save_progress()
            continue
        print(f"mutant {mutant['id']}: building", file=sys.stderr, flush=True)
        try:
            target.write_text(original.replace(mutant['needle'], mutant['replacement'], 1))
            changed = hashlib.sha256(target.read_bytes()).hexdigest()
            build_exit, build_output = run(args.build, mutant)
            test_exit, test_output = run(args.killer, mutant) if build_exit == 0 else (None, '')
            if build_exit:
                status = 'invalid'
            elif test_exit == 0 and test_outcome(test_output, expected, True):
                status = 'survived'
            elif test_exit == 101 and test_outcome(test_output, expected, False):
                status = 'killed'
            else:
                status = 'harness-error'
            results.append({**mutant, 'status': status, 'changed_sha256': changed,
                            'build_exit': build_exit, 'exit': test_exit,
                            'build_output': build_output, 'test_output': test_output})
        finally:
            target.write_text(original)
        save_progress()
        print(f"mutant {mutant['id']}: {status}", file=sys.stderr, flush=True)
    report = save_progress()
    print(json.dumps(report, separators=(',', ':')))
    return 0 if report['all_killed'] else 1


if __name__ == '__main__':
    sys.exit(main())
