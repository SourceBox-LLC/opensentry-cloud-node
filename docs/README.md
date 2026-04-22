# CloudNode docs

Supplementary documentation for SourceBox Sentry CloudNode. The top-level `README.md` is the user-facing install + operation guide; `AGENTS.md` is the developer / LLM-facing architecture reference. The docs in this tree cover the things that don't fit cleanly in either.

## Runbooks (`docs/runbooks/`)

Step-by-step procedures for when something's gone wrong. Each runbook names the symptom, lists what access you need, walks through the diagnostic steps in order, and ends with a rollback + escalation path.

- [video-not-showing.md](runbooks/video-not-showing.md) — camera registered but the tile is black / buffering / stuck on "stream not started yet"

## Architecture Decision Records (`docs/adr/`)

One decision per file, numbered in order. ADRs capture the *why* behind a non-obvious choice so future maintainers don't re-litigate it. Format per Michael Nygard's template (Context / Decision / Consequences).

- [0001-pi-software-encoding.md](adr/0001-pi-software-encoding.md) — why the Raspberry Pi path uses libx264 and not the hardware `h264_v4l2m2m` encoder

## Writing new docs

- **Runbook** — when you catch yourself pasting the same sequence of commands into more than one support thread. Cheap to write, saves time forever.
- **ADR** — when you make a decision that was hard to make, or that someone else will almost certainly re-argue. Write it *while the tradeoffs are fresh*, not six months later.
- **README / AGENTS** — these two are the primary docs and get updated in-place with every feature. Don't fork them into `docs/`.
