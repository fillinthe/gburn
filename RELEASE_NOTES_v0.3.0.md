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
