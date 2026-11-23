# Third-party notices

GBurn links these crates (their licences allow it; no code is copied): egui / eframe (MIT or Apache-2.0), serialport
(MPL-2.0, unmodified), serde / serde_json (MIT or Apache-2.0), rfd (MIT), wasm-bindgen / web-sys (MIT or Apache-2.0).
It shares code with GCarve and GSketch (gcarve-core, gcarve-ui: same owner, same licence).

## Protocol knowledge

The Ruida byte swizzle, number packing and command notes in `rust/gburn-core/src/ruida.rs` follow the public
reverse-engineering notes (EduTech Wiki "Ruida", Stefan Schuermans' RDCAM notes) as implemented in **MeerK40t**
(`meerk40t/ruida/rdjob.py`), MIT licence, Copyright (c) 2019–2026 the MeerK40t authors. Protocol facts are not
copyrightable; the Rust here was written from the description. The `.rd` job encoder (`rd_commands`, header / layer / tail order and the
relative-move rule) is a port of `rdjob.py`'s `write_header` / `write_settings` / `jump` / `mark` / `write_tail`; copies
of the MeerK40t sources consulted are kept under `docs/research/meerk40t/` with their LICENSE. The EZCAD2 galvo
module (`galvo.rs`: opcodes, init sequence, list building, `.cor` reading) is a port of **galvoplotter** (MIT,
Copyright (c) 2023 MeerK40t; after Bryce Schroeder's balor), kept under `docs/research/galvoplotter/`. The K40 module
(`k40.rs` and the app's `k40_link.rs`: LHYMICRO-GL speed codes, moves, packets, the CH341 set-up and status reads) is a
port of MeerK40t's `lihuiyu` driver, `laserspeed` and `ch341` libusb driver (MIT, Copyright (c) 2021 meerk40t), kept under
`docs/research/meerk40t/lihuiyu/`; its one-wire CRC table follows lentz.com.au (2-clause BSD). The galvo's UV source (the CO2 commands, Invert power)
follows MeerK40t's balor `uv` source (MIT). The xTool S1 dialect and conversation (`gcode::Flavor::XTool`, the app's xTool
commands) were written from protocol facts in thecodingdad/ha-xtool and hilman2's ha-xtool-s1 (both MIT, Home Assistant
integrations) and LightBurn forum console logs; no code was copied. The xTool F1 (older firmware) session lines, port 8780 and the HTTP stop on 8080 follow BeamInk (sfn-tools/BeamInk, MIT, Copyright (c) 2026 something.fromnothing.blog - Tools); no code was copied. The xTool M1 job (header, footer, the allowed G-codes, Z 17 for focus) and its HTTP upload of a zipped `gcodes.txt` follow xtm1_toolkit (fritzw/xtm1_toolkit, MIT, Copyright (c) 2022 Fritz Webering); no code was copied. Copies are under `docs/research/`. The MIT notice:

    Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
    documentation files (the "Software"), to deal in the Software without restriction, including without limitation
    the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and
    to permit persons to whom the Software is furnished to do so, subject to the following conditions: The above
    copyright notice and this permission notice shall be included in all copies or substantial portions of the
    Software. THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND.

GRBL (GPL-3.0) is firmware GBurn talks to over a serial port; nothing of it is linked or copied. LightBurn is a
trademark of LightBurn Software; GBurn reads its .lbrn2 files and is not affiliated with it.

## Icons

The arrow, chevron, house and x icons in `rust/gburn-app/icons/` are from **Lucide** (lucide.dev), ISC licence:
Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT). All other copyright
(c) for Lucide are held by Lucide Contributors 2022. Permission to use, copy, modify, and/or distribute this software
for any purpose with or without fee is hereby granted, provided that the above copyright notice and this permission
notice appear in all copies. THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO
THIS SOFTWARE. The other icons are GBurn's own.

## Software drawing (Windows package)

`software/opengl32.dll` and `software/libgallium_wgl.dll` in the Windows zip are **Mesa 3D** (mesa3d.org; the llvmpipe build from
github.com/pal1000/mesa-dist-win), used only when the PC has no OpenGL 2 driver. Mesa's licence (MIT):

Copyright (C) 1999-2007 Brian Paul All Rights Reserved.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the Software without restriction, including without limitation the
rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit
persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the
Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## Fonts

The Japanese, Chinese and Korean interface fonts in `rust/gburn-app/fonts/` (`cjk-*.ttf`, `names-*.ttf`) are subsets of
**Noto Sans JP**, **Noto Sans SC** and **Noto Sans KR** (Google Fonts), Copyright 2014-2021 Adobe (http://www.adobe.com/), with Reserved
Font Name 'Source'. They are under the SIL Open Font License 1.1, in full in `rust/gburn-app/fonts/OFL.txt`.

## Menus on the Mac

The menu bar at the top of the screen on macOS uses **muda** (Tauri project), MIT or Apache-2.0 at your option; crates.io/crates/muda.
