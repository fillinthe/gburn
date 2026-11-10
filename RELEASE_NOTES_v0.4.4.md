# GBurn 0.4.4

- **G-code files open** (.gcode, .nc, .ngc, .tap): their burn moves come in as shapes you can see, edit and burn again. To send a file exactly as it is, use Laser › Run a G-code File.
- Safer while a job runs: nothing can fire, frame, home or jog the laser until you Pause or Stop, from any menu, shortcut, the command palette or automation.
- Resume works on Ruida, galvo and K40; pausing a Marlin laser turns the beam off; a paused Ruida job finishes; a pulled USB cable is noticed.
- GBurn Link only answers gburn.app and only reaches laser ports on your own network.
- Cut settings show the everyday rows first, with the rest under More settings; the machine list shows bed size and power in columns.
- Start here goes through the review card; undo after opening a file can't overwrite it; Current position jobs end where they started.

Downloads: macOS (universal, signed and notarized), Windows x64, Linux x86_64 / aarch64, GBurn Link for macOS. Check them against SHA256SUMS.txt.
