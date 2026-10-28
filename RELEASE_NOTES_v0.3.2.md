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
