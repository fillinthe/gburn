GBurn for Windows 10/11 (64-bit)

Unzip anywhere and run gburn.exe. Nothing is installed.

Windows may show "Windows protected your PC" the first time: click "More info", then "Run anyway".
The app is not yet signed with a certificate.

GBurn draws with OpenGL 2 (any PC made after 2010 has it). Without it (a virtual machine with no graphics
driver, Remote Desktop) GBurn switches to software drawing on its own: slower, but it works. That is what
the software folder is (Mesa, MIT licence): keep it next to gburn.exe.

GBurn is free. No account is needed; signing in (the same free account as GCarve) adds cloud designs
shared with the online version at https://gburn.app/app/

GBurn drives GRBL lasers over USB, Ruida lasers (Thunder Nova) over the network and EZCAD2 galvo markers over USB
(the galvo needs the WinUSB driver: Zadig, or the driver LightBurn installs; EZCAD's own driver blocks other programs).

gburn-link.exe is only for the online version (gburn.app in a browser) to reach a Ruida laser: keep it open while
you use the web app. The desktop app does not need it.
