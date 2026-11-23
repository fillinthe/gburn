# dmgbuild settings for GBurn's dmg (build.sh: dmgbuild -s dmg-settings.py -D app=… -D bg=… GBurn out.dmg)
import os

app = defines["app"]
format = "UDZO"
filesystem = "HFS+"
files = [app]
symlinks = {"Applications": "/Applications"}
background = defines["bg"]
window_rect = ((200, 120), (660, 400))
icon_size = 112
text_size = 13
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
icon_locations = {os.path.basename(app): (165, 200), "Applications": (495, 200)}
