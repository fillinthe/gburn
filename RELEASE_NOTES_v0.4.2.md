GBurn 0.4.2: the xTool F1 on older firmware. Manual: [gburn.app/docs/connect](https://gburn.app/docs/connect/).

**xTool F1 (firmware before 40.51)**
- Machine Settings › Controller › xTool F1, blue laser or infrared laser, or pick the F1 in Choose your machine. Connect the USB cable; GBurn finds the F1 at its usual USB address, or type it in Machine Settings.
- Start, Pause, Stop, Frame, jog and Home. Frame is beam off unless you set % beam while framing.
- xTool's firmware 40.51 and later closed the connection GBurn uses, so an updated F1 won't connect (GBurn says so). The F1 Ultra, F1 Lite and other xTool network machines aren't supported.

**Notes**
- New and not yet tried on a real F1: run a small orientation test at low power on scrap first, and keep a hand near Stop.
- The macOS app is signed and notarized. The Windows files are not code-signed yet (SmartScreen: More info, Run anyway).
