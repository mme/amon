# Bundled notification sounds

`done.mp3` and `request.mp3` are herdr's, from `assets/sounds/` in
https://github.com/ogulcancelik/herdr — Copyright Ogulcan Celik, licensed under
the Apache License 2.0. See the repository NOTICE.

They are copied by hand rather than by `scripts/revendor.sh`. That script gives
every vendored file a provenance header and runs the token map over its
contents, both of which would corrupt an mp3, and growing a third category in it
for two files that will not change is not worth the machinery (ADR-0012). The
cost is that these are the one vendored thing `just revendor` cannot refresh —
which is why the provenance lives here instead of inside the files.
