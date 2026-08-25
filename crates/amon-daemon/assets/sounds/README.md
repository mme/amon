# Bundled notification sounds

`done.mp3` and `request.mp3` are herdr's, from `assets/sounds/` in
https://github.com/ogulcancelik/herdr — Copyright Ogulcan Celik, licensed under
the Apache License 2.0. See the repository NOTICE.

They are not byte-identical to herdr's: the originals are mastered quiet
(peaks at -10.2 and -8.1 dBFS), which made the notifications hard to hear
over anything else. Both were peak-normalized to -4.5 dBFS — a static gain,
no dynamics processing — with, from herdr's originals:

    ffmpeg -i done.mp3    -af volume=5.7dB -codec:a libmp3lame -b:a 320k done-out.mp3
    ffmpeg -i request.mp3 -af volume=3.6dB -codec:a libmp3lame -b:a 320k request-out.mp3

The -4.5 figure was found by ear on real speakers: -1.0 audibly clipped,
-3.5 was still slightly hot on `done`. Verify with
`ffmpeg -i <file> -af volumedetect -f null -`.

They are copied by hand rather than by `scripts/revendor.sh`. That script gives
every vendored file a provenance header and runs the token map over its
contents, both of which would corrupt an mp3, and growing a third category in it
for two files that will not change is not worth the machinery (ADR-0012). The
cost is that these are the one vendored thing `just revendor` cannot refresh —
which is why the provenance lives here instead of inside the files.
