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
