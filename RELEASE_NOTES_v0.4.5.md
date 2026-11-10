# GBurn 0.4.5

- **Windows without a graphics driver** (virtual machines, Remote Desktop, older PCs): GBurn no longer closes without a word. It switches to software drawing by itself (Mesa, in the `software` folder: keep it next to gburn.exe); slower, but it works. If even that fails, a message says what to install.
- G-code files you open come in on a Line layer, so they're traced as cuts, not filled.
- Icons: the Windows icon has clear corners, and the Linux menu entry shows GBurn's icon.

Downloads: macOS (universal, signed and notarized), Windows x64, Linux x86_64 / aarch64, GBurn Link for macOS. Check them against SHA256SUMS.txt.
