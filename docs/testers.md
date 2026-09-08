# The roll

Cameo is open source under Apache-2.0. The people who flash an unfinished OS
onto real AMD hardware are the ones who make a release honest. This file is
their public record. It lives in git, so a name here lasts as long as the
repository does.

Names are added **only with explicit consent**. A hardware report can stay
anonymous. A GitHub handle is enough. Full legal names are never inferred.

To be listed: open a [hardware report](https://github.com/sadlowskik/Cameo/issues/new?template=hardware-report.yml)
and check the consent box, or email the maintainer the same facts. The
canonical machine-readable roster is [`testers/roster.json`](../testers/roster.json).

## How a row is written

| Field | Meaning |
|---|---|
| Name | The string they asked to publish |
| Machine | GPU and enough host detail to be useful, not a street address |
| Artifact | ISO tag, commit, or container digest they actually booted |
| What they proved | Boot, chat, update, soak — only what they reported |
| Date | UTC day the report was accepted |

## Testers

No one is on the roll yet. The first consented hardware report becomes row one.

## Maintainers

| Name | Role |
|---|---|
| Korbin Sadlowski | Author and release owner |

See [CONTRIBUTING.md](../CONTRIBUTING.md) and [SECURITY.md](../SECURITY.md).
