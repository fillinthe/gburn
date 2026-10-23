GBurn 0.2.0: everything added since 0.1.0, and the manual at [gburn.app/docs](https://gburn.app/docs/).

**New**
- PCB files: Gerber (copper, mask, silkscreen, outline) and Excellon drill files from KiCad, Eagle and Altium.
- Pictures follow the layer's scan angle, and can be resampled to the layer's scan resolution.
- Fill layers: edge loops after the fill, a fill inset, and evenly spread lines.
- Arrange › Edit Cut Order: click shapes in the order to cut them.
- Arrays (grid, circular, along a path) copy whole shapes; variable text counts up across the copies, with a per-text Offset.
- Material Library entries stay linked to the layers that use them.
- Saving G-code or a Ruida file advances serial numbers and CSV rows, as Start does.
- Text: Fit to can shrink a whole line; font search, favourites and hidden fonts.
- G-code: M73 progress lines; start / end G-code tags ({minutes}, {diameter}, bed size, name, date).
- Nodes › Sharp corner; View › Flash the Layer's Shapes; Line tool ⇧ = 15° steps.
- The progress bar shows about how long is left; Start reads "Start · rotary" while the rotary is on.
- Files given on the command line open (Windows "Open with", Linux file managers).
- Help › Documentation opens the online manual.

**Fixed**
- Arrays used to drop text settings and groups, and circular arrays didn't turn pictures.
- How Do I… answers used some old menu names.

**Notes**
- The Ruida and galvo drivers are still untested on real machines: frame first and test at low power on scrap.
- The macOS app is not notarized and the Windows files are not code-signed yet (see the README for the first start).
