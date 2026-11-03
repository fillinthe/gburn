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
