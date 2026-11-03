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
