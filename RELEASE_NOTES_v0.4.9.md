# GBurn 0.4.9

- **USB lasers that restart when connected** (many ESP32 / STM32 diode lasers): GBurn now reopens the port by itself instead of reporting the laser lost (on Windows this showed "Lost the laser: Access denied").
- **Settings › General › Send problem reports** (on by default) turns error reports off. Signed in on the desktop, a report now carries your account so support can match it; file names and paths are never sent.
- The web version explains what to do when a browser has WebGL turned off, and error reports carry the right version.

Includes everything in 0.4.8 (busy USB / network GRBL ports no longer stop a job; Dutch, Polish, Turkish and Korean; Help opens the online manual).

Downloads below; check them against `SHA256SUMS.txt`. The Mac app is signed and notarized by Apple.
