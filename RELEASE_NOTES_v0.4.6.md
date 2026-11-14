# GBurn 0.4.6

- **Galvo lens correction files** (`.cor`): GBurn read the field size in the file wrong (Thunder's Aurora 110 mm file came in as 497 mm, so marks would come out far too small). It now reads the right field (about 132 mm for that file). If you loaded a `.cor` before, load it again in Machine Settings › Galvo.
- GRBL lasers: position reports are read with the shared GRBL code's newer, unit-aware reader.

Downloads: macOS (universal, signed and notarized), Windows x64, Linux x86_64 / aarch64, GBurn Link for macOS. Check them against SHA256SUMS.txt.
