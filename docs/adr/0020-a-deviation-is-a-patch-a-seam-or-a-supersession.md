# A deviation is a patch, a seam, or a supersession

ADR-0005 forbids hand-editing vendored code and gives every deviation a
mechanical form. What it does not say is which form a given deviation should
take, and the question has now come up enough times to deserve an answer that
is not re-derived per argument. The predictor of maintenance pain turned out
not to be *which file* is touched but *what kind of change* is wanted, so the
rule is written in those terms.

## The rule

**A mechanical change is a patch.** Text edits that carry no judgment: widen a
visibility for the crate split, stub a platform module, ignore a test that
cannot run here. `vendor/patches/0006` and `0007` are the pattern — they widen
herdr's JSON and CST-splice primitives to `pub(crate)` so an amon seam can edit
`settings.json` with herdr's own formatting-preserving code rather than a lossy
reserialize; not one line of herdr's behaviour changes, only what amon may
call. Patches live in `vendor/patches/`, apply with `git apply`,
and fail loudly when upstream moves. A patch that encodes an opinion — a
different behaviour, a rewritten function — is in the wrong tier: it will
conflict on every re-vendor and the conflict will need that opinion re-argued
each time.

**An additive change is a seam.** We want theirs *and* ours: a reader that
consumes the engine's output, an extra hook registered through upstream's own
`ensure_command_hook`, an amon-authored script installed beside a vendored one
— which the vendored assets' own headers invite. Seams are ordinary amon code
in amon files, calling vendored code through its public (or patch-widened)
surface. They never touch vendored text, so they cannot conflict.

**A substitutive change is a supersession.** We want ours *instead of*
theirs. The vendored implementation is not edited and not deleted: the
amon-owned call site stops routing through it, and amon's replacement takes
over the role. The vendored copy stays in the tree, refreshed by every
re-vendor, listed in `SUPERSEDED` in `scripts/revendor.sh` — and the script
reports its drift separately, so upstream's changes to code we no longer run
are still *seen* at the moment we would otherwise stop looking.

## Obligations

**Superseding a file never ends the watch on it.** The drift report is the
price of the freedom: we stopped taking upstream's implementation, not
tracking their understanding. A supersession whose upstream reference was
dropped from vendoring is a fork, which is the thing this whole structure
exists to avoid.

**Whatever a seam adds, amon removes.** Upstream's uninstall cannot know about
hooks amon registered beside theirs. `amon remove` and `amon doctor` own the
lifecycle of exactly what amon added — built on upstream's config-editing
primitives, never a parallel editor.

## Consequences

ADR-0005 stands unchanged in what it forbids; this records how to choose among
the forms it allows, and adds the supersession tier. The config-file surgery,
the terminal, the detect engine and its manifests all remain fully upstream's
— nothing here weakens the reason they were vendored (ADR-0001): to keep
pulling their fixes.

The test of this document is a proposal arriving in the wrong tier. A 40-line
semantic patch should be turned away toward supersession; a fork of the config
editor should be turned away toward a seam. If either happens anyway, this
ADR, not taste, is what the review points at.

`SUPERSEDED` is empty today. The first candidate is known — the python-spawning
hook assets, replaced by scripts that call `amon hook` — but that replacement
is its own change, made when its feature needs it, not as a side effect of
writing the rule down.
