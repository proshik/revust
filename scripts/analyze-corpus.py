#!/usr/bin/env python3
"""Summarize saved stage metadata without exposing finding bodies or running providers."""
import argparse
from collections import Counter, defaultdict
import json
import os
from pathlib import Path
import stat
import sys
from helper_common import HelperError, configuration, safe, open_readonly

CATEGORIES = ('findings', 'open_questions', 'pre_existing', 'immaterial')
LEVELS = {'minor': 1, 'major': 2, 'critical': 3}


def document(path):
    """Bounded, regular, no-follow JSON read; corrupt evidence is unavailable."""
    try:
        descriptor = open_readonly(path)
        with os.fdopen(descriptor, 'rb') as source:
            if not stat.S_ISREG(os.fstat(source.fileno()).st_mode):
                return None
            data = source.read((32 << 20) + 1)
        if len(data) > 32 << 20:
            return None
        decoded = json.loads(data)
        return decoded if isinstance(decoded, dict) else None
    except (OSError, ValueError, RecursionError):
        return None


def rows(report):
    result = []
    for category in CATEGORIES:
        values = report.get(category)
        if values is None:
            values = []
        if not isinstance(values, list) or not all(isinstance(value, dict) for value in values):
            raise ValueError('invalid finding category')
        result.extend(values)
    return result


def labels(value):
    return [label for label in value if isinstance(label, str) and label] if isinstance(value, list) else []


def index(report):
    entries = defaultdict(list)
    for finding in rows(report):
        identity = finding.get('id')
        if isinstance(identity, str) and identity:
            entries[identity].append(finding)
    return entries


def transition(before, after, judge):
    old, new = index(before), index(after)
    # Missing identities are not a negative count of removals. Newly introduced
    # IDs remain a separate observation rather than cancelling known disappearances.
    result = {'removed': len(old.keys() - new.keys())}
    if not judge:
        return result
    result.update(promoted=0, demoted=0, demoted_lenses=[], ambiguous=0, unknown_severity=0)
    for identity in old.keys() & new.keys():
        if len(old[identity]) != 1 or len(new[identity]) != 1:
            result['ambiguous'] += 1
            continue
        origin, current = old[identity][0], new[identity][0]
        previous, observed = LEVELS.get(origin.get('severity')), LEVELS.get(current.get('severity'))
        if previous is None or observed is None:
            result['unknown_severity'] += 1
        elif observed > previous:
            result['promoted'] += 1
        elif observed < previous:
            result['demoted'] += 1
            lenses = labels(current.get('lenses'))
            if len(lenses) == 1:
                result['demoted_lenses'].append(lenses[0])
    return result


def measure(task, run, directory):
    found = document(directory / 'stages/1-found.json')
    if found is None:
        return None
    try:
        findings = rows(found)
        lens_counts = defaultdict(Counter)
        naming = defaultdict(Counter)
        for finding in findings:
            lenses = labels(finding.get('lenses'))
            severity = finding.get('severity')
            severity = severity if isinstance(severity, str) and severity else 'unknown'
            for lens in lenses:
                lens_counts[lens][severity] += 1
            sources = labels(finding.get('sources'))
            # A multi-source observation cannot be charged to its first member.
            source = sources[0] if len(sources) == 1 else '?'
            naming[source][str(len(lenses))] += 1
        observation = {'task': task, 'run': run, 'raised': len(findings),
                       'by_lens': {key: dict(value) for key, value in lens_counts.items()},
                       'lens_naming': {key: dict(value) for key, value in naming.items()}}
        final = document(directory / 'findings.json')
        if final is not None:
            rows(final)  # Validate every category even though gating uses actionable rows.
            actionable = final.get('findings') or []
            observation.update(gating=sum(row.get('severity') in ('critical', 'major') for row in actionable),
                               minor=sum(row.get('severity') == 'minor' for row in actionable))
        synthesized = document(directory / 'stages/2-synthesized.json')
        verified = document(directory / 'stages/3-verified.json')
        if synthesized is not None:
            observation['synthesis'] = transition(found, synthesized, False)
        if verified is not None:
            observation['verify'] = transition(synthesized if synthesized is not None else found, verified, True)
        return observation
    except (ValueError, TypeError):
        return None


def totals(observations):
    summary = {'rounds': len(observations), 'raised': 0, 'reports': 0,
               'gating': None, 'minor': None, 'by_lens': {}, 'lens_naming': {}, 'demoted_lenses': {}}
    for stage in ('synthesis', 'verify'):
        summary[stage] = {'rounds': 0, 'removed': None}
        if stage == 'verify':
            summary[stage].update(promoted=None, demoted=None, ambiguous=None, unknown_severity=None)
    for item in observations:
        summary['raised'] += item['raised']
        if 'gating' in item:
            summary['reports'] += 1
            for name in ('gating', 'minor'):
                summary[name] = (summary[name] or 0) + item[name]
        for group in ('by_lens', 'lens_naming'):
            for key, counts in item[group].items():
                destination = summary[group].setdefault(key, {})
                for name, count in counts.items():
                    destination[name] = destination.get(name, 0) + count
        for stage in ('synthesis', 'verify'):
            if stage not in item:
                continue
            destination = summary[stage]
            destination['rounds'] += 1
            for key in destination:
                if key != 'rounds':
                    destination[key] = (destination[key] or 0) + item[stage][key]
            if stage == 'verify':
                for lens in item[stage]['demoted_lenses']:
                    summary['demoted_lenses'][lens] = summary['demoted_lenses'].get(lens, 0) + 1
    return summary


def conclusions(summary, observations):
    messages = [f"Observed {summary['rounds']} stage-bearing rounds; final reports available for {summary['reports']}."]
    if summary['rounds'] < 3:
        messages.append('Too thin for a tuning recommendation; collect more authorized evidence.')
    for stage in ('synthesis', 'verify'):
        values = summary[stage]
        if not values['rounds']:
            messages.append(stage + ': no comparable stage observations.')
        else:
            messages.append(f"{stage}: {values['removed']} input identities absent from output across {values['rounds']} rounds. Merges and drops require separate evidence.")
    if summary['verify']['rounds']:
        values = summary['verify']
        messages.append(f"Known severity changes: {values['promoted']} higher, {values['demoted']} lower; {values['ambiguous']} duplicate-ID comparisons and {values['unknown_severity']} unknown-severity comparisons excluded.")
    messages.append('Counts describe saved observations, not correctness, causation or permission for another review.')
    return messages


def directories(path):
    descriptor = open_readonly(path, directory=True)
    try:
        with os.scandir(descriptor) as entries:
            return sorted(Path(path) / entry.name for entry in entries if entry.is_dir(follow_symlinks=False))
    finally:
        os.close(descriptor)


def scan(tasks):
    observations, unreadable, skipped = [], [], []
    for task in directories(tasks):
        for run in directories(task):
            identity = task.name + '/' + run.name
            marker = document(run / 'manifest.json')
            if marker is None:
                skipped.append(identity)
                continue
            measured = measure(task.name, run.name, run)
            if measured is None:
                unreadable.append(identity)
            else:
                # Old markers can lack timestamps. A nonempty marker alone is
                # never represented here as successful provider completion.
                from datetime import datetime
                try:
                    finished = datetime.fromisoformat(marker.get('finished_at', '').replace('Z', '+00:00'))
                    measured['completion_observed'] = finished.year > 1 and finished.tzinfo is not None
                except (ValueError, AttributeError):
                    measured['completion_observed'] = False
                observations.append(measured)
    return observations, unreadable, skipped


def self_test():
    # Pure authored observation: category moves are not removals; unknown severity
    # cannot become an invented minor rating.
    before = {'findings': [{'id': 'x', 'severity': 'major', 'lenses': ['bugs']}]}
    after = {'pre_existing': [{'id': 'x', 'severity': 'minor', 'lenses': ['bugs']}]}
    observed = transition(before, after, True)
    assert observed['removed'] == 0 and observed['demoted'] == 1
    assert observed['demoted_lenses'] == ['bugs']
    assert transition(before, {'findings': [{'id': 'x'}]}, True)['unknown_severity'] == 1
    print('Corpus observation self-test passed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tasks-dir')
    parser.add_argument('--json', action='store_true')
    parser.add_argument('--test', action='store_true')
    options = parser.parse_args()
    try:
        if options.test:
            self_test()
            return 0
        selected_root = Path(options.tasks_dir) if options.tasks_dir else Path(configuration()['paths']['tasks_dir'])
        # The selected corpus root is an explicit trust anchor; only its
        # descendants are subject to the no-follow traversal boundary.
        tasks = selected_root.resolve(strict=True)
        observations, unreadable, skipped = scan(tasks)
        summary = totals(observations)
        output = {'tasks_dir': str(selected_root), 'totals': summary, 'rounds': observations,
                  'unreadable': unreadable, 'skipped': skipped, 'conclusions': conclusions(summary, observations)}
        if options.json:
            print(json.dumps(output, indent=2))
        else:
            for message in output['conclusions']:
                print(safe(message))
            print(f"Unavailable rounds: {len(unreadable)}; skipped markers: {len(skipped)}")
            for item in observations:
                print(safe(item['task'] + '/' + item['run']) + ': ' + str(item['raised']) + ' raised; gating=' + str(item.get('gating', 'unknown')))
        return 0
    except (HelperError, OSError, ValueError, KeyError, TypeError) as error:
        print('error: ' + safe(error), file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
