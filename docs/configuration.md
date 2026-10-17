# Configuration and reusable profiles

Default roots are ~/.config/revust and caller-cwd/.revust. Explicit --config-dir
and --tasks-dir accept arbitrary paths. Discovery performs no writes; equivalent
user/project directory aliases collapse into one layer. --workdir does not change
which project configuration was selected. No old-root or environment fallback is
implemented.

INI files named config merge per field: defaults, user, project, invocation.
The first assignment in a winning file is used, repeated invocation values use
the last. Unknown keys and malformed assignments in an active assignment file are
errors even when masked. Comment/header-only templates remain refreshable.
Hyphenated CLI names and documented CamelCase INI aliases are accepted.

| Setting | Default |
| --- | --- |
| idle-timeout / hard-timeout | 2m / 20m |
| stagger-delay | 30s |
| max-parallel | 4 |
| verify-groups / verify-group-by | 6 / dir |
| tasks-dir | ./.revust/tasks |
| auto-exit | 0s |
| profile | comprehensive |
| codex-sandbox | read-only |

Explicit zero remains a value. Extensions impose their own positive/range rules.
Durations accept signed unit sequences ns/us/ms/s/m/h including fractional units;
inspection can represent negative ordinary values that execution cannot use.

Prompt files are whole-document overlays: embedded, user, project. A malformed
shadowed file does not poison the winner. prompts/profiles/NAME.md contains YAML
frontmatter (description, model, agents, stages) and a Markdown body. Lenses live
under lenses/; stage documents are prompts/synthesis.md and prompts/verify.md.
User-authored raw winning bytes are retained independently of normalized bodies.

```yaml
---
description: Example caller profile
model: codex/example-model:high
agents:
  - name: logic
    lenses: [bugs, tests]
stages:
  verify: claude/example-verifier:high
---
Inspect {{SCOPE}} in {{WORKDIR}} under the supplied project rules at {{PROFILE}}.
```

Ordinary runner syntax is provider[/model][:effort] with claude or codex. Efforts
are low, medium, high, xhigh, max; a provider change cannot inherit another
provider's model. Structured extension runners also support opencode. Stage-file
settings refine the profile fallback; profile stage overrides refine stage-file
settings. --lenses replaces the roster with one explicitly composed assignment.

Placeholders are SCOPE, GOAL, PROFILE, CONTEXT, WORKDIR and stage FINDINGS/SOURCES.
Only valid placeholder identifiers are substituted, once. Supplied values that
look like placeholders remain literal. Prior-round history is metadata/context,
not a second template pass. Project calibration is .revust/profile.md; round
input/profile.md wins, and the selected raw bytes are archived. See
[extensions](extensions-v1.md) and [CLI workflows](usage.md).
