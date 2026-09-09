# Contributing to Cameo

Cameo is Apache-2.0. You do not need permission to clone, build, flash, or
fork it.

## The most useful contribution

Run it on a machine you own.

1. Flash a release ISO or a `main` artifact from Actions.
2. Install to a disk you are willing to erase, or stay on the live USB.
3. Open the console, start the starter model, send one chat with the network
   unplugged if you can.
4. On the box, run `cameo report --submit`. Credit is opt-in on that command.

If upload fails, paste the exact JSON at
[testers.html#retry](https://cameoconstruct.xyz/testers.html#retry). GitHub's
[hardware issue](.github/ISSUE_TEMPLATE/hardware-report.yml) is last resort.

Consented names go on [the roll](docs/testers.md) only after reviewer export.
That file is the public, git-backed record of people who tested Cameo before it
was a finished product. Consent is opt-in and can be withdrawn: open an issue
or mail the maintainer and the row is removed in the next commit.

## Code

- Match the surrounding style. Do not reformat unrelated files.
- Do not commit secrets, private keys, or model weights.
- Update or add a fixture when you change update, auth, or install behavior.
- `cargo test --workspace --locked` for Rust. Field tests live under `daedalus/field`.

## What this project is not asking for yet

Drive-by refactors, extra frameworks, and “rewrite it in X” are noise. A
reproducible hardware report beats a speculative patch.

## Maintainers

Korbin Sadlowski is the current release owner. See [SECURITY.md](SECURITY.md)
for private vulnerability reports.
