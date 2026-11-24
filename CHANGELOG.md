# Changelog

Every GBurn release, newest first. Downloads are on the [releases page](https://github.com/fillinthe/gburn/releases).

## 0.4.11

- **Dragging a shape moves it.** A quick drag that started just inside a shape's edge drew a selection box instead of moving the shape, and one that started near a corner resized it. The press now counts where the button went down.
- Corner handles reach less on small shapes, and the pointer shows whether a drag will move, resize or turn.
- After you draw a shape the tool goes back to Select, so the next drag moves it (Settings can keep the drawing tool on).
- With the Select tool, the top bar shows the selection's **X, Y, W and H** to type into.
- **Variables** (Edit › Variables): name a size and type the name in the size boxes; sums and units work (`gap * 2 + 1in`). Saved with the design.
- **Favourite machines**: a star on each row of Choose your machine keeps yours at the top.
- LightBurn files keep their dither setting, in and out.

## 0.4.10

- **Tidier settings.** Machine Settings is now eight short pages down the left (Work area, Motion, Framing and safety, Controller, G-code, Galvo, Rotary, Cut order), showing only the ones your laser uses; no page scrolls.
- The cut settings' **More settings** are grouped: Path, Lead-ins and Effects for line layers; Lines and Fill for fills and pictures; Timing and Z for every layer.
- With no laser connected, the laser panel shrinks to two lines (your machine with Connect, the job time with Simulate), so the cut settings fit. Start, Frame and Start from appear once you connect.

## 0.4.9

- **USB lasers that restart when connected** (many ESP32 / STM32 diode lasers): GBurn now reopens the port by itself instead of reporting the laser lost (on Windows this showed "Lost the laser: Access denied").
- **Settings › General › Send problem reports** (on by default) turns error reports off. Signed in on the desktop, a report now carries your account so support can match it; file names and paths are never sent.
- The web version explains what to do when a browser has WebGL turned off, and error reports carry the right version.

Includes everything in 0.4.8 (busy USB / network GRBL ports no longer stop a job; Dutch, Polish, Turkish and Korean; Help opens the online manual).

## 0.4.8

- **GRBL lasers over USB or the network no longer stop part way** when the port is briefly busy (on Windows: "Lost the laser: failed to write whole buffer"). Bytes the port can't take yet wait their turn instead of being dropped.
- The interface is now in **Dutch, Polish, Turkish and Korean** too (12 languages).
- **Help › User Manual** and the **?** button open the manual at [gburn.app/docs](https://gburn.app/docs/) in your language, always the latest.

## 0.4.7

- **Mac: closing the window no longer quits.** GBurn keeps running like other Mac apps: click its Dock icon to bring it back; Quit (⌘Q) ends it. While a job runs the window stays open. Window › Close Window (⌘W).
- **Settings in tabs**: General, Mouse, Design, Laser.
- **Tool bar above the drawing**: the tool you're using, its settings (corner radius for rectangles, sides and star for polygons) and how to use it.
- **The donate button can be hidden**: right-click the ♥ in the top bar, or Settings › General. Help › Donate stays.
- Dialogs: the scroll bar sits against the edge.

Downloads: macOS (universal, signed and notarized), Windows x64, Linux x86_64 / aarch64, GBurn Link for macOS. Check them against SHA256SUMS.txt.

## 0.4.6

- **Galvo lens correction files** (`.cor`): GBurn read the field size in the file wrong (Thunder's Aurora 110 mm file came in as 497 mm, so marks would come out far too small). It now reads the right field (about 132 mm for that file). If you loaded a `.cor` before, load it again in Machine Settings › Galvo.
- GRBL lasers: position reports are read with the shared GRBL code's newer, unit-aware reader.

Downloads: macOS (universal, signed and notarized), Windows x64, Linux x86_64 / aarch64, GBurn Link for macOS. Check them against SHA256SUMS.txt.

## 0.4.5

- **Windows without a graphics driver** (virtual machines, Remote Desktop, older PCs): GBurn no longer closes without a word. It switches to software drawing by itself (Mesa, in the `software` folder: keep it next to gburn.exe); slower, but it works. If even that fails, a message says what to install.
- G-code files you open come in on a Line layer, so they're traced as cuts, not filled.
- Icons: the Windows icon has clear corners, and the Linux menu entry shows GBurn's icon.

Downloads: macOS (universal, signed and notarized), Windows x64, Linux x86_64 / aarch64, GBurn Link for macOS. Check them against SHA256SUMS.txt.

## 0.4.4

- **G-code files open** (.gcode, .nc, .ngc, .tap): their burn moves come in as shapes you can see, edit and burn again. To send a file exactly as it is, use Laser › Run a G-code File.
- Safer while a job runs: nothing can fire, frame, home or jog the laser until you Pause or Stop, from any menu, shortcut, the command palette or automation.
- Resume works on Ruida, galvo and K40; pausing a Marlin laser turns the beam off; a paused Ruida job finishes; a pulled USB cable is noticed.
- GBurn Link only answers gburn.app and only reaches laser ports on your own network.
- Cut settings show the everyday rows first, with the rest under More settings; the machine list shows bed size and power in columns.
- Start here goes through the review card; undo after opening a file can't overwrite it; Current position jobs end where they started.

Downloads: macOS (universal, signed and notarized), Windows x64, Linux x86_64 / aarch64, GBurn Link for macOS. Check them against SHA256SUMS.txt.

## 0.4.3

GBurn 0.4.3: the xTool M1 on older firmware. Manual: [gburn.app/docs/connect](https://gburn.app/docs/connect/).

**xTool M1 (firmware before 40.18), desktop app**
- Laser tab › xTool M1 over USB network (older firmware), or pick the M1 in Choose your machine. Connect the USB cable; GBurn finds the M1 at its usual USB address, or type it in Machine Settings.
- Set Machine Settings › Stock thickness: the M1 focuses by head height.
- Start sends the job; press the M1's button when it lights blue to run it. GBurn shows when it runs and finishes; Stop works from GBurn. Pause with the M1's button. Jog, Home and Frame aren't offered for the M1.
- Laser only, no blade. Newer M1 firmware and the M1 Ultra aren't supported.

**Notes**
- New and not yet tried on a real M1: test at low power on scrap, and keep a hand near Stop.
- The macOS app is signed and notarized. The Windows files are not code-signed yet (SmartScreen: More info, Run anyway).

## 0.4.2

GBurn 0.4.2: the xTool F1 on older firmware. Manual: [gburn.app/docs/connect](https://gburn.app/docs/connect/).

**xTool F1 (firmware before 40.51)**
- Machine Settings › Controller › xTool F1, blue laser or infrared laser, or pick the F1 in Choose your machine. Connect the USB cable; GBurn finds the F1 at its usual USB address, or type it in Machine Settings.
- Start, Pause, Stop, Frame, jog and Home. Frame is beam off unless you set % beam while framing.
- xTool's firmware 40.51 and later closed the connection GBurn uses, so an updated F1 won't connect (GBurn says so). The F1 Ultra, F1 Lite and other xTool network machines aren't supported.

**Notes**
- New and not yet tried on a real F1: run a small orientation test at low power on scrap first, and keep a hand near Stop.
- The macOS app is signed and notarized. The Windows files are not code-signed yet (SmartScreen: More info, Run anyway).

## 0.4.1

GBurn 0.4.1: UV galvo lasers and the xTool S1. Manual: [gburn.app/docs/connect](https://gburn.app/docs/connect/).

**UV galvo lasers (EZCAD2 board)**
- Machine Settings › Galvo › UV, beside Fibre and CO2. Set Frequency on the layer (30–50 kHz is usual). If low values mark darker than high ones, tick Invert power.
- Before Start, GBurn reminds you that UV light at 355 nm is invisible: wear glasses rated for 355 nm and keep the lid shut.
- The Monport UV Galvo and Cloudray TR UV in Choose your machine set it up.

**xTool S1 over USB**
- Machine Settings › Controller › xTool S1 (USB), or pick the S1 in Choose your machine. Close xTool Studio first: it holds the port.
- Jog, Frame (beam off), Home, Start, Pause and Stop; lid-open and flame alarms from the machine show in GBurn. The lid must be closed even to jog or frame.
- USB only. The other xTool machines aren't supported yet: their current firmware uses an encrypted protocol.

**Notes**
- Both are new and not yet tried on a real machine: test at low power on scrap, and keep a hand near Stop.
- The macOS app is signed and notarized. The Windows files are not code-signed yet (SmartScreen: More info, Run anyway).

## 0.4.0

GBurn 0.4.0: K40 lasers, and G-code that suits more firmwares. Manual: [gburn.app/docs](https://gburn.app/docs/).

**K40 lasers (stock M2 / M3 Nano board)**
- Pick "K40 over USB" in the Laser tab (or OMTech K40+ in Choose your machine). Frame, jog, Home, Start, Pause and Stop. File › Export › K40 file (.egv) saves the job for MeerK40t or K40 Whisperer.
- Power is the machine's knob. Frame and jogs are beam-off moves; after a Stop, press Home before the next job.
- Windows: the board needs the WinUSB driver (Zadig) instead of the one LaserDRW installs. Linux: a udev rule for 1a86:5512.
- New and not yet tested on a real K40: try it at low power on scrap.

**G-code for more firmwares** (Machine Settings › Controller)
- New: RepRapFirmware (Duet), Snapmaker, and Marlin with the laser on the fan pin (M106).
- Marlin: the laser now switches on with `M3 I` and every move carries its power. Stock Creality firmware hung on M4, and outlines after a travel move could come out unburnt.
- Pauses and dots now use each firmware's own unit (milliseconds outside GRBL, which counts seconds), so they no longer flash past.
- Fire and Stop use the right commands for each firmware (Stop also switches off a fan-pin laser).

**Also**
- 14 more machines in Choose your machine match a driver: OMTech, Monport, Cloudray and Gweike fibre / CO2 galvos (EZCAD2), Gweike Cloud and Monport MEGAS (Ruida).
- Linux: the computer stays awake during a job, and a USB "Permission denied" says how to fix it. Windows: Print copes with any design name.

**Notes**
- The macOS app is signed and notarized. The Windows files are not code-signed yet (SmartScreen: More info, Run anyway).
- Test at low power on scrap first, and keep a hand near Stop.

## 0.3.2

GBurn 0.3.2: more fixes for problems laser owners keep reporting. Manual: [gburn.app/docs](https://gburn.app/docs/) (Troubleshooting explains each message).

**Ruida: clear messages instead of "busy or paused"**
- No answer at all: check the address, the cable, the network, and that LightBurn isn't holding the port. If answers come from a different address, GBurn names it (the laser's address may have changed).
- Stopped answering after it had answered: look at the panel for an error or an open menu, or a full memory.
- Keeps refusing the data: damaged data, so try another cable or a wired link.
- Couldn't send: the address can't be reached (on a Mac, also check Local Network access).
- These now show in the status bar, not just the Laser tab's log.

**Better time estimates on Ruida**
- On connect, GBurn reads the controller's acceleration and top speeds and offers them for the time estimate (Laser tab › Use it). The readings are shown first, so compare them with the panel.

**Jobs that stop part way**
- GBurn keeps the computer awake while a job runs (Windows and Mac).
- On Windows, a dropped link explains how to turn off USB power saving (USB selective suspend).

Also in 0.3.1: Frame traces only what burns, a GRBL laser mode warning, Rotary › Check the size, the galvo parks after a frame, and Mac Local Network access.

**Notes**
- The macOS app is signed and notarized. The Windows files are not code-signed yet (SmartScreen: More info, Run anyway).
- Test at low power on scrap first, and keep a hand near Stop.

## 0.3.1

GBurn 0.3.1: fixes for problems laser owners keep reporting with other laser software. Manual: [gburn.app/docs](https://gburn.app/docs/).

**Fixed and improved**
- Frame traces only what will burn: no background photo or camera overlay, and a picture only where it has something dark to engrave, not its white or see-through margins (those made frames too big and triggered "outside the bed" warnings).
- GRBL: a warning when the controller's laser mode is off ($32=0), the cause of dark dots at corners and the start of each line.
- A lost connection mid-job says how to pick up again: reconnect, then Preview › ⋯ › Start here.
- Rotary › Check the size: burn a square, type what it measured round the object, and Correct fixes mm per turn (A axis: the diameter). No more trial and error.
- Fibre galvo: the mirrors go back to the centre after a frame.
- Mac: GBurn asks for Local Network access properly, so a Ruida over Ethernet or Wi-Fi can connect on macOS 15 and newer.
- The Mac installer window shows where to drag GBurn.

**Notes**
- The macOS app is signed and notarized. The Windows files are not code-signed yet (SmartScreen: More info, Run anyway).
- Test at low power on scrap first, and keep a hand near Stop.

## 0.3.0

GBurn 0.3.0: everything added since 0.2.0. Manual: [gburn.app/docs](https://gburn.app/docs/).

**Tested on real machines**
- Thunder Laser Nova 63 (Ruida) over USB: connect, status, position, the controller's bed, jog and Frame (beam off). Jobs not run on it yet.
- Thunder Laser Aurora 8 (EZCAD2 fibre galvo) over USB: set-up, frame and real marks. Swap X / Y and a scale fix line it up.

**New**
- Languages: Japanese and Simplified Chinese, beside Spanish, French, German, Italian and Brazilian Portuguese.
- Review before burn: Start shows the machine, size, time and every layer's settings first (Settings can turn it off).
- Choose your machine: a list of about 190 lasers sets the work area, home corner and controller.
- The laser panel (jog, Start, Pause, Stop, Frame, Simulate, Start from) sits under every tab and names the laser in use.
- Machine Settings in pages; inches or mm in every setting.
- Ruida over USB (desktop and browser), controller files, settings and info; Marlin and Smoothieware controllers.
- Trace over a picture by hand: lock it faded on a layer that never burns.
- Opening a LightBurn file starts a new design and keeps its groups.
- The Mac menu bar, double-click a .gburn in Finder to open it, a phone layout for the browser version.
- Help › Report a Problem sends what went wrong.

**Fixed**
- Ruida Frame no longer turns the beam on (the move command's red-light flag fired the laser on the Nova).
- Ruida jogs were relative moves; galvo USB reads and multi-packet jobs; dialogs that shifted when switching tabs.

**Notes**
- The macOS app is signed and notarized. The Windows files are not code-signed yet (SmartScreen: More info, Run anyway).
- Test at low power on scrap first, and keep a hand near Stop.

## 0.2.0

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

## 0.1.0

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
