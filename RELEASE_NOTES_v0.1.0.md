First public desktop build of GBurn.

**Highlights**
- Laser design: shapes, curves, text in any font (on a path, single-line fonts), QR / Data Matrix / barcodes, booleans, offsets, nodes, arrays, nesting, pages.
- Layers with line, fill, offset fill and image modes; kerf, tabs, lead-ins, overscan, ramps, sub-layers; material library and test patterns.
- Pictures: many dithering modes, adjustments, tracing, 3D slices.
- Preview in time order with a speed control and a material look.
- Lasers: GRBL (USB, network), Ruida (network, USB), EZCAD2 fibre galvo (USB). Frame, start, pause, stop; rotary; print and cut; camera overlay; variable text.
- Opens SVG, DXF, PDF, AI, EPS, HPGL and LightBurn projects; exports G-code, Ruida .rd, SVG, DXF, PDF and LightBurn projects.
- Asks before closing with unsaved work, and stops the laser if you close during a job.

**Notes**
- No account needed on the desktop; signing in adds cloud designs shared with gburn.app.
- The Ruida and galvo drivers are new and have only been tested against stand-ins, not real machines yet: frame first and test at low power on scrap.
- The macOS app is not notarized and the Windows files are not code-signed yet (see the README for the first start).
- Windows was self-checked under Wine; the Linux builds start and draw in a container. Linux ARM64 is untested on real hardware.
